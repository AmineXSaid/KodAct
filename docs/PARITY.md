# Parity ledger

Parity is the compatibility baseline, not the goal. `docs/GOALS.md` sets what
KodAct optimizes and the order work is done in. Read it before picking up a
row here.

The target is the reference CLI, version **2.1.290**.

**Rule:** a row is `done` only when the **Test** column names a test that
exists and passes. A milestone is done only when every row it claims is
`done` or `out`.

Test names are `<crate or file>::<test>`; `e2e` is `crates/kodact-cli/tests/e2e.rs`.

**Status values:**

| Status | Meaning |
| --- | --- |
| `done` | Implemented and tested |
| `partial` | Implemented, but with a documented gap |
| `todo` | Not started |
| `out` | Deliberately left out; the reason is given |

**Wire-shape column (stream-json rows only):**

| Value | Meaning |
| --- | --- |
| `inferred` | Built from public documentation and KodAct's TypeScript consumer, not observed from the real binary |
| `observed` | Checked against a captured fixture in `docs/fixtures/` |

## Protocol (M2)

| Item | Status | Wire shape | Test | Notes |
| --- | --- | --- | --- | --- |
| `system/init` | done | inferred | `e2e::stream_json_golden_basic` |  |
| `assistant` message | done | inferred | `e2e::stream_json_golden_basic` |  |
| `user` message (tool results, `tool_use_result`) | done | inferred | `e2e::host_permission_prompt_allow_and_deny` |  |
| `stream_event` (`--include-partial-messages`) | done | inferred | `e2e::host_controls_mode_model_and_interrupt` |  |
| `result` success / `error_max_turns` / `error_max_budget_usd` / `error_during_execution` | done | inferred | `e2e::max_turns_and_continue`, `e2e::print_errors`, `engine::c7_budget_and_max_turns_stop_the_run` |  |
| `--input-format stream-json` user messages | done | inferred | `e2e::host_permission_prompt_allow_and_deny` |  |
| `--replay-user-messages` (`isReplay`) | done | inferred | `e2e::replay_uuid_drives_rewind_files` |  |
| `control_request` `can_use_tool` (CLI → host) | done | inferred | `e2e::host_permission_prompt_allow_and_deny` | Contract C1 |
| `control_request` `initialize` | partial | inferred | `e2e::host_permission_prompt_allow_and_deny` | `systemPrompt` / `appendSystemPrompt` are applied; SDK callback hooks and SDK MCP servers in `initialize` are not supported yet |
| `control_request` `interrupt` | done | inferred | `e2e::host_controls_mode_model_and_interrupt` | Contract C3 |
| `control_request` `stop_task` (KodAct extension) | done | inferred | `e2e::host_stops_one_agent_and_the_turn_goes_on`, `engine::tests::agent_registry_stops_one_or_all` | `{task_id}` or `{all: true}` → `{stopped: n}`; stops Task agents without ending the turn; their results begin `Stopped by user` |
| `control_request` `set_permission_mode` | done | inferred | `e2e::host_controls_mode_model_and_interrupt`, `core::driver_tests::bypass_needs_the_launch_flag_and_no_managed_ban` | `bypassPermissions` only when launched in it or with `--allow-dangerously-skip-permissions`, and never when managed settings disable it |
| `control_request` `set_model` | done | inferred | `e2e::host_controls_mode_model_and_interrupt` |  |
| `control_request` `set_max_thinking_tokens` | done | inferred | `e2e::host_sets_thinking_tokens` |  |
| `control_request` `mcp_status` | done | inferred | `mcp::session_uses_mcp_tools_and_reports_status` | `{mcpServers: [{name, status, scope, serverInfo, tools, error}]}` |
| `control_request` `get_context_usage` | done | kodact | `kodact-cli::e2e::host_reads_what_fills_the_context` | KodAct's own: `/context`'s numbers as JSON (model, window, total, free, measured, autocompactAt, categories, tools, memoryFiles, skills, tips), answered from the view, mid-turn too |
| `control_request` `mcp_reconnect` | done | inferred | `client::servers_turn_off_on_and_reconnect` | `{serverName}`; answered when the restart ends (success with no body, or an error saying why), while other control requests keep being answered |
| `control_request` `mcp_toggle` | done | inferred | `client::servers_turn_off_on_and_reconnect` | `{serverName, enabled}`; the same as `/mcp enable` / `disable`, saved the same way |
| `control_request` `rewind_files` | done | inferred | `e2e::replay_uuid_drives_rewind_files` | Contract C4 |
| `system/compact_boundary` | done | inferred | `engine::c9_auto_triggers_at_threshold` | `compact_metadata: {trigger, pre_tokens}` |
| `system/model_fallback` | done | inferred | `engine::c6_fallback_on_overload_for_this_turn_only` | Contract C6 |

## Providers: Gemini and gateways

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Gemini 3 thought signatures on the OpenAI-compatible wire | done | `openai::gemini_tests::google_s_signatures_go_back_on_the_calls_they_came_with` | A tool call's `extra_content` (Google's `extra_content.google.thought_signature`), any other key it carries, and its function's extra keys are kept on the block (`provider`, in transcripts) and sent back on that call unchanged. Parallel calls at one `index` with their own ids stay apart. Tool messages carry `name` |
| Gateways' signatures and reasoning (OmniRoute) | done | `openai::gemini_tests::a_gateway_s_message_signature_and_reasoning_go_back_too`, `messages::provider_data_tests::thinking_signatures_and_tool_ids_go_back_unchanged` | A message-level `thinking_signature`/`thought_signature` rides on the turn's first call and goes back on the message; a call's `thought_signature` goes back on it; tool-call ids go back exactly as issued (OmniRoute keys its signature store by them); Gemini models get their `reasoning_content` back. On the Messages wire, thinking signatures and ids go back unchanged and `provider` is never sent |
| Gemini's request rules on the OpenAI-compatible wire | done | `gemini_rules::tests::*`, `openai::gemini_tests::gemini_gets_names_schemas_and_effort_it_takes`, `openai::gemini_tests::thoughts_in_content_tags_become_thinking` | Gemini models (any gateway prefix): tool names over 64 characters or with other characters get a stable alias, mapped back; schemas cut to what Gemini takes (no `$schema`/`$ref`/`additionalProperties`/`default`/`examples`, `const`→`enum`, `["T","null"]`→`nullable`, `oneOf`→`anyOf`, no parameters for an empty object); effort as `reasoning_effort` (`minimal` on Flash, `xhigh`/`max`→`high`), dropped if refused; `<thought>` in content as thinking. No temperature unless set |
| Native Gemini API (`generateContent`) | done | `gemini::tests::*`, `endpoint::tests::a_gemini_key_alone_chooses_google_ai_studio`, `endpoint::tests::gemini_keys_stay_with_the_gemini_endpoint`, `endpoint::tests::a_project_cannot_choose_the_gemini_endpoint_or_helper` | `KODACT_GEMINI_API_KEY` (alone: Google AI Studio), `KODACT_GEMINI_BASE_URL`/`gemini.baseUrl` (Vertex AI, gateways), `gemini.apiKeyHelper`, `gemini.authHeader`. Streams `:streamGenerateContent?alt=sse`. Each `functionCall`'s `thoughtSignature` is kept and replayed on that part (first parallel call only, as Gemini sends it); thought parts are thinking with their signature; a text part's signature goes back on it; a call Gemini 3 never signed gets Google's placeholder (`skip_thought_signature_validator`) on the first call of its turn. `functionResponse` with id (when Gemini gave one) and name; PDFs and images as `inlineData`. Tools as `parametersJsonSchema`, falling back to the sanitized `parameters` if refused; refused thinking settings or temperature are dropped and remembered per model. Effort: `thinkingLevel` (3+, `includeThoughts`), `thinkingBudget` (2.5). Usage incl. cached and thought tokens; finish reasons mapped (safety → refusal with the reason; a malformed function call is asked again once with a note, then an error; a stream without a finish reason is a cut connection). Errors: Google's status as the kind, `RetryInfo.retryDelay` honoured, `API_KEY_INVALID` as an auth failure with a Gemini hint. `/model` lists `GET /models` (pages, `generateContent` only, limits recorded); `countTokens`. Built-in limits and prices for Gemini 2.5, 3 and 3.1. Live check (`real_gemini_tool_loop`, ignored) still owed: needs a key |
| Switching models with Gemini signatures in history | done | `tests::a_new_model_gets_the_whole_conversation_without_foreign_thinking`, `gemini::tests::a_call_gemini_3_never_signed_gets_the_placeholder_once_per_turn`, `openai::gemini_tests::a_call_gemini_3_never_signed_gets_the_placeholder` | `/model` drops the last model's provider data with its thinking (signatures are per model). A Gemini 3 request whose history has a call without a signature (another model's, an old session's) gets `skip_thought_signature_validator` on the first call of that turn: as `thoughtSignature` natively, as `extra_content.google.thought_signature` on the OpenAI-compatible wire |
| Gemini tool loop end to end | done | `kodact-cli/tests/gemini.rs` (`kodact_test_host::MockGemini`) | The `kodact` binary through parallel and sequential calls and a `--continue`d session, against an endpoint that refuses a lost or changed signature, a nameless `functionResponse`, an unpaired call, a tool name Gemini refuses, a schema keyword outside its subset and `thinkingBudget` on Gemini 4, with Google's error texts |

## Tools

| Tool | Status | Test | Notes |
| --- | --- | --- | --- |
| Bash | done | `builtin::bash_runs_and_keeps_cwd_inside_project`, `bash_timeout_moves_the_command_to_the_background`, `bash_interrupt_returns_interrupted` | persistent cwd, timeout, background. Differs: a command still running at its `timeout` moves to the background instead of being killed (C22) |
| BashOutput / KillShell (background shells) | done | `builtin::background_shell_output_and_kill`, `a_moved_command_can_be_stopped_with_its_children` | Differs: the model is told when a background shell exits (C22) |
| Read | partial | `builtin::read_numbers_lines_and_pages`, `notebook_edit_replace_insert_delete`, `openai::tests::a_tool_result_never_carries_base64_and_its_images_follow` | Text, images, notebooks done. PDFs are sent whole: `pages` is passed as a hint, not extracted. On an OpenAI-compatible endpoint (no document block) a PDF goes as its extracted text (`pdf_text`, lopdf with a stream-scan fallback), and images a tool returned follow in the next user message |
| Write | done | `builtin::write_creates_and_checkpoints` |  |
| Edit | done | `builtin::edit_requires_read_and_unique_match`, `edit_preserves_crlf_and_multiedit_is_atomic` |  |
| MultiEdit | done | `builtin::edit_preserves_crlf_and_multiedit_is_atomic` |  |
| NotebookEdit | done | `builtin::notebook_edit_replace_insert_delete` |  |
| Glob | done | `builtin::glob_and_grep` |  |
| Grep | done | `builtin::glob_and_grep` |  |
| LS | done | `builtin::ls_lists_tree` |  |
| TodoWrite | done | `builtin::todo_write_validates_and_stores` |  |
| Task / Agent | done | `agents::explore_agent_reports_back_in_its_own_context`, `agents::parallel_agents_run_concurrently` | Named `Task` |
| WebFetch | done | `builtin::web_fetch_converts_summarizes_and_reports_redirects`, `web::urls_are_upgraded_and_metadata_hosts_refused`, `html::converts_a_typical_page` | HTML to Markdown, small-model answer to `prompt`, cross-host redirects reported, metadata hosts blocked, 15 min cache, 10 MB cap |
| WebSearch | done | `builtin::web_fetch_converts_summarizes_and_reports_redirects`, `web::search_results_list_their_sources` | Through the provider's `web_search_20250305` server tool (`webSearch.toolType` overrides); not offered with OpenAI-compatible providers. No live test |
| ExitPlanMode / EnterPlanMode | done | `engine::c8_questions_and_plan_approval_with_a_person`, `engine::c8_headless_questions_and_plans_are_answered_by_contract` | Contract C8; approval ends plan mode, or sets the mode the approval names |
| AskUserQuestion | done | `engine::c8_questions_and_plan_approval_with_a_person`, `engine::c8_headless_questions_and_plans_are_answered_by_contract` | Contract C8; the line REPL numbers the options |
| Skill | todo | | |
| SlashCommand | todo | | |
| ListMcpResources / ReadMcpResource | todo | | |
| TaskCreate / TaskUpdate / TaskList / TaskGet | todo | | |
| EnterWorktree / ExitWorktree | todo | | |

## Engine

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Agent loop (tool use until `end_turn`) | done | `engine::tool_loop_reads_file`, `e2e::print_text_and_request_shape` |  |
| Parallel batches of read-only tools | done | `engine::c2_result_order_survives_concurrency` | Contract C2 |
| Serialized permission prompts | done | `engine::c2_prompts_are_serialized` | Contract C2 |
| Interrupt | done | `engine::c3_interrupt_during_stream`, `engine::c3_interrupt_aborts_running_and_pending_tools` | Contract C3 |
| Retries with backoff (429 / 5xx / 529) | done | `e2e::retries_transient_errors` |  |
| Fallback model | done | `engine::c6_fallback_on_overload_for_this_turn_only`, `engine::non_overload_errors_fail_the_turn` | Contract C6 |
| `--autonomous` (KodAct extension) | done | `engine::unattended_runs_attempt_before_giving_up`, `engine::unattended_runs_retry_failed_model_calls`, `engine::unattended_runs_finish_announced_steps_and_open_todos`, `e2e::autonomous_runs_have_no_question_tools_and_attempt_first` | Contract C21 |
| `--max-time` (KodAct extension) | done | `engine::the_model_is_told_the_time_limit_and_warned_near_the_end`, `e2e::print_tells_the_model_its_time_limit` | Stated up front, reminder at a fifth left; the host enforces it |
| `--max-turns` / `--max-budget-usd` | done | `engine::c7_unknown_model_budget_fails_closed`, `engine::c7_budget_and_max_turns_stop_the_run`, `e2e::max_turns_and_continue` | Contract C7 |
| Thinking / effort per model | done | `request::thinking_per_model` |  |
| Prompt-cache breakpoints | done | `engine::cache_breakpoints_on_system_tools_and_last_message` | System, last tool and last message |
| Micro-compaction | done | `compact::c9_micro_clears_only_eligible_results`, `engine::c9_micro_is_sticky_across_requests` | Contract C9 |
| Auto-compact / `/compact` | done | `engine::c9_auto_triggers_at_threshold`, `engine::manual_compact_passes_instructions`, `engine::prompt_too_long_compacts_and_retries` | Contract C9. `/compact` works in every mode |

## Permissions

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Modes: default (manual), acceptEdits, plan, dontAsk, bypassPermissions | done | `permissions::modes` |  |
| Mode: auto | partial | `permissions::modes` | Rule-based approximation, not a model classifier |
| Rule grammar: `Tool`, `Tool(spec)`, `Bash(prefix *)`, path globs, `WebFetch(domain:)`, `mcp__*` | done | `permissions::rule_parsing`, `path_rules`, `webfetch_domains_and_mcp`, `prefix_rule_respects_word_boundary` |  |
| Precedence deny > ask > allow | done | `permissions::deny_beats_everything_including_bypass`, `ask_rule_overrides_allow_rule`, `compound_deny_matches_any_part` |  |
| `--allowedTools` / `--disallowedTools` / `--tools` | done | `core::builds_from_settings_and_flags`, `e2e::headless_denies_and_reports` |  |
| `--add-dir` scoping | done | `permissions::read_only_inside_working_dirs_is_allowed` |  |
| Headless auto-deny | done | `engine::c1_headless_denies_and_continues`, `e2e::headless_denies_and_reports` | Contract C1 |

## Harness (beyond the baseline)

These rows go past the reference CLI. Each names its pillar from `docs/GOALS.md`.

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Verification loop (contract C12): detected checks, reminder before finishing on unchecked changes | done | `engine::verify_reminds_once_when_changes_are_unchecked`, `engine::verify_accepts_a_check_after_the_last_change`, `engine::verify_sees_shell_writes_through_git_and_failed_checks`, `verify::detects_checks_from_manifests`, `verify::recognizes_check_commands`, `eval::eval_measures_a_pass_and_a_false_finish` | Verification / false-finish rate. Eval task `invoice-rounding` |
| Edit "not found" hints: whitespace-only differences, line-number prefixes, closest window; ambiguous matches list lines (C13) | done | `builtin::edit_failures_point_at_the_nearest_text` | Recovery / turns after first error. Eval task `makefile-tabs` |
| Loop guard: repeated failure, no progress, oscillating edits, error streak (C13) | done | `stuck::repeated_failures_fire_once`, `stuck::same_result_three_times_is_no_progress`, `stuck::edits_that_undo_each_other`, `stuck::a_streak_of_different_failures`, `engine::loop_guard_reminds_after_repeated_failures` | Recovery |
| `max_tokens` recovery: cut-off tool calls answered, text continued, output cap raised (C13) | done | `engine::max_tokens_continues_text_and_answers_cut_off_calls`, `accumulate::invalid_tool_json_is_marked_truncated` | Recovery |
| Dangerous-command patterns force a prompt (C14, the Goose scanner pattern) | done | `threat::flags_dangerous_commands`, `threat::leaves_ordinary_commands_alone`, `permissions::flagged_commands_are_never_approved_automatically` | Safety (OWASP agent risks) |
| Injected instructions in tool output marked as data (C14) | done | `injection::spots_instructions_hidden_in_data`, `injection::ordinary_text_passes`, `engine::injected_instructions_in_tool_output_are_marked` | Safety (OWASP LLM01). Eval task `injected-docs` |
| `AGENTS.md` memory; `.agents/<kind>/` resource directories | done | `config::agents_md_when_no_forge_md`, `agents::vendor_neutral_agents_dir_is_read_and_forge_wins` | Compatibility with other agent CLIs' repos |
| Tool output budgets with full output saved and pointed to (C15) | done | `builtin::long_output_is_saved_in_full_and_pointed_to`, `permissions::read_dirs_are_readable_not_writable` | Context / tokens per turn. Eval task `log-needle` |
| No repeated reads of unchanged content (C15) | done | `builtin::repeated_reads_of_unchanged_content_are_not_resent` | Context. Structured result `file_unchanged` is inferred |
| Task list survives compaction and resume; budget reminder (C15) | done | `engine::the_plan_survives_compaction_and_resume`, `engine::the_model_is_told_when_turns_run_low` | Long-horizon |
| OS sandbox for Bash (`--sandbox`, `KODACT_SANDBOX`, `sandbox.*` settings) | partial | `builtin::sandbox_confines_writes_and_network`, `sandbox::bwrap_arguments`, `sandbox::failure_hints`, `permissions::sandboxed_commands_need_no_prompt_but_rules_still_apply`, `e2e::sandbox_lets_headless_runs_build_without_prompts` | Tool design / safety. The Codex pattern. Linux bubblewrap done; the macOS Seatbelt profile is untested |

## Config, memory, hooks (M3)

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Settings layers + `--setting-sources` | done | `config::layers_merge_in_order`, `config::sources_filter_layers` |  |
| `--settings` file or JSON | done | `config::layers_merge_in_order` |  |
| KODACT.md discovery + `@import` | done | `config::discovers_and_imports`, `config::import_cycles_stop` |  |
| Hooks: PreToolUse, PostToolUse, PostToolUseFailure, UserPromptSubmit, Stop, SubagentStop, SessionStart, SessionEnd, PreCompact, Notification | partial | `engine::hooks_in_the_loop`, `engine::user_prompt_hook_can_block` | All events run except Notification, which waits for the TUI; SubagentStop: `agents::unknown_agent_is_rejected_and_subagent_stop_hook_runs` |
| Hook exit-code semantics per event | done | `hooks::c11_exit2_semantics_per_event` | Contract C11 |
| Hook JSON output (decision, reason, `updatedInput`, `additionalContext`) | done | `hooks::c11_json_permission_decision` | Contract C11 |

## Sessions (kodact-session)

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| JSONL transcript | done | `session::c5_key_collision_keeps_projects_apart`, `session::write_and_resume_round_trip` | Contract C5 |
| `additionalDirectories` restored on resume | done | `core::resume_restores_additional_directories` | Contract C5 |
| System events in JSONL (fallback, compact, microcompact, mode) | done | `engine::c6_fallback_on_overload_for_this_turn_only`, `engine::c9_micro_is_sticky_across_requests`, `engine::c9_auto_triggers_at_threshold` | Contracts C5, C6 and C9 |
| `--continue` / `--resume <id>` | done | `core::continue_and_fork`, `e2e::max_turns_and_continue` |  |
| `--fork-session` / `--session-id` | done | `core::continue_and_fork`, `session::fork_copies_chain_under_new_id` |  |
| `--no-session-persistence` | done | `session::no_persistence_writes_nothing` |  |
| File checkpoints + rewind | done | `session::rewind_restores_and_deletes`, `engine::edits_are_checkpointed_for_rewind`, `e2e::replay_uuid_drives_rewind_files` | Contract C4 |

## Agents, skills, commands (M5)

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Sub-agents (Task) with built-in types (general-purpose, Explore, Plan) | done | `agents::explore_agent_reports_back_in_its_own_context`, `agents::child_edits_are_checkpointed_in_the_parent_turn` | Isolated context; shared rules, prompt lock, checkpoints and budget; no nesting. Per-agent `effort` (Explore runs at `low`) |
| Task `isolation: "worktree"` | done | `agents::isolated_agents_work_in_their_own_worktrees`, `agents::isolation_outside_a_repository_is_refused_with_the_way_out` | The agent works in `.kodact/worktrees/agent-<id>` on branch `kodact/agent-<id>`; kept and named in the result when it changed something, removed otherwise. Nothing is merged for it |
| Custom agents `agents/*.md`, `--agents`, `--agent` | done | `agents::markdown_agents`, `agents::json_agents_and_precedence`, `core::sub_agents_are_offered_and_configurable` |  |
| Skills `skills/*/SKILL.md` (Skill tool, `/name`) | done | `skills::discovers_and_loads_skills_on_demand`, `extend::commands_skills_styles_and_plugins` | Names and descriptions up front, bodies on demand; `disable-model-invocation`, `user-invocable` |
| Reference-CLI skills load unchanged: `~/.claude/skills`, `.claude/skills` (repository root down), and the same for commands, agents and output styles | done | `skills::discovers_and_loads_skills_on_demand`, `eval::a_reference_cli_skill_runs_unchanged` | `$CLAUDE_CONFIG_DIR` moves the user folder. Lowest precedence in each scope (user: `~/.claude` then KodAct's; project: `.claude`, `.agents`, `.kodact`). A skill of yours with a built-in's name replaces it |
| Skill text filled in: `$ARGUMENTS`, `$ARGUMENTS[N]`, `$N` (from 0, shell-style quoting), `$name` from `arguments`, `${KODACT_SKILL_DIR}` `${KODACT_PROJECT_DIR}` `${KODACT_SESSION_ID}` `${KODACT_EFFORT}` `${KODACT_PLUGIN_ROOT}` `${KODACT_PLUGIN_DATA}` and their `${CLAUDE_*}` aliases | done | `substitute::tests::*`, `skills::reference_cli_skills_render_as_they_do_there`, `eval::a_reference_cli_skill_runs_unchanged` | `\$` escapes; values are not re-expanded; a missing index stays as written, a missing name is empty; `ARGUMENTS: …` is appended only when no placeholder took them |
| Skill frontmatter as the reference CLI reads it | done | `frontmatter::tests::*`, `invoke::tests::*`, `permissions::tests::a_skill_s_grants_last_one_turn`, `core::driver_tests::a_typed_skill_holds_its_frontmatter_for_its_turn_only`, `core::driver_tests::the_model_loads_a_skill_and_its_grants_hold_for_the_rest_of_the_turn`, `core::driver_tests::a_fork_skill_runs_in_a_sub_agent_and_its_report_joins_the_conversation`, `eval::a_reference_cli_skill_runs_unchanged` | ``!`cmd` `` (line start or after whitespace) and ` ```! ` blocks run before the model sees the skill: deny rules, then `allowed-tools`, then the session's rules; bash, or PowerShell with `shell: powershell`; 2 minutes; a failure stops the invocation (exit 1 of grep/diff/test is not one); `disableSkillShellExecution`. `allowed-tools` allows and `disallowed-tools` hides tools for the turn; `model`/`effort` hold for the turn; `hooks` (nested YAML) stay for the session; `context: fork` runs in a sub-agent of type `agent` and its report joins (it waits for it, as the reference CLI does under `-p`); `arguments`, `argument-hint`, `when_to_use`, `paths`, `shell`. Lists split outside `(…)`; `1`/`0` booleans |
| Where skills come from, and which wins | done | `core::commands::tests::your_skill_replaces_a_built_in_or_a_command_of_its_name`, `core::driver_tests::path_limited_and_nested_skills_appear_once_their_files_are_in_play`, `core::driver_tests::added_folders_and_overrides_shape_the_skills` | Your skill replaces a built-in of its name (not an alias) and a command file of its name; names match regardless of case. `.claude/skills` in a subfolder loads once a file there is read or written (`apps/web:deploy` when the name is taken); `--add-dir` folders' skills load (`<folder>:<name>` when taken). `skillOverrides` (`on`, `name-only`, `user-invocable-only`, `off`; plugin skills excepted). `Skill(name)` / `Skill(name *)` rules. A skill loaded again adds a note, not a copy; loaded skills are attached again after compaction (5,000 tokens each, 25,000 in all) |
| `@path` mentions attach files, in every mode (`-p`, stream-json, REPL, TUI) | done | `attach::tests::*` (matching, quoting, dedup, limits, directories, binaries, permissions), `core::driver_tests::at_mentions_attach_files_to_plain_prompts`, `e2e::print_attaches_at_mentions` | The prompt stays as typed; contents follow in a `<system-reminder>` block. Read's permission decision without prompting (ask or deny leave a note). Attached files count as read for Edit. Images and PDFs get a note to use Read rather than being inlined. PreToolUse hooks don't run for attachments. Wrapper tags in file text are escaped and the injection scan applies (`attach::tests::file_text_cannot_close_its_wrapper_and_is_flagged`) |
| Custom slash commands `commands/*.md` | done | `commands::expands_arguments_commands_and_files`, `commands::loads_namespaced_commands_with_precedence`, `core::commands::tests::parses_commands_skills_paths_and_unknowns`, `extend::commands_skills_styles_and_plugins` | `$ARGUMENTS`, `$1`..`$9`, ``!`cmd` `` gated by `allowed-tools`, `@file`, `dir:name` namespaces. `allowed-tools` does not yet grant tool permissions for the turn, and `model` is not applied |
| Output styles: default, explanatory, learning + custom | done | `styles::builtins_and_custom_styles`, `extend::commands_skills_styles_and_plugins` | `outputStyle` setting; KodAct's own style texts |
| Plugins (`--plugin-dir`, `pluginDirs`) | done | `plugins::manifests_and_components`, `plugins::reference_cli_plugins_load_with_their_paths_filled_in`, `extend::commands_skills_styles_and_plugins` | Commands, agents, skills, output styles, hooks and `.mcp.json`. The reference CLI's `.claude-plugin/plugin.json`, with `hooks` and `mcpServers` inline or as paths, and extra `skills`/`commands`/`agents`/`output-styles` paths; a `SKILL.md` at the plugin's root; `${KODACT_PLUGIN_ROOT}`/`${KODACT_PLUGIN_DATA}` (and `${CLAUDE_*}`) filled in. No marketplace (KodAct Desktop installs plugins) |
| Built-in commands in every mode | done | See "Slash commands" below | One registry (C17) |

## MCP (M6)

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| stdio client (server requests `ping`, `roots/list`; `list_changed`; stderr in errors) | done | `client::stdio_server_tools_resources_and_server_requests`, `client::failures_are_reported_not_fatal`, `transport::dispatch_routes_responses_requests_and_notifications` | |
| Streamable HTTP client (JSON and SSE answers, `Mcp-Session-Id`, protocol header, DELETE on close) | done | `client::streamable_http_json_sse_and_sessions` | |
| SSE client (2024-11-05 transport) | partial | | Built; no test against an SSE server yet |
| Tools (`mcp__<server>__<tool>`), resources (`ListMcpResourcesTool`, `ReadMcpResourceTool`) | done | `client::stdio_server_tools_resources_and_server_requests`, `tools::converts_every_content_type`, `tools::names_are_clean_and_short` | |
| Prompts as slash commands | done | `core::commands::tests::parses_commands_skills_paths_and_unknowns` (routing) | `/mcp__<server>__<prompt> args`; arguments fill the declared ones in order. No end-to-end test with a prompt server yet |
| Server instructions in the system prompt | done | `client::stdio_server_tools_resources_and_server_requests` | |
| `--mcp-config` / `--strict-mcp-config` / `.mcp.json` with approval (C16) | done | `config::project_servers_need_the_users_approval`, `config::precedence_and_strict_mode`, `mcp::project_servers_wait_for_approval` | |
| `${VAR}` / `${VAR:-default}` expansion | done | `config::expands_environment_variables` | |
| `kodact mcp add/add-json/remove/list/get/approve` | done | `mcp::add_list_get_remove`, `mcp::project_servers_wait_for_approval` | `approve` is KodAct's (the reference asks interactively) |
| `kodact mcp serve` | done | `client::kodact_serves_its_tools`, `mcp::add_list_get_remove` | Dangerous-command patterns are refused (C14) |
| MCP tools in sub-agents | done | `core` wiring via `AgentRuntime.extra_tools` | Agent tool lists may name `mcp__<server>` |
| OAuth for remote servers | todo | | Headers (`-H`) only for now |

## CLI flags (from the reference `--help`)

| Flag | Status | Test | Notes |
| --- | --- | --- | --- |
| `-p/--print`, `--output-format`, `--input-format` | done | `e2e::print_text_and_request_shape`, `e2e::print_json_and_piped_stdin`, `e2e::stream_json_golden_basic` |  |
| `--include-partial-messages`, `--replay-user-messages`, `--include-hook-events` | partial | `e2e::host_controls_mode_model_and_interrupt`, `e2e::replay_uuid_drives_rewind_files` | `--include-hook-events` is accepted but emits nothing yet |
| `--model`, `--fallback-model`, `--effort`, `--betas` | done | `core::builds_from_settings_and_flags`, `engine::c6_fallback_on_overload_for_this_turn_only` |  |
| `--permission-mode`, `--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, `--permission-prompts`, `--permission-prompt-tool` | done | `core::skip_permissions_flag_beats_settings_mode`, `e2e::host_permission_prompt_allow_and_deny`, `e2e::print_errors` |  |
| `--allowedTools`, `--disallowedTools`, `--tools`, `--add-dir` | done | `core::builds_from_settings_and_flags`, `e2e::headless_denies_and_reports` |  |
| `--system-prompt[-file]`, `--append-system-prompt[-file]`, `--exclude-dynamic-system-prompt-sections` | done | `prompts::replace_append_and_exclude_dynamic` |  |
| `-c/--continue`, `-r/--resume`, `--fork-session`, `--session-id`, `--no-session-persistence`, `-n/--name` | partial | `core::continue_and_fork`, `e2e::max_turns_and_continue` | `--name` is accepted but not shown anywhere until the TUI |
| `--max-turns`, `--max-budget-usd`, `--json-schema` | partial | `e2e::max_turns_and_continue`, `engine::c7_budget_and_max_turns_stop_the_run` | `--json-schema` sends `output_config.format` and parses the result; no retry on invalid output yet |
| `--mcp-config`, `--strict-mcp-config` | done | `mcp::session_uses_mcp_tools_and_reports_status`, `mcp::project_servers_wait_for_approval` | |
| `--settings`, `--setting-sources`, `--bare`, `--safe-mode`, `--restricted` | partial | `config::layers_merge_in_order` | `--restricted` not implemented yet |
| `--agents`, `--agent`, `--plugin-dir` | done | `agents::json_agents_and_precedence`, `core::sub_agents_are_offered_and_configurable`, `extend::commands_skills_styles_and_plugins` | |
| `--disable-slash-commands` | todo | | |
| `--debug`, `--debug-file`, `--verbose`, `-v/--version` | done | `e2e::version_and_help` |  |
| `-w/--worktree` | done | `core::c10_worktree_flag_runs_the_session_in_a_new_worktree`, `git::c10_worktree_session_listed_from_main_repo` | Contract C10. The exit prompt to remove it waits for the TUI |
| `--autocompact` | done | `core::autocompact_values` | Also `autoCompactWindow` and `autoCompactEnabled` in settings |
| `--brief`, `--prompt-suggestions`, `--forward-subagent-text` | todo | | |
| `--cloud`, `--teleport`, `--remote-control*`, `--desktop`, `--chrome`/`--no-chrome`, `--ide`, `--from-pr`, `--environment`, `--file`, `--bg`, `--tmux`, `--plugin-url`, `--ax-screen-reader`, `--system-prompt-snapshot` | out | | Need a vendor's cloud, desktop or IDE services, or hosted infrastructure that a personal CLI doesn't have |

## Subcommands

| Command | Status | Test | Notes |
| --- | --- | --- | --- |
| `mcp` | done | `mcp::add_list_get_remove` | `serve`, `add`, `add-json`, `remove`, `list`, `get`, `approve` |
| `doctor` | partial |  | Basic checks only |
| `config` (get/set/list/add/remove) | partial |  | `list` and `get` only |
| `agents` (list) | todo | | Background-session management is out |
| `auth`, `setup-token`, `install`, `update`, `gateway`, `ultrareview`, `attach`, `logs`, `stop`, `rm`, `respawn`, `purge`, `import`, `auto-mode`, `plugin` marketplace | out | | Accounts, cloud, distribution or background-session services |

## Slash commands (C17)

The reference is the official command list (`/help` of version 2.1.290 and
the commands page). Every built-in sits in one registry,
`crates/kodact-core/src/commands/mod.rs` `BUILTINS`, which also feeds `/help`,
`system/init` `slash_commands` and the `initialize` response's `commands`.
The test `core::commands::tests::registry_is_consistent` checks that names
and aliases are unique, sorted and never shadow each other.

Prefix `cmds::` is `crates/kodact-cli/tests/commands.rs`.

**Modes:** **P** is `-p` (text, json, stream-json), **R** the line REPL, **T**
the terminal UI (M8). The REPL, the TUI and `-p` share one driver
(`kodact_core::Driver`), so a command tested in `-p` behaves the same in R
and T; every command marked R also runs in T.

| Command (aliases) | Status | Modes | Test | Notes |
| --- | --- | --- | --- | --- |
| `/help` | done | PR | `cmds::info_commands_answer_locally`, `core::commands::tests::help_and_catalog_come_from_the_registry` | Built-ins with hints and aliases, then custom commands, skills and MCP prompts |
| `/exit` (`/quit`) | done | PR | `cmds::info_commands_answer_locally` | In `-p` it ends quietly with exit 0 |
| `/clear [name]` (`/reset`, `/new`) | done | PR | `cmds::info_commands_answer_locally`, `cmds::clear_starts_a_new_session_for_stream_hosts`, `core::driver_tests::clear_resume_and_branch_switch_sessions`, `tui::session::tests::clear_resume_and_rewind_start_the_screen_over`, `tui::app::tests::a_replaced_conversation_starts_the_screen_over` | A new session id; the old conversation stays resumable, optionally named. Stream hosts get a new `system/init`. In the terminal UI the screen starts over: the front page again, with the conversation it left as the one to `/resume` |
| `/compact [instructions]` | done | PR | `extend::commands_skills_styles_and_plugins`, engine compaction tests (C9) |  |
| `/usage` (`/cost`, `/stats`) | done | PR | `cmds::info_commands_answer_locally`, `cmds::json_output_marks_local_results`, `core::driver_tests::immediate_commands_answer_from_the_view_during_a_turn`, `tui::app::tests::immediate_commands_are_sent_while_busy` | Cost, wall and API time, model calls, prompts, tool calls, tokens per model, context now. No plan limits: KodAct has no subscription. Immediate: mid-turn it counts the turn so far |
| `/status` | done | PR | `cmds::info_commands_answer_locally` | Version, session and title, directories, model, effort, thinking, mode, output style, sandbox, API key source, settings files, memory, MCP, hooks. Immediate: answered mid-turn in the TUI and stream-json (`tui::session::tests::immediate_commands_answer_while_a_turn_runs`, `e2e::stream_json_answers_immediate_commands_mid_turn`) |
| `/doctor` (`/checkup`) | done | PR | `cmds::info_commands_answer_locally` | `kodact doctor`'s checks plus session warnings, MCP failures and model pricing |
| `/release-notes` | done | PR | `cmds::info_commands_answer_locally` | `CHANGELOG.md`, embedded at build time |
| `/hooks` | done | PRT | `cmds::info_commands_answer_locally`, `core::driver_tests::hooks_add_list_and_remove_through_settings` | Numbered list per event with each hook's source; `add` and `remove` argument forms write the settings file and reload. In the TUI an editor screen: each event's hooks, Enter on one asks then removes it, "Add hook…" opens a form (event, matcher, command, scope) |
| `/mcp [reconnect\|enable\|disable <server\|all>]` | done | PR | `cmds::info_commands_answer_locally`, `core::driver_tests::mcp_servers_turn_off_and_on_through_the_driver`, `client::servers_turn_off_on_and_reconnect` | Status list. `reconnect` restarts a server from the config it started with; `disable` stops it and hides its tools, prompts, resources and instructions at once, and saves it in `disabledMcpjsonServers` (local settings); `enable` reverses both. Tools a server didn't list when the session was built join after `/reload-plugins`. `--mcp-config` and plugin servers change for the session only. Immediate: mid-turn, the manager changes at once and the instructions and `system/init` refresh when the turn ends (`core::driver_tests::mcp_changes_mid_turn_refresh_the_session_after_the_turn`) |
| `/skills` | done | PR | `cmds::info_commands_answer_locally` | Source, who can invoke it, token estimate |
| `/agents` | done | PRT | `cmds::info_commands_answer_locally`, `core::driver_tests::agents_create_writes_a_definition_and_reloads` | List plus `/agents create <name> --description .. --tools .. --model .. --scope ..`, which writes the definition and reloads. In the TUI a screen with a creation wizard (a form: name, description, instructions, tools multi-select, model, scope). Editing and deleting agents is by file |
| `/plugin` | partial | PR | `cmds::bad_commands_fail_with_exit_1` | `list` only. Marketplaces are out (vendor service) |
| `/memory` | partial | PR | `cmds::info_commands_answer_locally` | Lists the files; opening one in `$EDITOR` comes with R/T editing |
| `/tasks` (`/bashes`) | done | PR | `cmds::info_commands_answer_locally`, `cmds::bad_commands_fail_with_exit_1`, `core::driver_tests::immediate_commands_answer_from_the_view_during_a_turn` | Background shells, subtasks and scheduled tasks, and `stop <id>`. Immediate |
| Unknown and out-of-scope names | done | PR | `cmds::bad_commands_fail_with_exit_1` | `Unknown command: /name`, exit 1; a path, a file name (`/package.json ...`) or `a/b` is a prompt |
| Skill chaining `/a /b text` | done | PR | `core::commands::tests::parses_commands_skills_paths_and_unknowns` | Up to 6 skills; the text goes to each |
| `/context [all]` | done | PR | `core::driver_tests::rename_export_context_and_diff` | Estimates (4 chars a token) per part against the window, the last measured request, suggestions. Immediate. In the TUI, bare `/context` is a 10×10 colour grid with the same numbers (letters without colour), mid-turn too (`tui::session::tests::bare_diff_and_context_open_screens`, `core::driver_tests::context_screen_and_text_share_their_numbers`, `tui::render::tests::context_parts_use_letters_without_colour`) |
| `/model [model]` | done | PRT | `core::driver_tests::model_effort_and_fast_reach_the_request`, `core::driver_tests::interactive_surfaces_save_defaults`, `cmds::settings_commands_persist_and_reach_the_api` | Text list in P and R, a picker in T; saves `model` in R, session-only in P. Warns when effort or fast mode stop applying. Lists only what the endpoint reports (`Provider::list_models`), never a built-in catalogue (`core::driver_tests::model_lists_only_what_the_endpoint_offers`) |
| `/effort [level\|auto\|status]` | done | PR | `core::driver_tests::model_effort_and_fast_reach_the_request` | Levels from the model table; `max` is session-only; `auto` clears it |
| `/fast [on\|off]` | partial | PR | `core::driver_tests::model_effort_and_fast_reach_the_request`, `cmds::settings_commands_persist_and_reach_the_api` | Sends `speed: fast` and the beta flag only on models that offer it. Gap: fast-mode prices aren't modelled, so `/usage` under-reports its cost |
| `/config [key=value]` (`/settings`) | done | PR | `core::driver_tests::config_validates_then_writes_and_applies`, `core::driver_tests::config_reports_a_layer_that_overrides_it`, `cmds::settings_commands_persist_and_reach_the_api` | Argument form of the reference's panel: a whitelist of ten keys, validated before any write, applied live, `--scope`; names an overriding layer |
| `/output-style [style]` | done | PRT | `core::driver_tests::output_style_survives_an_sdk_system_prompt` | Rebuilds the system prompt; saves `outputStyle` in local settings (R) |
| `/autocompact [auto\|tokens]` | done | PR | `core::driver_tests::autocompact_and_sandbox` | Also `on`/`off` (`autoCompactEnabled`) |
| `/permissions` (`/allowed-tools`) | done | PRT | `core::driver_tests::permissions_add_list_and_remove` | Text list with each rule's layer; `add`/`remove` arguments instead of the dialog (a picker in T) |
| `/add-dir <path>` | done | PR | `core::driver_tests::add_dir_widens_access_and_tells_the_model` | `--save` stands in for the reference's "remember" choice |
| `/sandbox [on\|off\|mode]` | done | PR | `core::driver_tests::autocompact_and_sandbox` | Changes the live policy, sub-agents included |
| `/reload-skills`, `/reload-plugins` | done | PR | `core::driver_tests::cd_and_reload_rebuild_the_session` | Rebuilds the session in place (same id and conversation) and reports counts added and removed. Plugin MCP servers start with a new session |
| `/rename [name]` | done | PR | `core::driver_tests::rename_export_context_and_diff` | Without a name, the small model suggests one |
| `/export [file]` | done | PR | `core::driver_tests::rename_export_context_and_diff` | No argument prints it (no clipboard dialog) |
| `/diff` | done | PRT | `core::driver_tests::rename_export_context_and_diff`, `commands::screens::tests::diffs_list_files_then_hunks_with_jumps`, `tui::render::tests::screens_scroll_inside_a_box_that_fits_the_terminal` | Text output elsewhere. In the TUI a scrolling viewer: files with +/- counts (Enter jumps to the hunks, Esc back), then hunks coloured by side |
| `/debug [description]` | done | PR | `cmds::debug_turns_on_a_session_log` | A reloadable logger that is off until `/debug`; requests, responses, tool calls, permission checks, hooks and compaction are logged |
| `/plan [description]` | done | PR | `core::driver_tests::recap_plan_and_shell_mode` | Plan mode; with a description, that prompt runs in plan mode |
| `/goal [condition\|clear]` | done | PR | `core::driver_tests::goal_runs_until_the_check_passes`, `core::driver_tests::goal_pauses_without_progress_and_resumes_on_a_prompt`, `core::driver_tests::goal_is_cleared_by_fatal_errors_and_by_request`, `core::driver_tests::goal_is_refused_when_hooks_are_disabled_and_restored_on_resume`, `cmds::goal_runs_to_completion_in_print_mode` | Contract C18. KodAct's own evaluator prompt. Extra: an unmet goal makes a single `-p` run exit 1 |
| `/btw [question]` | done | PR | `core::driver_tests::btw_answers_without_touching_the_conversation` | Same system prompt and tools with `tool_choice: none`; the 20 newest exchanges ride along; cost counts. Immediate: mid-turn it answers from the conversation so far (a reply whose tool calls are unanswered is left out); its cost and exchange are recorded when the turn ends (`core::driver_tests::btw_mid_turn_answers_and_its_cost_counts_after_the_turn`) |
| `/recap` | done | PR | `core::driver_tests::recap_plan_and_shell_mode` | One line from the small model |
| `!command` shell mode | done | R | `core::driver_tests::recap_plan_and_shell_mode`, `core::driver_tests::shell_mode_can_skip_the_model` | Interactive only: in `-p` and stream-json a `!` prompt goes to the model as text, since it usually comes from a program. Runs as the user (no prompt, no sandbox), 2-minute limit, Ctrl-C stops it; the model answers unless `respondToBashCommands` is false |
| `/rewind` (`/checkpoint`, `/undo`) | done | PRT | `core::driver_tests::rewind_restores_code_and_conversation`, `core::driver_tests::rewind_summarizes_part_of_the_conversation` | Argument form of the picker: `/rewind <n> both\|conversation\|code\|summarize-from\|summarize-to [instructions]`. In T, a two-step picker that puts the restored prompt back in the input box |
| `/branch [name]` | done | PR | `core::driver_tests::clear_resume_and_branch_switch_sessions` | Copies the conversation from memory, so it works without session persistence |
| `/resume [session]` (`/continue`) | done | RT | `core::driver_tests::clear_resume_and_branch_switch_sessions` | Numbered list, then a number, id prefix or name (a picker in T). In `-p` it explains `--resume` |
| `/cd <path>` | done | PR | `core::driver_tests::cd_and_reload_rebuild_the_session` | Continues the conversation as a new session in the new directory; its commands, skills and settings load; MCP connections stay |
| `/loop [interval] [prompt]`, `CronCreate`, `CronList`, `CronDelete`, `ScheduleWakeup` | done | PR | `core::schedule::tests::*`, `core::driver_tests::loop_with_an_interval_schedules_and_runs_now`, `core::driver_tests::self_paced_loops_reschedule_fall_back_and_stop`, `core::driver_tests::loop_md_and_scheduled_commands`, `cmds::print_mode_keeps_running_for_scheduled_tasks` | Contract C19. Intervals are parsed in Rust, not by a bundled skill; KodAct's own loop and maintenance prompts. `-p` keeps running while tasks are pending |
| Bundled skills: `/init`, `/code-review` (`/review`), `/security-review`, `/simplify`, `/verify`, `/run`, `/run-skill-generator`, `/batch`, `/fewer-permission-prompts`, `/update-config` | done | PR | `agents::skills::tests::bundled_skills_load_and_give_way`, `cmds::info_commands_answer_locally` | KodAct's own text (`crates/kodact-agents/bundled/`), lowest precedence; `KODACT_PROMPTS_DIR/skills/<name>.md` or a same-named skill replaces one. The model is offered only code-review, security-review, simplify, verify and update-config |
| `/feedback [description]` (`/bug`, `/share`) | done | PR | `cmds::feedback_writes_a_local_bundle_with_secrets_masked`, `config::redact::tests::redacts_secrets_inside_text`, `config::redact::tests::redacts_headers_quoted_keys_and_private_keys` | Out of the reference's upload path on purpose: a private local bundle (report, doctor, masked transcript and settings) the person can share themselves |
| `/import [codex\|gemini\|cursor] [--yes]` | done | PR | `core::import::tests::*`, `cmds::import_shows_a_plan_then_applies_it` | MCP servers from `~/.codex/config.toml` (a TOML-subset parser), `~/.gemini/settings.json`, `~/.cursor/mcp.json` and `.cursor/mcp.json`; instructions from GEMINI.md, `.cursor/rules` and `.cursorrules`. User-level servers go to user settings, repository servers to `.mcp.json` (still needing approval). Plan first, `--yes` applies, never twice |
| `/advisor [model\|off]` | done | PR | `core::driver_tests::advisor_is_consulted_only_while_set` | An `Advisor` tool, shown only while a model is set. It reads the conversation from the transcript and asks with a KodAct-written prompt; its cost counts toward the session and the budget. `advisorModel` is saved in the REPL |
| `/subtask <task>` | done | PRT | `core::driver_tests::subtask_forks_the_conversation_and_reports_back`, `core::driver_tests::subtasks_stop_on_request_and_are_orphaned_by_clear`, `cmds::print_mode_waits_for_subtasks_then_answers_with_their_reports` | A background agent forked from the whole conversation (same system prompt, tools and messages, so its first request reads the prompt cache). It never asks; tools that would reach the person or the schedule refuse. Its report reaches the model with the next prompt; `-p` waits for it and runs one more turn. `/tasks` lists and stops subtasks; `/clear` and `/resume` stop them. At most 8 at once. Contract C20 |
| `/theme [dark\|light\|none]` | done | T | `core::driver_tests::tui_only_commands_save_theme_and_status_line`, `tui::app::tests::question_mark_themes_and_clipboard` | Three themes; saved as `theme`; a picker without an argument. Other surfaces answer "works only in the terminal UI" |
| `/copy [n]` | done | T | `tui::session::tests::turns_commands_questions_and_exit_go_through_the_channels`, `tui::tests::osc52_carries_base64` | OSC 52 only (no system clipboard tools), so it reaches the clipboard over SSH too; no confirmation that the terminal accepted it |
| `/keybindings` | done | T | `tui::session::tests::turns_commands_questions_and_exit_go_through_the_channels`, `tui::keys::tests::*`, `tui::app::tests::key_bindings_apply_and_show_in_the_key_table` | Shows the keys in effect (also `?`). `<config>/keybindings.json` maps keys to named actions (`{"ctrl+s": "submit", "ctrl+g": "none"}`); bad entries are warnings at start. KodAct's own file and action names (CLI.md, "Key bindings") |
| `/statusline [command\|off]` | done | T | `core::driver_tests::tui_only_commands_save_theme_and_status_line`, `tui::session::tests::turns_commands_questions_and_exit_go_through_the_channels`, `tui::render::tests::a_status_line_command_replaces_the_right_side` | The reference has the model write the script; here you give the command. KodAct's own JSON fields (CLI.md); first line only, escapes stripped |
| `/terminal-setup` | partial | T | `core::driver_tests::tui_only_commands_save_theme_and_status_line` | Explains the keyboard protocol and the fallbacks; doesn't edit terminal config files |
| `/color [name\|default]` | done | T | `tui::app::tests::color_and_focus_change_this_session_only` | The accent colour (prompt and answer markers, selections, dialog borders) for this session: red, orange, yellow, green, cyan, blue, purple, pink. Not saved; `/theme` changes keep it |
| `/focus [on\|off]` | done | T | `tui::app::tests::color_and_focus_change_this_session_only` | Keeps tool calls and their results out of the scrollback until turned off; the spinner still names the running tool. Session only |
| `/tui` | out | | | KodAct has one terminal UI (an inline viewport over the terminal's scrollback); `--no-tui` and `KODACT_TUI=0` pick the line REPL at launch |
| `/scroll-speed` | out | | | The terminal scrolls its own scrollback (KodAct never takes over the screen), so the terminal's scroll setting applies; screens scroll by row and page |
| `/login`, `/logout`, `/upgrade`, `/usage-credits`, `/rate-limit-options`, `/privacy-settings`, `/passes`, `/stickers`, `/mobile`, `/desktop`, `/chrome`, `/remote-control`, `/remote-env`, `/teleport`, `/web-setup`, `/autofix-pr`, `/schedule`, `/install-github-app`, `/install-slack-app`, `/ultrareview`, `/insights`, `/team-onboarding` and other account, cloud or vendor commands | out | P | `cmds::bad_commands_fail_with_exit_1` | They need the vendor's account or cloud. Each answers `Unknown command` |
| `/vim`, `/pr-comments`, `/ultraplan` | out |  |  | Removed upstream |

## TUI (M8)

The design is in `docs/TUI.md`. Prefix `tui::` is
`crates/kodact-cli/src/tui/` (run with `cargo test -p kodact-cli --bin kodact tui`).
The manual checks are in `docs/CHECKLIST.md`, "Terminal UI".

| Feature | Status | Test | Notes |
| --- | --- | --- | --- |
| Inline viewport: output in the terminal's scrollback, a live region at the bottom | done | `tui::tests::commit_wraps_into_scrollback_above_the_viewport`, `tui::tests::commit_writes_long_output_in_chunks` | Not an alternate screen. The region grows at once and shrinks when idle or when a dialog closes |
| Streaming answers, one-line markdown, tool calls and results | done | `tui::app::tests::streamed_text_moves_to_scrollback_line_by_line` | Headings, bold, inline code and fences. No syntax highlighting or tables yet |
| Spinner with activity and elapsed time | done | `tui::render::tests::busy_shows_the_answer_line_spinner_and_queue` | KodAct's own frames and wording |
| Input box: multiline, word moves, kill commands, history, paste | done | `tui::editor::tests::*`, `tui::render::tests::narrow_terminals_wrap_the_input_and_keep_the_box` | Bracketed paste; Shift+Enter needs the keyboard protocol, else Alt+Enter, Ctrl+J or `\` Enter |
| Prompt history across sessions | done | `tui::tests::history_is_per_directory_and_private` | `<state>/history.jsonl`, per directory, last 500, mode 0600 |
| `/` command menu | done | `tui::app::tests::slash_menu_filters_completes_and_runs`, `tui::render::tests::the_menu_lists_matching_commands` | Prefix matches first, then substring matches |
| Permission, AskUserQuestion and plan dialogs | done | `tui::app::tests::dialogs_answer_permissions_questions_and_plans`, `tui::render::tests::dialogs_draw_in_a_box_without_the_input`, `tui::session::tests::turns_commands_questions_and_exit_go_through_the_channels` | "Don't ask again" adds the suggested rule |
| Queued messages while a turn runs | done | `tui::app::tests::enter_sends_and_queues_while_busy`, `tui::session::tests::turns_commands_questions_and_exit_go_through_the_channels` | |
| Esc / Ctrl-C / Ctrl-D / Shift+Tab | done | `tui::app::tests::esc_ctrl_c_and_shift_tab` | Esc twice opens `/rewind` |
| Status line: mode, model, context, cost | done | `tui::render::tests::idle_shows_the_placeholder_and_status`, `tui::render::tests::plan_mode_and_hints_show_in_the_status_line` | |
| Scheduled tasks and subtasks while idle | done | `tui::session` (same loop as the REPL) | A firing task shows the spinner |
| Terminal restored on exit and on panic | done | manual (CHECKLIST) | A Drop guard and a panic hook |
| `--no-tui`, `KODACT_TUI=0` | done | manual | The line REPL |
| Pickers for `/model`, `/resume`, `/rewind` (two steps), `/output-style`, `/permissions` | done | `core::driver_tests::pickers_list_choices_that_are_command_text`, `tui::app::tests::pickers_filter_and_run_command_text`, `tui::render::tests::pickers_search_and_file_menus` | Rows come from kodact-core; each runs the command's argument form. A conversation rewind puts the prompt back in the input box |
| Ctrl+R history search | done | `tui::app::tests::ctrl_r_searches_history` | Enter keeps the match in the input box rather than sending it |
| `@` file completion | done | `tui::app::tests::at_completes_paths`, `tui::app::tests::attached_files_show_under_the_prompt`, `tui::tests::project_files_follow_ignore_rules` | Completes paths (quoted when they hold spaces); sending attaches them, and a dim `(attached: ...)` line follows the prompt |
| No colour (`NO_COLOR`, `--color never`) | done | `tui::render::tests::dialogs_draw_in_a_box_without_the_input` | Bold, dim and reverse only |
