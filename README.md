# KodAct

KodAct is an agentic coding CLI written in Rust: an agent loop, a tool
system, permission modes, MCP, context compaction, hooks, a stream-json
protocol for IDE hosts, and a terminal UI.

KodAct is written from public interface documentation only, and it ships
its own system prompt and tool descriptions.

## Goal

`docs/GOALS.md` defines what KodAct optimizes: the best measured task success
per model, cost and time. The target is a top-3 harness on Terminal-Bench, as
the proof of the real aim: the best everyday CLI for a person's daily and
complex problems. Every feature is built against it and measured with
`kodact-eval` (see `KODACT.md`).

## Status

Under construction. `docs/PARITY.md` tracks every tool, flag, protocol message
and command against the reference CLI, with the test that proves each one. `docs/ARCHITECTURE.md` holds
the crate layout and the behavioural contracts (C1–C11).

## Install

From crates.io (the package is `kodact-cli`, the command is `kodact`):

```bash
cargo install kodact-cli
kodact --help
```

The evaluation harness installs the same way: `cargo install kodact-eval`.

## Build

```bash
cargo build --release
./target/release/kodact --help
```

On a new Debian or Ubuntu machine, WSL included, `scripts/setup-debian.sh`
installs what the build, the tests and the Harbor runs need. It also makes
every tool trust the certificate authority of a company proxy that re-signs
HTTPS, taken from Windows' own trust store under WSL. Run it again at any
time: it only adds what is missing and never changes a setting already there,
yours or its own. `--check` only reports.

## Credentials

| Variable | Used for |
| --- | --- |
| `KODACT_API_KEY` | API key, sent as `x-api-key` |
| `KODACT_AUTH_TOKEN` | Bearer token, for gateways and proxies |
| `KODACT_BASE_URL` | A different Messages API endpoint |
| `KODACT_CUSTOM_HEADERS` | Extra request headers, one `Name: value` per line |
| `KODACT_OPENAI_BASE_URL`, `KODACT_OPENAI_API_KEY` | An OpenAI-compatible endpoint |
| `KODACT_GEMINI_API_KEY`, `KODACT_GEMINI_BASE_URL` | Google's Gemini API (Google AI Studio by default; Vertex AI or a gateway by URL) |

## Optional external prompt set

Point `KODACT_PROMPTS_DIR` at a directory containing:
- `system/*.md`, which replace the built-in system prompt sections;
- `tools/<ToolName>.md`, which replace individual tool descriptions.

Keep such a directory out of this repository.
