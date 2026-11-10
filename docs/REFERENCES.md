# Reference CLIs: what each one teaches KodAct

Each project was read from its official repository or documentation on
2026-10-06, and each pattern is credited to it.
- **Benchmark numbers are claims, not facts.** None was reproduced here.
- **Sources this environment couldn't reach are marked as such:** arXiv, the
  vendor blogs and the hosted docs sites are blocked by the network proxy.
- **Nothing is copied.** Patterns are reimplemented from their description.

| Reference | Source checked | Patterns KodAct adopts | Status in KodAct |
| --- | --- | --- | --- |
| Claude Code | Public `--help`, SDK protocol | Tool loop, permission modes, hooks, stream-json, sessions, sub-agents | Baseline; built |
| OpenAI Codex CLI (Apache-2.0) | `openai/codex` source: `protocol/config_types.rs`, `protocol.rs`, `linux-sandbox/README.md`, `exec/` | **An OS sandbox, separate from approvals**: `read-only`, `workspace-write`, `danger-full-access`; network off by default. On Linux, bubblewrap mounts `/` read-only and binds only the writable roots, with `no_new_privs` and a seccomp network filter. On macOS, Seatbelt. Approval policies: untrusted, on-request, never. `exec --json` emits thread, turn and item lifecycle events | Sandbox: built (`--sandbox`; see `docs/CLI.md`). Its JSONL event names are noted; KodAct keeps stream-json |
| OpenCode (MIT) | `sst/opencode` docs (permissions, LSP, formatters, config, CLI) | Per-tool allow/ask/deny with wildcards (KodAct already has this); `--auto` approves anything not explicitly denied; **LSP diagnostics after an edit, fed back to the agent**; formatters run after edits; JSONC config; build and plan agents switched with Tab; `run` subcommand | LSP/formatter feedback is planned under the verification pillar; `--auto` maps to bypass, which keeps deny rules |
| Goose (Apache-2.0, Rust) | `block/goose` docs (agents, tools, security, recipes, multi-model) | **A vendor-neutral `.agents/agents/` directory** for agents; **a pattern-based scanner for prompt injection and dangerous commands** that pauses risky tool calls; **recipes**: YAML with parameters, prompt and response schema for headless runs; concise or detailed tool output; MCP-first extensions | `.agents/` directories, `AGENTS.md` and a threat scanner (KodAct's own patterns) plus injection markers: built. Recipes and `.agents/checks` review agents: planned |
| Cline (Apache-2.0) | `cline/cline` README | Explicit Plan and Act phases, approval for each action, checkpoints, a headless CI mode, scheduled tasks | Plan mode and checkpoints exist; Act/Plan maps to the permission modes |
| R-CLI (Backboard) | Search results only (backboard.io blocked) | Claims automatic reasoning control (effort adapted per task) and 84–91% on Terminal-Bench 2.1. Unverified | Adaptive effort is an eval hypothesis, not built |
| "Crux" / harness-design study | arXiv 2609.20804, via search abstracts (arXiv blocked) | Rule-based elision before LLM summarization was the most efficient context strategy; making elided content recoverable added machinery models rarely used | Supports KodAct's C9 order (micro-compaction, then summary). Recoverable elision is deprioritized |
| Villani Code | Search results only (CDN blocked) | Runtime for small local models: verification-driven loops, robustness to messy terminals. Claims +46% relative to the baseline on the same 9B model. Unverified | Informs the verification and recovery pillars |
| KISS Sorcar | **Not found.** No repository or paper matched | – | Skipped |
| ClawCodex | PyPI page | Describes itself as *ported from the reference CLI's TypeScript implementation*, i.e. derived from leaked source | **Excluded** on provenance grounds |
| oh-my-pi (MIT) | `can1357/oh-my-pi` source, read 2026-11-10: `packages/agent`, `crates/pi-edit`, `crates/pi-iso`, `docs/` | **Edits that name lines** under a content tag instead of retyping text, refused when stale; **language-server diagnostics on every write**; **sub-agents isolated** in copy-on-write copies of the tree | Line-range Edit with an anchor line (`edit.lineRanges`), language-server errors after edits (`verification.languageServer`) and worktree isolation for Task agents: built, each KodAct's own design, off or opt-in until measured |
| DeepSeek Harness (MIT) | `deepseek-ai/deepseek-harness` source, read 2026-11-10: `packages/core/agent-loop`, docs | **A durable session log** in which an interrupted turn is closed on resume | Turn start and end records, and a resume that tells the model what a killed turn left unanswered: built |
| Terminal-Bench | Search results (tbench.ai not fetched) | Tasks run in a real terminal and are judged by a hidden verification script | `kodact-eval` follows the same design (hidden checks); a Terminal-Bench adapter is planned |

## Rules for comparisons

Any claim that KodAct beats another CLI must hold these fixed and record
them in the `kodact-eval` report:
- the model and its version, reasoning effort and context limit;
- the system prompt and tool descriptions;
- turn limits, timeouts and retries;
- compaction, recovery and cancellation settings;
- the task subset and the number of independent runs;
- cost accounting, and a verification method independent of the agent.

So far no comparison has been run: this environment has no API key.
