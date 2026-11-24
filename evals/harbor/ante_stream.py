"""Ante's JSON events (`ante --output-format json`), as the stream-json events KodAct writes.

ante_agent.py saves Ante's events in `agent/ante.jsonl`. report.py and watch.py read KodAct's
`kodact.jsonl` (assistant, user, system and result events); this module turns Ante's events into
the same shape, as pi_stream.py does for Pi, so turns, tokens, tool calls and errors are counted
and shown the same way for every harness. Tool names stay Ante's own.

Each line of Ante's stream is `{"timestamp": ..., "event": {"<Name>": <payload>}}`. The names and
fields read here are the ones Ante's own Harbor adapter reads (ante-harbor/ante_events.py):
`Thinking` and `AgentMessage` (text), their `ThinkingDelta` / `MessageDelta` chunks, `ToolStart`
(`id`, `name`, `args`), `ToolEnd` (`tool_use_id`, `status`, "Completed" when it worked,
`result_json`), `UsageUpdate` (`usage`, one model call each: summed), `CompactEnd`, `Error`, and
`TurnEnd` (`status`, `steps`), which only the root turn sends.

A run Harbor stopped at the time limit has no `TurnEnd`: `flush()` returns what the model had
written so far (its streamed chunks included), so the report shows where the time went.

    stream = AnteStream()
    for line in lines:
        for event in stream.feed(json.loads(line)):
            ...
    tail = stream.flush()
"""

import json

OK = "Completed"


def _text(value) -> str:
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    return json.dumps(value, ensure_ascii=False)


class AnteStream:
    """Feed Ante's events in order; each call returns the stream-json events they amount to."""

    def __init__(self):
        self.turns = 0
        self.tokens = {"input_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0, "output_tokens": 0}
        self.content: list[dict] = []  # the model's reply being assembled
        self.usage: dict = {}
        self.think = ""  # streamed chunks not yet followed by the whole block
        self.say = ""
        self.last_text = ""
        self.errors: list[str] = []
        self.ended = False

    def feed(self, line: dict) -> list[dict]:
        ev = line.get("event")
        if isinstance(ev, str):
            name, data = ev, None
        elif isinstance(ev, dict) and len(ev) == 1:
            name, data = next(iter(ev.items()))
        else:
            return []
        out: list[dict] = []
        if name == "ThinkingDelta":
            self.think += _text(data)
        elif name == "MessageDelta":
            self.say += _text(data)
        elif name == "Thinking":
            self.think = ""
            if _text(data).strip():
                self.content.append({"type": "thinking", "thinking": _text(data)})
        elif name == "AgentMessage":
            self.say = ""
            if _text(data).strip():
                self.content.append({"type": "text", "text": _text(data)})
                self.last_text = _text(data)
        elif name == "ToolStart" and isinstance(data, dict):
            self._chunks()
            args = data.get("args")
            self.content.append({
                "type": "tool_use",
                "id": data.get("id"),
                "name": data.get("name") or "?",
                "input": args if isinstance(args, dict) else {"value": args},
            })
        elif name == "ToolEnd" and isinstance(data, dict):
            out += self._reply()
            status = data.get("status")
            block = {
                "type": "tool_result",
                "tool_use_id": data.get("tool_use_id"),
                "content": _text(data.get("result_json")),
                "is_error": status != OK,
            }
            out.append({"type": "user", "message": {"role": "user", "content": [block]}})
        elif name == "UsageUpdate" and isinstance(data, dict):
            usage = data.get("usage") or {}
            step = {
                "input_tokens": usage.get("input_tokens") or 0,
                "cache_read_input_tokens": usage.get("cache_read_tokens") or 0,
                "cache_creation_input_tokens": usage.get("cache_write_tokens") or usage.get("cache_creation_tokens") or 0,
                "output_tokens": usage.get("output_tokens") or 0,
            }
            for k, v in step.items():
                self.tokens[k] += v
                self.usage[k] = self.usage.get(k, 0) + v
        elif name == "CompactEnd":
            out.append({"type": "system", "subtype": "compaction", "detail": data})
        elif name == "Error":
            text = _text(data)
            self.errors.append(text)
            out.append({"type": "system", "subtype": "error", "error": text})
        elif name == "TurnEnd":
            out += self._reply()
            out.append(self._result(data if isinstance(data, dict) else {}))
        return out

    def flush(self) -> list[dict]:
        """The end of the log: the reply still being written, and no result (the run was stopped)."""
        return [] if self.ended else self._reply()

    def _chunks(self):
        """Streamed chunks whose whole block never came: keep them as the block."""
        if self.think.strip():
            self.content.append({"type": "thinking", "thinking": self.think})
        if self.say.strip():
            self.content.append({"type": "text", "text": self.say})
            self.last_text = self.say
        self.think = self.say = ""

    def _reply(self) -> list[dict]:
        self._chunks()
        if not self.content:
            return []
        self.turns += 1
        message = {"role": "assistant", "content": self.content, "usage": self.usage}
        self.content, self.usage = [], {}
        return [{"type": "assistant", "message": message}]

    def _result(self, data: dict) -> dict:
        self.ended = True
        status = data.get("status")
        error = status.get("Error") if isinstance(status, dict) else None
        if isinstance(error, dict):
            text = " — ".join(str(error.get(k)) for k in ("kind", "headline", "details") if error.get(k))
        else:
            text = _text(error) if error else ""
        steps = data.get("steps")
        return {
            "type": "result",
            "subtype": "error_during_execution" if error else "success",
            "is_error": bool(error),
            "num_turns": steps if isinstance(steps, int) and steps >= 0 else self.turns,
            "stop_reason": "error" if error else "end_turn",
            "errors": [text] if text else [],
            "result": self.last_text,
            "usage": dict(self.tokens),
        }
