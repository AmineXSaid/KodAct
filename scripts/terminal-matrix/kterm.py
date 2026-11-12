"""Drive the real `kodact` binary the way a person does, and read the screen.

Three parts, used by `run.py`:

- `Stub`: a local OpenAI-compatible endpoint that plays the model. Replies are
  scripted per prompt: text, tool calls, thinking, slow replies, HTTP errors, a
  dropped connection and a malformed stream. No model and no key are needed.
- `Term`: `kodact` in a pseudo-terminal, with a terminal emulator (pyte)
  reading what it draws. Keys go in as the bytes a terminal sends; checks read
  the screen, the terminal modes left on, and the files on disk.
- `Sandbox`: a throwaway home, state folder and project for one check.

Nothing here knows about any one check.
"""

import fcntl
import json
import os
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import tempfile
import termios
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pyte

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))

# Keys, as the bytes an xterm sends.
ENTER, ESC, TAB, BACKTAB, BS = "\r", "\x1b", "\t", "\x1b[Z", "\x7f"
UP, DOWN, RIGHT, LEFT = "\x1b[A", "\x1b[B", "\x1b[C", "\x1b[D"
HOME, END, PGUP, PGDN, DEL = "\x1b[H", "\x1b[F", "\x1b[5~", "\x1b[6~", "\x1b[3~"
ALT_ENTER = "\x1b\r"


def ctrl(c):
    return chr(ord(c.upper()) - 64)


def paste(text):
    return "\x1b[200~" + text + "\x1b[201~"


# --------------------------------------------------------------------------
# The stub endpoint


class Stub:
    """An OpenAI-compatible endpoint with scripted replies.

    `rules` maps a substring of the person's prompt to a list of steps. The
    newest user message naming a rule chooses the script; the step is the
    number of assistant messages after it. A step is a dict:

      text        the reply's text
      tools       [(name, args), ...] tool calls in this reply
      thinking    reasoning text sent before the answer
      delay       seconds to wait before the first byte
      chunk_delay seconds between text chunks (the text is sent word by word)
      status      an HTTP error status instead of a reply (401, 429, 500...)
      times       how many times `status` is sent before the step succeeds
      drop        close the connection partway through the stream
      malformed   send a broken event stream
      usage       prompt tokens to report
    A prompt with no rule is answered `echo: <prompt>`.
    """

    def __init__(self, models=("stub-model",), window=None, key=None):
        self.rules = {}
        self.requests = []
        self.paths = []
        self.failures = {}
        self.models = list(models)
        self.window = window
        self.key = key
        self.lock = threading.Lock()
        stub = self

        class H(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *a):
                pass

            def send_json(self, code, obj, headers=()):
                body = json.dumps(obj).encode()
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                for k, v in headers:
                    self.send_header(k, v)
                self.end_headers()
                self.wfile.write(body)

            def authorized(self):
                return stub.key is None or self.headers.get("Authorization") == f"Bearer {stub.key}"

            def do_GET(self):
                if not self.authorized():
                    return self.send_json(401, {"error": {"message": "Incorrect API key provided."}})
                data = [{"id": m, "object": "model"} for m in stub.models]
                if stub.window:
                    for d in data:
                        d["context_length"] = stub.window
                self.send_json(200, {"object": "list", "data": data})

            def event(self, obj):
                data = f"data: {json.dumps(obj)}\n\n".encode()
                self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
                self.wfile.flush()

            def chunk(self, model, delta=None, finish=None, usage=None):
                c = {"id": "x", "object": "chat.completion.chunk", "created": 0, "model": model,
                     "choices": [] if usage else [{"index": 0, "delta": delta or {}, "finish_reason": finish}]}
                if usage:
                    c["usage"] = usage
                self.event(c)

            def sse(self, events):
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                for name, obj in events:
                    data = ((f"event: {name}\n" if name else "") + f"data: {json.dumps(obj)}\n\n").encode()
                    self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
                self.wfile.write(b"0\r\n\r\n")
                self.wfile.flush()

            def messages_api(self, req):
                """The Messages API: the same scripts, text only."""
                if stub.key is not None and self.headers.get("x-api-key") != stub.key:
                    return self.send_json(401, {"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}})
                users = [m for m in req.get("messages", []) if m.get("role") == "user"]
                step, _ = stub.step_for({"messages": [{"role": m["role"], "content": Stub.text_of(m)} for m in req.get("messages", [])]})
                text = step.get("text", "")
                usage = {"input_tokens": 100, "output_tokens": 10}
                self.sse([
                    ("message_start", {"type": "message_start", "message": {"id": "m", "type": "message", "role": "assistant",
                                       "model": req.get("model"), "content": [], "stop_reason": None, "usage": usage}}),
                    ("content_block_start", {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                    ("content_block_delta", {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}),
                    ("content_block_stop", {"type": "content_block_stop", "index": 0}),
                    ("message_delta", {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 10}}),
                    ("message_stop", {"type": "message_stop"}),
                ])

            def gemini_api(self, req):
                if stub.key is not None and self.headers.get("x-goog-api-key") != stub.key:
                    return self.send_json(401, {"error": {"code": 401, "message": "API key not valid.", "status": "UNAUTHENTICATED"}})
                msgs = [{"role": "user" if c.get("role") == "user" else "assistant",
                         "content": " ".join(p.get("text", "") for p in c.get("parts", []))} for c in req.get("contents", [])]
                step, _ = stub.step_for({"messages": msgs})
                self.sse([(None, {"candidates": [{"content": {"role": "model", "parts": [{"text": step.get("text", "")}]},
                                                  "finishReason": "STOP", "index": 0}],
                                  "usageMetadata": {"promptTokenCount": 100, "candidatesTokenCount": 10, "totalTokenCount": 110}})])

            def do_POST(self):
                req = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
                with stub.lock:
                    stub.requests.append(req)
                    stub.paths.append(self.path)
                if self.path.endswith("/messages") or "/messages?" in self.path:
                    return self.messages_api(req)
                if "GenerateContent" in self.path:
                    return self.gemini_api(req)
                if self.path.rstrip("/") != "/v1/chat/completions":
                    return self.send_json(404, {"error": {"message": f"no such path {self.path}"}})
                if not self.authorized():
                    return self.send_json(401, {"error": {"message": "Incorrect API key provided."}})
                model = req.get("model", "stub-model")
                if model not in stub.models and "*" not in stub.models:
                    return self.send_json(404, {"error": {"message": f"The model `{model}` does not exist."}})
                step, key = stub.step_for(req)
                if step.get("delay"):
                    time.sleep(step["delay"])
                if step.get("status"):
                    with stub.lock:
                        n = stub.failures.get(key, 0)
                        stub.failures[key] = n + 1
                    if n < step.get("times", 1 << 30):
                        hdr = [("Retry-After", "1")] if step["status"] == 429 else []
                        return self.send_json(step["status"], {"error": {"message": f"stub error {step['status']}"}}, hdr)
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                try:
                    self.stream(model, req, step)
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def stream(self, model, req, step):
                if step.get("malformed"):
                    data = b"data: {\"choices\": [{\"delta\": {\"content\": \"brok\n\ndata: not json at all\n\n"
                    self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n0\r\n\r\n")
                    return
                self.chunk(model, {"role": "assistant"})
                if step.get("thinking"):
                    for w in re.findall(r"\S+\s*", step["thinking"]):
                        self.chunk(model, {"reasoning_content": w})
                        time.sleep(step.get("chunk_delay", 0))
                text = step.get("text", "")
                parts = re.findall(r"\S+\s*|\s+", text) if step.get("chunk_delay") else [text] if text else []
                for i, w in enumerate(parts):
                    self.chunk(model, {"content": w})
                    if step.get("drop") and i >= len(parts) // 2:
                        self.wfile.flush()
                        self.connection.shutdown(2)
                        return
                    if step.get("chunk_delay"):
                        time.sleep(step["chunk_delay"])
                if step.get("drop"):
                    self.connection.shutdown(2)
                    return
                tools = step.get("tools") or []
                for i, (name, args) in enumerate(tools):
                    self.chunk(model, {"tool_calls": [{"index": i, "id": f"call_{len(stub.requests)}_{i}", "type": "function",
                                                       "function": {"name": name, "arguments": json.dumps(args)}}]})
                self.chunk(model, finish="tool_calls" if tools else "stop")
                prompt = step.get("usage") or len(json.dumps(req.get("messages", []))) // 4
                self.chunk(model, usage={"prompt_tokens": prompt, "completion_tokens": 20, "total_tokens": prompt + 20})
                data = b"data: [DONE]\n\n"
                self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n0\r\n\r\n")
                self.wfile.flush()

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.server.daemon_threads = True
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}/v1"

    @staticmethod
    def text_of(m):
        c = m.get("content")
        if isinstance(c, str):
            return c
        return " ".join(p.get("text", "") for p in c or [] if isinstance(p, dict))

    def step_for(self, req):
        msgs = req.get("messages", [])
        users = [i for i, m in enumerate(msgs) if m.get("role") == "user"]
        last = self.text_of(msgs[users[-1]]) if users else ""
        if "is about to be replaced by a summary" in last:
            return {"text": "<summary>The person and KodAct talked; the facts so far are kept. SUMMARY-MARK</summary>"}, "summary"
        # A "ping" is the harness asking whether the session still answers: always echoed, whatever
        # unanswered prompt KodAct sent along with it.
        pings = re.findall(r"ping-[\w-]+", re.sub(r"<system-reminder>.*?</system-reminder>", "", last, flags=re.S))
        if pings:
            return {"text": "echo: " + pings[-1]}, "echo"
        for i in reversed(users):
            text = self.text_of(msgs[i])
            # The person's words, without the reminders KodAct adds around them.
            said = re.sub(r"<system-reminder>.*?</system-reminder>", "", text, flags=re.S)
            if "Here is a summary of the conversation" in said:
                continue
            for key in sorted(self.rules, key=len, reverse=True):
                if key in said:
                    script = self.rules[key]
                    n = sum(1 for m in msgs[i + 1:] if m.get("role") == "assistant")
                    return (script[n] if n < len(script) else {"text": "ok."}), f"{key}#{n}"
            if said.strip():
                break
        said = re.sub(r"<system-reminder>.*?</system-reminder>", "", last, flags=re.S).strip()
        # After a `!command` its output and the next prompt are one message: echo the prompt.
        if "</shell-output>" in said:
            said = said.rsplit("</shell-output>", 1)[1].strip() or "(a shell command's output)"
        # After a compaction the summary and the new prompt are one message: echo its end.
        if "Here is a summary of the conversation" in said:
            return {"text": "echo: " + said[-120:].strip().splitlines()[-1]}, "echo"
        return {"text": "echo: " + said[:200]}, "echo"

    def prompts(self):
        """The last user message of each chat request, in order."""
        out = []
        for r in self.requests:
            users = [m for m in r.get("messages", []) if m.get("role") == "user"]
            if users:
                out.append(self.text_of(users[-1]))
        return out

    def close(self):
        self.server.shutdown()
        self.server.server_close()


# --------------------------------------------------------------------------
# A throwaway home and project


class Sandbox:
    def __init__(self, git=True):
        self.root = tempfile.mkdtemp(prefix="kodact-matrix-")
        self.home = os.path.join(self.root, "home")
        self.kodact_home = os.path.join(self.root, "kodact")
        self.project = os.path.join(self.root, "project")
        for d in (self.home, self.kodact_home, self.project):
            os.makedirs(d)
        with open(os.path.join(self.project, "notes.txt"), "w") as f:
            f.write("first line\nsecond line\n")
        if git:
            run = lambda *a: subprocess.run(["git", *a], cwd=self.project, capture_output=True, check=True)
            run("init", "-q", "-b", "main")
            run("-c", "user.email=t@example.com", "-c", "user.name=t", "add", "-A")
            run("-c", "user.email=t@example.com", "-c", "user.name=t", "commit", "-q", "-m", "start")

    def env(self, stub=None, **extra):
        env = {k: v for k, v in os.environ.items()
               if not k.startswith("KODACT_") and k not in ("NO_COLOR", "FORCE_COLOR", "CLICOLOR_FORCE", "TMUX", "STY")}
        env.update({"HOME": self.home, "KODACT_HOME": self.kodact_home, "TERM": "xterm-256color", "COLORTERM": "truecolor",
                    "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost", "LANG": "C.UTF-8",
                    "XDG_CONFIG_HOME": os.path.join(self.home, ".config"), "XDG_STATE_HOME": os.path.join(self.home, ".state"),
                    "XDG_CACHE_HOME": os.path.join(self.home, ".cache")})
        if stub is not None:
            env.update({"KODACT_OPENAI_BASE_URL": stub.url, "KODACT_OPENAI_API_KEY": stub.key or "stub-key-1234567890",
                        "KODACT_MODEL": stub.models[0], "KODACT_CONTEXT_WINDOW": str(stub.window or 200000)})
        for k, v in extra.items():
            if v is None:
                env.pop(k, None)
            else:
                env[k] = str(v)
        return env

    def state(self, *parts):
        return os.path.join(self.kodact_home, "state", *parts)

    def sessions(self):
        """Every session file in the store: [(path, [json lines])]."""
        out = []
        for base, _, files in os.walk(self.state()):
            for f in files:
                if f.endswith(".jsonl") and f != "history.jsonl":
                    p = os.path.join(base, f)
                    lines = []
                    for line in open(p, errors="replace"):
                        try:
                            lines.append(json.loads(line))
                        except ValueError:
                            pass
                    out.append((p, lines))
        return out

    def files(self):
        """Every file under the sandbox, relative, for 'nothing was written outside' checks."""
        out = set()
        for base, _, files in os.walk(self.root):
            for f in files:
                out.add(os.path.relpath(os.path.join(base, f), self.root))
        return out

    def close(self):
        shutil.rmtree(self.root, ignore_errors=True)


# --------------------------------------------------------------------------
# The terminal

MODES = {
    "alt_screen": (rb"\x1b\[\?1049h", rb"\x1b\[\?1049l"),
    "mouse": (rb"\x1b\[\?100[0236]h", rb"\x1b\[\?100[0236]l"),
    "bracketed_paste": (rb"\x1b\[\?2004h", rb"\x1b\[\?2004l"),
    "cursor_hidden": (rb"\x1b\[\?25l", rb"\x1b\[\?25h"),
    "keyboard_protocol": (rb"\x1b\[>\d*u", rb"\x1b\[<\d*u"),
    "focus_events": (rb"\x1b\[\?1004h", rb"\x1b\[\?1004l"),
}


class Term:
    """`kodact` in a pseudo-terminal, read through a terminal emulator."""

    def __init__(self, args, env, cwd, cols=110, rows=36, binary=None, keyboard_protocol=False):
        self.cols, self.rows = cols, rows
        # Answer the keyboard-protocol query as kitty, foot or WezTerm do, so Shift+Enter is its own key.
        self.keyboard_protocol = keyboard_protocol
        self.raw = b""
        self.screen = pyte.HistoryScreen(cols, rows, history=20000, ratio=0.5)
        self.screen.write_process_input = self._answer
        self.stream = pyte.ByteStream(self.screen)
        binary = binary or os.environ.get("KODACT_BIN") or os.path.join(ROOT, "target", "debug", "kodact")
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
            os.chdir(cwd)
            os.execve(binary, [binary, *args], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.cooked = termios.tcgetattr(self.fd)
        self.status = None

    def _answer(self, data):
        try:
            os.write(self.fd, data.encode())
        except OSError:
            pass

    # reading -----------------------------------------------------------

    def pump(self, seconds=0.05):
        """Read what the program wrote for `seconds`."""
        end = time.time() + seconds
        while True:
            left = end - time.time()
            r, _, _ = select.select([self.fd], [], [], max(left, 0))
            if r:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    data = b""
                if not data:
                    self._reap()
                    return
                self.raw += data
                if self.keyboard_protocol and b"\x1b[?u" in data:
                    self._answer("\x1b[?0u")
                self.stream.feed(data)
            if left <= 0:
                return

    def _reap(self, wait=0.0):
        end = time.time() + wait
        while self.status is None:
            try:
                pid, st = os.waitpid(self.pid, os.WNOHANG)
            except ChildProcessError:
                self.status = -1
                return
            if pid:
                self.status = os.waitstatus_to_exitcode(st)
            elif time.time() >= end:
                return
            else:
                time.sleep(0.02)

    def lines(self):
        return [l.rstrip() for l in self.screen.display]

    def text(self):
        return "\n".join(self.lines())

    def scrollback(self):
        """What scrolled off the top, plus the screen: the terminal's whole buffer."""
        top = ["".join(c.data for _, c in sorted(row.items())).rstrip() for row in self.screen.history.top]
        return "\n".join(top + self.lines())

    def cursor(self):
        return self.screen.cursor.x, self.screen.cursor.y

    def cell(self, x, y):
        """The character drawn at a cell ('' for the second half of a wide one)."""
        if 0 <= y < self.rows and 0 <= x < self.cols:
            return self.screen.buffer[y][x].data
        return None

    def input_rows(self):
        """The rows of the prompt's text, as drawn: from the row with the prompt mark to the box's end."""
        lines = self.lines()
        start = max((i for i, l in enumerate(lines) if re.match(r"^\s*[│]?\s*[❯›>] ", l) or re.match(r"^\s*[│]?\s*[❯›>]$", l)), default=None)
        if start is None:
            return []
        out = []
        for l in lines[start:]:
            body = l.strip()
            if body.startswith(("╰", "└", "─")) or (out and not body.strip("│ ")):
                break
            body = re.sub(r"^│ ?", "", body)
            body = re.sub(r" ?│$", "", body)
            out.append(re.sub(r"^[❯›>] ?", "", body.rstrip()) if not out else body.strip())
        return out

    PLACEHOLDERS = ("What shall we build", "Reply…", "Type to queue", "Try again, or /model", "Refine the plan", "Try \"")

    def input_text(self):
        """What is typed in the prompt ('' when it shows only its placeholder)."""
        text = "\n".join(self.input_rows())
        return "" if text.startswith(self.PLACEHOLDERS) else text

    def styles(self):
        """Every cell's colours and weight, to see a highlight that moved when the text did not."""
        return [[(c.fg, c.bg, c.bold, c.reverse) for _, c in sorted(row.items())] for _, row in sorted(self.screen.buffer.items())]

    def lands(self, mark="Z"):
        """Type one character and say whether it was drawn where the cursor stood: 'the cursor
        is where the next character lands'. Returns (ok, detail). The character stays typed."""
        self.settle(quiet=0.15)
        x, y = self.cursor()
        os.write(self.fd, mark.encode())
        self.settle(quiet=0.15)
        x2, y2 = self.cursor()
        if self.cell(x, y) == mark:
            return True, ""
        # The box grew by a row (the text wrapped): the mark is just before the cursor now.
        if x2 > 0 and self.cell(x2 - 1, y2) == mark:
            return True, ""
        return False, f"cursor was at {(x, y)}, is at {(x2, y2)}; no {mark!r} there\n{self.text()}"

    def wait(self, what, timeout=10.0, gone=False):
        """Wait until `what` (text, regex or predicate over the screen text) holds. Returns the screen."""
        end = time.time() + timeout
        while True:
            self.pump(0.05)
            t = self.text()
            if callable(what):
                ok = what(t)
            elif isinstance(what, re.Pattern):
                ok = bool(what.search(t))
            else:
                ok = what in t
            if ok != gone:
                return t
            if time.time() > end or self.status is not None:
                raise Timeout(f"{'still' if gone else 'never'} on screen: {what!r}\n--- screen ---\n{t}\n---")

    def settle(self, quiet=0.25, timeout=8.0):
        """Wait until the screen stops changing."""
        end = time.time() + timeout
        last, since = None, time.time()
        while time.time() < end and self.status is None:
            self.pump(0.05)
            t = self.text()
            if t != last:
                last, since = t, time.time()
            elif time.time() - since >= quiet:
                break
        return self.text()

    # writing -----------------------------------------------------------

    def send(self, keys, settle=True):
        os.write(self.fd, keys.encode() if isinstance(keys, str) else keys)
        if settle:
            return self.settle()

    def type(self, text, enter=True, settle=True):
        """Type text key by key (a person, not a paste), then Enter."""
        for ch in text:
            os.write(self.fd, ch.encode())
            self.pump(0.004)
        if enter:
            self.pump(0.1)
            os.write(self.fd, b"\r")
        if settle:
            return self.settle()

    def resize(self, cols, rows):
        self.cols, self.rows = cols, rows
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.screen.resize(rows, cols)
        os.kill(self.pid, signal.SIGWINCH)
        return self.settle()

    # the end -----------------------------------------------------------

    def modes(self, raw=None):
        """Which terminal modes the program has left on, from every escape it wrote."""
        raw = self.raw if raw is None else raw
        on = {}
        for name, (set_, reset) in MODES.items():
            s = max((m.end() for m in re.finditer(set_, raw)), default=-1)
            r = max((m.end() for m in re.finditer(reset, raw)), default=-1)
            on[name] = s > r
        return on

    def after_alt_screen(self):
        """What was printed after the program left the alternate screen (what stays in the scrollback)."""
        m = list(re.finditer(MODES["alt_screen"][1], self.raw))
        tail = self.raw[m[-1].end():] if m else self.raw
        s = pyte.HistoryScreen(self.cols, 2000, history=1)
        pyte.ByteStream(s).feed(tail)
        return "\n".join(l.rstrip() for l in s.display).rstrip()

    def tty_restored(self):
        """Is the terminal back in the mode the shell had it (echo, canonical input, signals)?"""
        now = termios.tcgetattr(self.fd)
        return now[3] & (termios.ECHO | termios.ICANON | termios.ISIG) == \
            self.cooked[3] & (termios.ECHO | termios.ICANON | termios.ISIG)

    def wait_exit(self, timeout=10.0):
        end = time.time() + timeout
        while self.status is None and time.time() < end:
            self.pump(0.05)
            self._reap()
        # Read what is left before the terminal closes.
        if self.status is not None:
            try:
                while select.select([self.fd], [], [], 0.05)[0]:
                    data = os.read(self.fd, 65536)
                    if not data:
                        break
                    self.raw += data
                    self.stream.feed(data)
            except OSError:
                pass
        return self.status

    def left_clean(self):
        """The faults a person meets after the program is gone: [] when the terminal is as it was."""
        bad = [name for name, on in self.modes().items() if on]
        try:
            if not self.tty_restored():
                bad.append("raw mode")
        except termios.error:
            pass
        return bad

    def kill(self):
        if self.status is None:
            try:
                os.kill(self.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self._reap(2.0)
        try:
            os.close(self.fd)
        except OSError:
            pass


def colours(raw):
    """Which kinds of colour the program wrote: (24-bit, 256-colour, the 16 ANSI colours)."""
    true = c256 = c16 = False
    for m in re.finditer(rb"\x1b\[([0-9;:]*)m", raw):
        p = [int(x) if x else 0 for x in re.split(rb"[;:]", m.group(1))]
        i = 0
        while i < len(p):
            if p[i] in (38, 48, 58) and i + 1 < len(p):
                if p[i + 1] == 2:
                    true, i = True, i + 5
                    continue
                if p[i + 1] == 5:
                    # Indexes 0 to 15 are the 16 ANSI colours, written the long way.
                    if i + 2 < len(p) and p[i + 2] < 16:
                        c16 = True
                    else:
                        c256 = True
                    i += 3
                    continue
            if 30 <= p[i] <= 37 or 40 <= p[i] <= 47 or 90 <= p[i] <= 97 or 100 <= p[i] <= 107:
                c16 = True
            i += 1
    return true, c256, c16


class Timeout(AssertionError):
    pass


def builtins():
    """Every built-in command and its aliases, read from the registry's source: [(name, [aliases], args, tui_only, immediate)]."""
    src = open(os.path.join(ROOT, "crates", "kodact-core", "src", "commands", "mod.rs")).read()
    body = src[src.index("builtins! {\n    ("):]
    body = body[:body.index("\n}\n")]
    out = []
    for m in re.finditer(r'\(\s*(\w+),\s*"([^"]+)",\s*\[([^\]]*)\],\s*"([^"]*)",\s*"[^"]*"\s*(?:,\s*(Surfaces::\w+),\s*(true|false)\s*)?\)', body):
        aliases = re.findall(r'"([^"]+)"', m.group(3))
        out.append((m.group(2), aliases, m.group(4), m.group(5) == "Surfaces::TUI", m.group(6) == "true"))
    return out
