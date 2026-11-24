"""Tests for ante_stream.py and report.py on Ante's JSON events, run without Harbor or Ante.

    python3 -m pytest evals/harbor/test_ante_stream.py

The Ante job of 2026-11-24 (write-compressor, deep-thinking) timed out after 15 minutes with
three lines of text and no turns, tokens or tool calls in its report: Ante ran without its JSON
output. These check that its events now count as KodAct's do, and that a run stopped mid-reply
still shows what it was writing.
"""

import json

import report
from ante_stream import AnteStream, result_text


def ev(name, data=None):
    return {"timestamp": "2026-11-24T15:50:00Z", "event": {name: data} if data is not None else name}


RUN = [
    ev("SessionStart", {"session_id": "s", "provider": "openai-compatible"}),
    ev("TurnStart", {}),
    ev("Thinking", "Look at the decompressor first."),
    ev("AgentMessage", "I'll inspect the decompressor."),
    ev("ToolStart", {"id": "t1", "name": "bash", "args": {"command": "cat /app/decomp.c"}}),
    ev("UsageUpdate", {"usage": {"input_tokens": 1000, "output_tokens": 200, "cache_read_tokens": 50}}),
    ev("ToolEnd", {"tool_use_id": "t1", "status": "Completed", "result_json": {"output": "int main..."}}),
    ev("ToolStart", {"id": "t2", "name": "bash", "args": {"command": "python3 -V"}}),
    ev("UsageUpdate", {"usage": {"input_tokens": 1500, "output_tokens": 100}}),
    ev("ToolEnd", {"tool_use_id": "t2", "status": "Failed", "result_json": {"error": "python3: not found"}}),
    ev("AgentMessage", "Done: data.comp is 2,301 bytes."),
    ev("UsageUpdate", {"usage": {"input_tokens": 2000, "output_tokens": 30}}),
    ev("TurnEnd", {"status": "Completed", "steps": 3}),
]


def feed(lines):
    s, out = AnteStream(), []
    for line in lines:
        out += s.feed(line)
    return s, out + s.flush()


def test_a_finished_run_counts_turns_tokens_and_tool_calls():
    s, out = feed(RUN)
    kinds = [e["type"] for e in out]
    assert kinds == ["assistant", "user", "assistant", "user", "assistant", "result"]
    assert out[0]["message"]["content"][2] == {
        "type": "tool_use", "id": "t1", "name": "bash", "input": {"command": "cat /app/decomp.c"}
    }
    assert out[1]["message"]["content"][0]["is_error"] is False
    assert out[3]["message"]["content"][0]["is_error"] is True
    result = out[-1]
    assert result["subtype"] == "success" and result["num_turns"] == 3
    assert result["result"] == "Done: data.comp is 2,301 bytes."
    assert result["usage"]["input_tokens"] == 4500 and result["usage"]["output_tokens"] == 330
    assert result["usage"]["cache_read_input_tokens"] == 50


def test_a_failed_turn_says_why():
    _, out = feed([ev("AgentMessage", "hi"), ev("TurnEnd", {"status": {"Error": {"kind": "Api", "headline": "HTTP 502"}}})])
    assert out[-1]["is_error"] and out[-1]["errors"] == ["Api — HTTP 502"]


def test_a_run_stopped_mid_reply_keeps_what_it_was_writing():
    lines = RUN[:3] + [ev("MessageDelta", "The decompressor is an adaptive "), ev("MessageDelta", "arithmetic coder.")]
    _, out = feed(lines)
    assert [e["type"] for e in out] == ["assistant"]
    texts = [b.get("text") or b.get("thinking") for b in out[0]["message"]["content"]]
    assert texts == ["Look at the decompressor first.", "The decompressor is an adaptive arithmetic coder."]


def test_the_report_reads_an_ante_trial(tmp_path):
    d = tmp_path / "write-compressor__X"
    (d / "agent").mkdir(parents=True)
    (d / "verifier").mkdir()
    (d / "agent" / "ante.jsonl").write_text("\n".join(json.dumps(e) for e in RUN) + "\nnot json\n")
    (d / "agent" / "ante.stderr.txt").write_text("")
    (d / "verifier" / "reward.txt").write_text("1")
    t = report.Trial(d)
    assert t.kodact  # a structured stream, read as KodAct's
    assert t.turns == 3
    assert t.tools["bash"] == 2 and t.tool_errors["bash"] == 1
    assert t.tokens_out == 330
    assert t.final_text == "Done: data.comp is 2,301 bytes."
    assert any("→ bash" in line for line in t.conversation())


def test_a_command_result_reads_as_its_output():
    # As the 2026-11-24 job's watch showed it: the outcome wrapped around the command's result.
    wrapped = {"Completed": {"exit_code": 127, "stderr": "bash: python3: command not found\n", "stdout": "In information theory\n"}}
    assert result_text(wrapped) == "In information theory\nbash: python3: command not found\n(exit 127)"
    assert result_text({"Completed": {"exit_code": 0, "stdout": "4868 /app/data.txt\n"}}) == "4868 /app/data.txt"
    assert result_text("plain") == "plain"
