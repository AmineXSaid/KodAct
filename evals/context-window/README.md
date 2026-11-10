# Context window discovery, end to end

Runs the real `kodact` binary against a stand-in OpenAI-compatible endpoint
(`server.py`) whose behaviour each case sets, and checks the context window
KodAct uses: what `/context` shows, the startup notice, how long startup
waits, the `max_tokens` of each request, and when compaction runs.

```bash
cargo build -p kodact-cli
python3 evals/context-window/run.py            # every case
python3 evals/context-window/run.py latency    # the cases whose name contains "latency"
```

Needs `tmux` for the terminal-UI cases. `KODACT_BIN` picks another binary,
`KODACT_CTX_PORT` another port (default 18090).

The cases: each field name gateways use for the window; nothing reported;
a model list that answers after 0.3 s to 30 s; a list that fails (401, 404,
500, bad JSON, HTML, empty, refused); a model missing from the list; 5,000
models; odd values (0, -1, text, decimals, 10M, 1,000); who wins
(`KODACT_CONTEXT_WINDOW`, `modelLimits`, a known model; a value set for a
small model while the endpoint lists a large one); LiteLLM's `/model/info`; a refusal
that states the window, learned and sent again (and a large model never refused); the reply limit;
compaction under and over the threshold on a 32k window; a server that
rejects a too-long prompt.
