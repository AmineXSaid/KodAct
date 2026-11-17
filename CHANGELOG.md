# Changelog

## 0.1.0 (unreleased)

### The terminal UI tested as a person uses it
- `kodact_test_host::pty` runs the real binary in a pseudo-terminal: keys and
  bracketed pastes in, the screen read back through a terminal emulator. Until
  now nothing tested the TUI's own input or screen: `kodact-eval` runs `-p`, and
  the render tests draw into memory (breadth plan, 1.4).
- First tests (`kodact-cli/tests/tui_pty.rs`): a typed prompt reaches the model
  and its answer is shown; a multi-line paste is one prompt, sent only on Enter;
  `/exit` quits.

### Eval tasks checked in a real browser
- A task can say what its check needs: `"requires": ["chromium"]`. Without a
  headless Chrome or Chromium (`KODACT_EVAL_CHROME`, or one on `PATH`),
  `kodact-eval run` and `validate` skip it and say why.
- Two everyday web tasks, `page-total` and `price-order`, checked in the page
  a headless Chrome rendered: the baseline a browser tool for KodAct would
  have to beat (breadth plan, 1.3).

### The dangerous-command scanner, measured and tightened
A labelled corpus (`kodact-permissions/tests/threat_corpus.rs`): 33 commands an
injected instruction could use, 42 ordinary ones close to a pattern. Before:
20 of 32 caught, and reading `/etc/sudoers.d/README` flagged. Now all caught,
none of the ordinary flagged. Newly caught: `sh -c "$(curl …)"` (the form most
install instructions use), PowerShell's `iex`/`irm`, `~/.ssh` without a slash,
the environment piped to the network, `mkfs -t … /dev/…`, `find / -delete`, a
Python reverse shell, `chown … /`, and one level below a system directory
(`/var/lib`). Editing sudoers is flagged; reading it no longer is.

### Azure OpenAI and gateways that want their own headers
- `KODACT_CUSTOM_HEADERS` reached only the Messages API: an OpenAI-compatible
  or Gemini endpoint behind a company gateway that wants a header of its own
  got none. Every endpoint now gets them.
- An OpenAI-compatible endpoint got its key only as `Authorization: Bearer`.
  `openai.authHeader` (or `KODACT_OPENAI_AUTH_HEADER`) names another header to
  send it in: Azure OpenAI's `api-key`, a gateway's `x-api-key`. Logs already
  mask such headers.
- The `notes-across-compaction` eval never offered the Notes tool (off by
  default), so it measured compaction alone. Its project now turns Notes on.
- docs/TUI.md and docs/PARITY.md said the UI was inline only, with no tables or
  syntax highlighting: it is full screen by default and has both.

### Provider fixes found in another harness's field reports
Checked one by one against KodAct, from AlphaCode's provider code (MIT): only
the holes KodAct had too.
- **A stream cut before its end is sent again.** A chat-completions body that
  ends with neither a finish reason nor `[DONE]`, or a Messages body that ends
  before `message_stop`, was taken for the whole reply: half an answer, or a
  turn ended mid-sentence. It is now a "stream ended" error, which the engine
  retries, handing back the reasoning streamed so far. A reply that gave its
  end reason is whole even without the last marker (a finish reason without
  `[DONE]`, a stop reason without `message_stop`): some servers leave it out.
- **A silent reasoner isn't cut off mid-thought.** Some models reason without
  streaming anything (OpenAI's reasoning models over chat completions); at a
  high level that silence ran past the fixed 5-minute read timeout, and the
  request failed and was sent again from the start. The limit now grows with
  the level asked for: twice at `high`, three times at `xhigh`, four times at
  `max`, on chat completions, the Messages API and Gemini alike. Each request
  bounds its own wait for the answer's start the same way, so one with no level
  asked waits no longer than before.
- **MCP tool schemas the endpoint refused.** An object schema without
  `properties` (an MCP tool that takes no arguments) is sent with
  `properties: {}`, which LM Studio and others require. On the Messages API, a
  schema with `anyOf`, `oneOf` or `allOf` at its top, which it refuses, is
  sent as one object holding the branches' properties, those of `anyOf` and
  `oneOf` optional; a property the branches define differently keeps each
  definition, so a discriminated union still offers every tag.
- **Tool call ids from another endpoint.** On the Messages API, an id outside
  `[A-Za-z0-9_-]` (`functions.Read:0`, as some gateways give) has those
  characters replaced, in the call and its result alike, and never meets
  another id.
- Not taken: retry budgets per kind of failure (KodAct already tells quotas,
  outages, long waits and first contact apart), text-wrapped tool calls
  (`textcall.rs` already reads them), and AlphaCode's string-matched error
  classes (KodAct reads the status and error type).

### The model's reasoning can go back with each request (`carryReasoning`)
- **What happened.** On chat completions KodAct sends a model's reasoning back
  only to Gemini models. `deep-thinking` (served by vLLM; deepseek-v4-flash, as
  the Harbor kit's model probe reports it) reasons for thousands of characters
  per reply and writes almost nothing else, so each step began without what
  the last one worked out. In gpt2-codegolf with three times its time
  (2026-11-16), 23 of 38 replies, then 16 of 43 in the next run, worked out
  the same tensor layout again; the layout verified at step 18 was doubted at
  step 19.
- **Now, with `carryReasoning`** (or `KODACT_CARRY_REASONING=1`, off until
  measured), the reasoning of every reply still in the conversation goes back
  with each request, as `reasoning_content`. That is what DeepSeek's thinking
  mode asks when tools are used: "the `reasoning_content` of all previous
  turns should be passed back to the API", or it answers 400. A chat template
  that keeps reasoning only after the last user message (DeepSeek-V3.2's,
  Qwen3's) drops the older part itself; for it, KodAct's own reminders that
  come with tool results go in the last tool result, so they don't end the
  tool loop. The person's own words stay a user message.
- **In everyday use:** in a long request to a reasoning model behind a
  chat-completions endpoint, each step starts from what the earlier ones
  worked out instead of working it out again. On DeepSeek's own API with
  tools, it is what the API requires.
- **What it costs.** No model call and no wait. Each reply's reasoning is
  added to every later request until compaction summarizes it away with the
  rest: it counts toward the context as anything else does, so a small window
  compacts sooner and a 1M window keeps it longer. With a prompt cache it is
  read from the cache after its first request. Nothing changes where the
  provider sends reasoning back on its own (the Messages API, Gemini), or
  where the endpoint returns none.
- **Valid on DeepSeek's own API.** A reply that was all reasoning goes with
  `content: ""` rather than null (DeepSeek refuses an assistant message with
  neither content nor tool calls, carried or not), and with reasoning carried
  a tool-call message whose step reasoned nothing gets a blank
  `reasoning_content`, which DeepSeek asks for on every such message.
- **Safe on an endpoint that refuses it.** A 400 or 422 naming
  `reasoning_content` (not one asking for it, as "must be passed back") sends
  the request again without it, and requests to that model go without it for
  the rest of the session, with a warning (`system/carry_reasoning` event).
  Sub-agents follow the session.
- **Check an endpoint first:** `scripts/probe-reasoning.py MODEL` runs a small
  tool loop that ends with the tool's result, as KodAct's do, and sends the
  result back twice: with reasoning that notes a random eight-digit number,
  and without. Passed to the model, the reply gives the number; dropped, the
  model can't know it. It tells an endpoint that refuses the field, requires
  it, drops it or passes it apart.
- **Measured:** not yet; the comparison is owed (no key here). Eval task
  `layer-order` turns it on: `kodact-eval run` on a reasoning model over chat
  completions, again with `KODACT_CARRY_REASONING=0`, then `kodact-eval
  compare` on turns and wall time. Harbor: `KODACT_CARRY_REASONING=1
  evals/harbor/run.sh …`; the job name gets `-carry`.

### A reply whose stream breaks hands its reasoning to the retry
- **What happened.** In gpt2-codegolf with three times its time (2026-11-16)
  a reply reasoned for 9 minutes, then the connection dropped ("error
  decoding response body"). KodAct sent the request again from scratch: the
  9 minutes of reasoning were lost, a fifth of the run.
- **Now** the reasoning a reply had streamed when its stream broke goes back
  with the request sent again, as it does when a reply reaches the output
  limit while still reasoning: "Your reply was cut off after 548 seconds by an
  error (...), while you were still reasoning; the request is sent again.
  Your next reply would start without that reasoning. Here it is: ...". A
  request that fails before anything streamed is sent again as before.
  Recorded as an `act_reminder` event of kind `broken_stream`.
- **The clock counts it.** With `nudges.clock`, time spent in replies that
  failed partway is said apart ("Replies that failed partway and were sent
  again took 9m08s more."), so the average reply time stays the replies' own.

### The clock: where a timed run stands, as facts (`nudges.clock`)
- **What happened.** In a run with `--max-time` the model is told the limit
  at the start and warned when a fifth is left, and nothing in between. A
  model has no clock. gpt2-codegolf with three times its time (2026-11-16)
  heard "44 minutes" at the start, then nothing for 35 minutes, and spent 27
  of them exploring with `/app/gpt2.c` not written. The pacing nudges removed
  on 2026-11-12 gave advice ("make the change now") and showed no gain.
- **Now, with `nudges.clock`** (off until measured), each time another tenth
  of the limit has gone, until the last fifth, the step's results carry the
  facts: time used and left, tokens written, how many replies so far and
  how long one takes (89% of that run was the model's own replies, about a
  minute each), files the request asks for that don't exist yet, files over the size it allows. No advice: what to do with
  them is the model's call. Recorded as `system/nudge` events of kind `clock`.
- **Everyday use, without `--max-time`:** every 10 replies of a request, how
  long the request has run, its replies and their average length, the tokens
  it wrote, and the same file lines. A short request never hears it, and each
  request counts from zero. Turn it on with `KODACT_NUDGES=clock` or
  `"nudges": {"clock": true}` in settings.

### A file a shell command printed whole counts as read
- **What happened.** Edit and Write refuse a file the model hasn't read, or
  that changed since. Only the Read tool counted as reading. On 2026-11-16, in
  15 long-horizon eval runs, a model that reads files with `cat` was refused
  three times for a file it had just printed whole (`cat pack.py` at the first
  step, then an Edit at the fourth): each time a model call spent reading
  what it had already seen, and a Write's whole content sent again.
- **Now** a file whose whole current content a shell command printed, in
  output still in the conversation, counts as read, as a Read would make it.
  The protection stays where it matters: a file printed in part (`head -1`),
  changed since it was printed, never printed, or whose output compaction
  has cleared from the conversation must still be read first.

### A file the person builds or runs, or one only described, isn't "missing"
- **What happened.** The result's `verification.missing` named files the
  request never asked the model to make:
  - gpt2-codegolf (2026-11-16): `/app/a.out`, from "I will compile with gcc
    … I will run it /app/a.out": the person's own build of the program;
  - password-recovery (2026-11-15, a run that passed): `launchcode.txt`,
    from "The file was named launchcode.txt", a file that was deleted.

  With the `constraints` nudge on, the model was told to make them too.
- **Now** a file named only where the person says what they will do with it
  ("I will …", "I'll …", "we will …", with no "you" or "your" in between
  and no "write it" after) is theirs to build or run, and one named only in
  a sentence that says what was ("was", "were", "had", and no word asking
  for work) is described. Neither is listed missing nor told to the model,
  unless another sentence asks for it ("Write run.sh. I will run run.sh"
  still asks for run.sh).
- **Measured on the requests at hand.** Across the 31 recorded
  Terminal-Bench requests and KodAct's eval tasks, the reading changed for
  those two files, path-tracing's `./image` (the output of the person's
  `gcc -o image`), and files that exist when the run starts, which were
  never listed.

### The result says when a file is over the size the request allows
- **What happened.** gpt2-codegolf (2026-11-15) asked for a C file "<5000
  bytes". The file ended at 5,093 bytes, and the run's result said `success`
  with nothing flagged. KodAct had read the limit from the request, but only
  the `constraints` nudge, off by default, used it.
- **Now** the result's `verification` lists such files under `over_limit`,
  with their size and the most bytes the request allows, beside `missing`:
  `{"over_limit": [{"file": "/app/gpt2.c", "size": 5093, "limit": 4999}]}`.
  It is there whatever the nudges: it tells the caller, not the model. A
  script running `kodact -p --output-format json` can fail or retry on it
  instead of trusting `success`.
- **Where it shows.** `kodact-eval`'s runs table, beside the missing files;
  the Harbor kit's live view (`watch.py`); and a trial's details in its
  report (`KodAct's result flagged: …`).

### The plan's task in progress turns as a ring
- **In the Plan pane.** The sidebar marked the plan's task in progress with
  a braille cell that filled and drained, as it marks a server connecting or
  a sub-agent working. While the model works, that task now has a ring of
  eight dots over two columns (`⢎⡱`), in lime, with a gap of two dots going
  round its edge, clockwise, a dot every 100 ms: the edge turns and the
  inside stays empty, so the step being worked on reads as in progress at a
  glance. While the session waits, the ring is whole and still. The other
  steps' marks take the same two columns, so the text lines up.
- **In the transcript.** Each plan update's checklist marks the task in
  progress with a still circle, `◉`, where it had the half-full cell `⣤`.
  Scrollback doesn't move, and its marks stay one column wide.
- **Seen in snapshots.** Scene 15 gains eight frames, `15-sidebar-ring-0` to
  `15-sidebar-ring-7`, a dot of the turn apart. The snapshot page keeps each
  cell one column wide, as a terminal does, and draws braille in FreeMono:
  DejaVu Sans Mono has none, and the fallback drew it wider, which pushed
  the context gauge past its pane.

### The terminal shows each plan update as a checklist
- **A TodoWrite call shows the plan.** It was shown as `Updated the plan 4
  todos`, with the tool's own message under it. It is now the whole plan, as
  a checklist:
  - done tasks have a green `✓` and their text struck through, as a Markdown
    task list shows them;
  - the task in progress is lime and bold, with a still circle `◉`;
  - the rest are open `○`;
  - the header says how far the plan is (`Updated the plan 1 of 4 done`).
- **Every width, every look.** The sidebar's Plan pane needs 140 columns, so
  in a narrower terminal this was the only view of the plan. The checklist
  shows at any width, dark or light.
- **Seen in snapshots.** The TUI snapshots (`KODACT_SNAPSHOT_DIR`) gain the
  scenes `16-plan` and `16-plan-light` at 100 columns, and draw struck-through
  and italic text.

### The task list as the reference CLI keeps it, completed
- **Every reply counts.** The task-list reminder (`nudges.todoTracking`) now
  counts every reply of the model since the last TodoWrite call, with tools
  or without, as the reference CLI does, not only steps that used tools. It
  goes with the step's results or, when a turn ends first, with the next
  prompt, so a conversation of short turns is reminded too.
- **An empty list is said as a conversation starts.** With the first prompt,
  a note says the list is empty and to create it if the work has several
  steps.
- **The description and the section say the rest.**
  - The description adds: use it in plan mode, keep exactly one task in
    progress while work is left, finish it before starting another, and
    don't mark a task completed while a file or dependency it needs is
    missing.
  - The prompt section gets two worked examples.
- **The spinner says the task in progress.** In the terminal, while the model
  works, the spinner shows the plan's task in progress ("Running the
  tests…", its `activeForm`, else its content) instead of a stock word, as
  the reference CLI's does. This is on for everyone: it changes only what
  the person sees.
- **What it costs.** Off: nothing. On: the notes are in KodAct's own words, at
  most once every 10 replies, with the step's other notes or the prompt,
  never a model call of their own.

### The reference CLI's task list, as an A/B (`nudges.todoTracking`)
- **What it is.** The reference CLI asks a model to keep its task list far
  more than KodAct does, and Claude models use TodoWrite a lot under it. The
  deep-thinking model never called TodoWrite in 30 runs (2026-11-15).
  `todoTracking` turns on that handling, in KodAct's own words:
  - TodoWrite gets a long description: when to use it, when not, worked
    examples, and its states and rules;
  - the system prompt gets a "Task list" section asking for it on work with
    several steps;
  - after 10 steps (model calls that used tools) without a TodoWrite call,
    and at most once every 10 steps, counted across turns, a reminder comes
    with the step's results, showing the list as it stands.
- **Off by default**, like every nudge, until its A/B shows it pays: the
  reminder is advice, not a fact the model lacks (docs/goal.md, rule 8).
  `scripts/nudge-ab.sh todos` runs the long-horizon tasks without it and with
  it; `KODACT_NUDGES=todoTracking` does the same for a Harbor job, whose name
  then says `-n-todoTracking`.
- `kodact-eval` counts TodoWrite calls per run and shows them in its
  reports and A/B tables, beside the pass rate.
- **What it costs.** Off: nothing. On: the longer description and the section
  in every request (read from the server's cache after the first), and the
  reminder's note at most once every 10 steps.

### Measurement tools for what the 2026-11-15 run left open
- `scripts/probe-effort.py` also sends the level in a model's chat-template
  arguments (`chat_template_kwargs.reasoning_effort`), named and as the
  number KodAct sends on DeepSeek's [1, 100] scale, to tell an endpoint that
  ignores `--effort` from one that honours it.
- `scripts/background-waits.py` counts KodAct's end-of-turn waits for
  background commands across recorded runs, with the time left, whether the
  wait began in the run's last fifth, and what the model did next.
- `scripts/nudge-ab.sh`'s `constraints` group runs every task tagged
  `constraints`, size limits and the quiet named-file tasks included.
- Nothing changes in KodAct: each measures a question before a change is
  proposed.

### A file the turn made where the request said is not reported missing
- **A name alone beside its full path is that file.** A request that names
  `/app/repo/algo.py` and later says "algo.py" was read as asking for two
  files, the second at the project's root. Once the model made the first,
  the result's `verification.missing` still listed `algo.py` (a run of
  2026-11-15). The name alone now stands for the path, and a size limit
  given for it is the path's.
- **A name alone made in a folder of its own is not missing.** "Add
  `test_counter.py`" or "the function in `__init__.py`" is read, as at the
  start of a turn, as the file of that name anywhere in the project; one
  made in `tests/` or in a package's folder is no longer listed. A path the
  request gives is still where the file must be.
- **What it changes in real use.** `-p` results, the /goal evaluator (told
  which files "don't exist") and the `constraints` nudge no longer hear of
  files that the turn made. A false entry could keep a goal loop going or
  have the model write a stray copy at the project's root.
- **What it costs.** The alias costs nothing. At a turn's end, the project is
  searched (the same bounded search as at its start) only when a file named
  alone is missing where it was looked for. The model and its window play no
  part.
- `named_corpus`, an ignored test, prints this reading for every request in
  a folder (Harbor job folders, the eval tasks), so a change to it can be
  measured across many requests before it is made.

### Harbor: no trial starts into a dead endpoint; trials that say nothing are listed to run again
- **The endpoint gate.** Before each trial's agent starts, the KodAct agent
  asks the model endpoint whether it answers (`endpoint_gate.py`, with the
  request `probe_model.py` already sends, which costs no tokens). While it
  doesn't (no answer, or a gateway's 502/503/504), the trial waits in
  Harbor's setup, not in the agent's time, and holds its slot, so no other
  trial starts. One request a minute is shared by the waiting trials; when
  the endpoint answers, they all go on. `KODACT_HARBOR_GATE=0` turns it off.
  On 2026-11-15 the model gateway stopped answering at about 06:00: ten
  trials started afterwards, each waited the outage out as an unattended
  KodAct run should, and they spent 2 h 38 min with no reply. Every one of
  them started after this machine could no longer resolve the gateway.
- **"didn't run".** A trial whose agent Harbor never started (a container
  that didn't start, a setup that failed or ran out of time) was counted as a
  valid failure ("crashed"). It is now its own verdict, outside the score
  like an API error or a verifier error. On the 2026-11-15 job, the valid
  trials become 17, of which 15 passed (the report said 15 of 18).
- **Run again.** The report lists the trials outside the score with the
  command that runs them again, one attempt each (`run_again` in its data).
- **watch.py's score** leaves out a run that ended because the endpoint
  didn't answer, as the report does; before, it counted as a failure, in
  `--compact` mode too.
- Nothing changes in KodAct itself: this is the Terminal-Bench runner, and the
  numbers every KodAct change is judged by.

### Harbor: deep-thinking runs give the model a 65,536-token reply limit
- **`KODACT_HARBOR_MAX_OUTPUT`** gives the run's model a reply limit, passed
  to `kodact` as `modelLimits` through `--settings`, and the job name gets
  `-maxout<N>`. It defaults to 65,536 for `deep-thinking`: that alias is
  served by vLLM, which bounds a reply only by its window and states no
  reply limit, so KodAct kept its 32,000 default and cut a reply that was
  still reasoning at 32,000 (three times in the run of 2026-11-15). Other
  models get none unless it is set; `0` gives none; a `--settings` of the
  person's own in `KODACT_HARBOR_ARGS` wins (`max-output.sh`).
- Nothing changes in KodAct itself: this is the Terminal-Bench runner.

### A model's reply limit is learned from its endpoint
- **A refusal that names the limit is learned.** An endpoint that caps
  replies below KodAct's 32,000-token default refused every request, and the
  session ended at the first one ("this model supports at most 8192
  completion tokens", "the valid range of max_tokens is [1, 8192]"). KodAct
  now reads the limit from the refusal, in OpenAI's, the Messages API's,
  DeepSeek's and Groq's words (also as LiteLLM passes them on), uses it for
  every later request, and sends the refused one again
  (`system/output_limit_learned`).
- **A stated limit above the default is used.** A reply limit from
  `modelLimits`, the model list (`max_output_tokens`, `max_completion_tokens`,
  OpenRouter's `top_provider.max_completion_tokens`) or LiteLLM's
  `/model/info` could only lower the cap: a model listed at 65,536 was still
  cut off at 32,000, and setting `maxOutputTokens` above 32,000 did nothing.
  It is now the cap, still fitted to the room the window has left.
  `KODACT_MAX_OUTPUT_TOKENS` still wins; the table's models and models nothing
  is known about keep 32,000. `/model/info` is now also read for a model
  whose window is known but whose reply limit isn't.
- **What it changes in real use.** Pointing KodAct at DeepSeek's own API,
  Groq or a small hosted model no longer fails on the first request until
  `modelLimits` is set. A gateway that states a long reply limit gets it. A
  server that bounds replies only by its window (vLLM, as behind the
  deep-thinking runs) states no reply limit, so nothing changes there on its
  own; `modelLimits` now raises the cap for it, which it couldn't before.
- **What it costs.** Nothing on a turn where the endpoint states nothing. A
  higher stated limit lets a reply run longer before it is cut off, a reply
  that only reasons included; the thinking time limit
  (`nudges.thinkingTimeBox`) bounds that by time.
- **Measured by** `evals/context-window/`, cases `max-output-*`, with the real
  binary: the build before this passed 3 of the 7 (it asked 32,000 where
  65,536 was listed, and ended with an error on both refusals); this one
  passes all 7, and the suite's 62 cases.

### A reply cut off while reasoning gets its reasoning back, sized to the window
- **The handback comes from the model's limits.** Where reasoning isn't sent
  back (chat completions), a reply cut off at the reply cap, stopped by the
  thinking time limit, or ended with no answer got 8,000 characters of its
  reasoning back: the first 2,000 and the last 6,000. On a 524k window with a
  32k cap, that was under a tenth of a 70,000 to 110,000-character analysis.
  The middle, with the findings, was lost, and the next reply spent minutes
  working them out again and was cut off again. The handback is now half the
  room left before compaction, at least a quarter of the reply cap (32,000
  bytes on a 32k cap) and at least 8,000 bytes, but never more than that room
  holds: a handback that set off compaction would be summarized away before
  the model saw it. It is measured in bytes, as the context estimate is, so
  reasoning in Chinese (three bytes a character) fits the same room. A large
  window gets the whole reasoning back; a nearly full small one gets what
  fits, from its start and its end.
- **Nothing of it is lost.** Whenever anything is left out (what doesn't
  fit, or a repeat), the whole reasoning is saved in the session's spill
  directory, where long tool output already goes and which is read without
  a prompt, and the reminder names the file to Grep or Read instead of
  working it out again; its long lines are wrapped, since Read cuts lines
  past 2,000 characters. Sub-agents now save
  there too, their long tool output included, and each file name is claimed
  with an exclusive create, so two writers never share one.
- **Repeats come back once, and drafts stay whole.** A paragraph the
  reasoning repeats word for word is shown at the last place the reminder
  shows it, so a revised draft keeps all of its parts; where an earlier copy
  was, a short marker says it is repeated below. A second cut-off in a row
  doesn't hand back again what the first one gave, which is still in the
  conversation: a marker says it was handed back above. Code inside fences
  always comes back as written, everything kept comes back byte for byte,
  and a cut falls at a line start.
- **Reminders in a row count as one turn** for micro-compaction, which keeps
  the tool results of the last three turns: a few act reminders in a row
  had pushed the person's current turn out, and its file reads were cleared
  just as the model was told to act on them. A reminder between tool rounds
  still marks a turn, as unattended runs (one message of the person's) take
  their turns from those.
- **What it costs.** A turn with no such reply: nothing. When it fires, the
  reasoning handed back stays in the conversation until compaction, up to
  about one reply cap of tokens per reminder (three in a row for cut-off or
  empty replies, two more for the thinking time limit), sent again with each
  later request, where re-deriving it cost as many output tokens and minutes
  of generation.
- **Measured by** `scripts/act-after-cutoff.py`: for each handback in a run's
  stream, whether the next reply acted, answered, or reasoned to the limit
  again, whether the saved file was read, and how much came back. Task
  `reasoning-cutoff-resume` caps the reply at 6,000 tokens and sets a
  20,000-token window, so a reasoning model is cut off mid-analysis and part
  of its reasoning comes back with the rest saved, and passes only if the
  reply after the handback acts or answers. kodact-eval now flushes a run's
  stream before its check, so a check can read it (`../transcript.jsonl`).
  The before/after comparison on a reasoning model is still owed.

### Harbor runs on a slow network: Python and Torch from disk
- **The usual command is enough.** `KODACT_HARBOR_JOBS=6 evals/harbor/run.sh
  subset -l 89` makes the missing caches before it starts, once, and says so:
  - Torch for the agent.
  - Python's apt packages.
  - The tests' packages for each task whose tests, in a past job here, judged
    nothing (they ran out of time, or failed to fetch what they need).
  Later runs reuse them, and `KODACT_HARBOR_WARM=0` makes nothing. The tests'
  cache, gigabytes, goes only into the containers of the tasks it was made
  for.
- **Harbor's own limits for a slow network get 3× the time:** the container
  start (four containers took longer than 600 s with 6 trials at a time), the
  agent's setup (copying the caches in) and the tests (torch-tensor-parallelism's
  ran out of their 900 s). The agent's limit stays as the task sets it.
  `KODACT_HARBOR_TIMEOUT_MULTIPLIER` changes the 3, `1` keeps Harbor's limits,
  and a flag given to the run script wins.
- **Python from a local apt cache.** When a task's container has no Python
  (torch-tensor-parallelism's Ubuntu 24.04), the agent runs `apt-get install
  python3 python3-pip`. Through a slow proxy that took 10 of the task's 14
  minutes, leaving no time to test anything. `warm_apt_cache.py` now
  downloads those packages once per release. Each agent copies the folder for
  its container's release in and points apt's downloads at it, out of reach
  of the Docker images' `docker-clean` hook, which empties the default folder
  after every `apt-get update`. Checked on Ubuntu 24.04: the install then
  needed 0 B from the network.
- **Torch wheels for the Pythons the containers have.** `warm_pip_cache.py`
  fetched wheels only for this machine's Python, and pip ignores wheels for
  any other version. It now fetches them for 3.12 (what Ubuntu 24.04's apt
  installs) and 3.13 by default (`--python` picks others).
- **The caches are used whenever their folder exists.** The run scripts no
  longer need `KODACT_HARBOR_UV_CACHE`, `KODACT_HARBOR_PIP_CACHE` or
  `KODACT_HARBOR_APT_CACHE` exported: each defaults to its folder in
  `~/.kodact-harbor`, and `off` turns one off. In every case the task still
  withholds the packages, so the agent still installs them itself; only the
  download speed changes, for every agent alike. Jobs get `-uvcache`,
  `-pipcache` and `-aptcache`.

### Harbor runs behind a proxy: tests that judge instead of failing to start
- **The tests always have uv.** Terminal-Bench test scripts download uv
  before testing anything. When that download failed ("Could not resolve
  host: releases.astral.sh"), the result was `uvx: command not found`, and
  the trial was a verifier error that judged nothing. After the agent's run,
  the agent now puts this machine's uv where the installer would (`uvx`, the
  `env` file). It skips this if the container already has a uv, or if this
  uv doesn't run there. `KODACT_HARBOR_UV_BIN` picks the binary, and `none`
  turns it off. The agent never sees that uv: it is added after its run.
- **The Hugging Face mirror is chosen at the start, only when needed.** With
  `KODACT_HARBOR_HF_ENDPOINT` unset, each run script now checks huggingface.co
  through this machine's proxy. If it is blocked and hf-mirror.com answers,
  the agent and the tests use the mirror, the run says so, and the job name
  gets `-hfmirror`. Before, a blocked huggingface.co made `reshard-c4-data`
  fail in every run unless the variable was exported each time. `off` never
  uses a mirror, and a URL picks another. The mirror is run by a third party:
  only public data goes through it, and no Hugging Face token reaches the
  containers.

### "a or b" asks for one file, not both
- **`verification.missing` no longer lists the other alternative**: a request
  naming "`requirements.txt` OR `pyproject.toml`" got both files listed as
  missing until both existed, so a run that made one (and was right) said a
  file was missing, and a script gating on `.verification.missing == null`
  rejected it. Files named as `a or b` (`either a or b`) are now one
  deliverable: either existing is enough. `/goal`'s check and the opt-in
  `nudges.constraints` note read the same list (task `either-deliverable`).

### Words aren't files, and a file named alone is the one in the project
- **`and/or`, `input/output` and other words joined by a slash are no longer
  read as files.** A word with a slash counts as a path only when it reads
  as one: it exists, starts at a root (`/app/x`, `./x`, `~/x`), ends with `/`,
  or ends with a file name (`out/plan.jsonl`). Before, `verification.missing`
  listed such words. With `nudges.constraints`, the model was also told "It
  names files that don't exist" and to create them. In a full Terminal-Bench
  run on 2026-11-14, four tasks' results listed words such as
  `missing/duplicate`, `cost/latency` and `ceil(prompt_len/64)*64`.
- **A file named alone is the one elsewhere in the project.** "Fix
  parser.py" means `src/parser.py` when there is no `parser.py` at the root.
  Before, it was listed missing, the model was told to create it, and edits
  to the real file didn't count as progress. The search skips hidden folders,
  virtual environments, installed packages and build folders, so pip's own
  `main.py` isn't the one a request asks for. It stops after 20,000 entries.
  Task `named-elsewhere`, with a scripted model: before, `members.py`,
  `and/or` and `missing/duplicate` were listed missing in 3/3 runs, and the
  constraint reminder cost a turn. After, nothing is listed.
- **`watch.py` says `verifier error` where `report.py` does.** A trial whose
  tests couldn't judge the work (they couldn't fetch their data, or their
  tools couldn't run) now shows `reward 0 · verifier error` with the line
  that shows why, instead of `✘ failed`. Like the report, the running score
  leaves it out. A package database the agent left broken shows `✘ failed:
  left dpkg interrupted`.

### `verified` means a check that could have failed
- **The Finish tool asks for `verified: true` only when a check run after
  the last change would have failed had the work been wrong**, and that check
  is named in the summary, so a reader can judge the claim. A check that
  passes whatever was chosen (a sample every option handles alike, a test of
  a part that didn't change) verifies nothing: the model is asked to set
  `false` and name its guess in the summary. In 8 pytorch-model-cli runs, 7
  claimed `verified` after checks that passed whatever input scaling had been
  guessed; the 3 whose hidden test ran all failed it. A script reading
  `finish` sees the guess instead of a false "done". The scores don't change:
  the guess is still a guess. Task `finish-guess`.

### Stopping an install without breaking the machine, and what a stop left
- **An install or removal KodAct stops is sent SIGINT, not SIGTERM**: apt,
  dpkg, apk, pacman, dnf, yum and zypper, read from the command line or, on
  Linux, from the programs running in the command (`./setup.sh` that runs
  apt-get). It finishes the step it is on and ends with its database
  consistent; nothing forces it. Measured on a real apt install stopped
  mid-configure through KodAct's own stop (apt 2.8.3, dpkg 1.22.6): the
  previous stop left dpkg's journal unmerged and the packages half-configured,
  so every later install failed with "dpkg was interrupted", for the person as
  much as the session; SIGINT left them installed. apt runs dpkg in a session
  of its own, so the process-group SIGKILL never reached dpkg anyway: it died
  later, mid-run.
- **KillShell on an install** waits up to 10 seconds and says whether it ended
  (packages it hadn't reached are not installed) or is still finishing (the
  model is told when it exits, and not to start another install meanwhile).
  KillShell again stops it at once and names the repair. `/tasks stop` works
  the same way.
- **An interrupt or `--max-time`** stops an install in a call the same way,
  waits up to 5 seconds, and leaves one still finishing as a background shell.
- **At the end of the session** KodAct waits up to 10 seconds for an install
  asked to stop and tells you on stderr what was cut short: an install that
  ended, or one still finishing, with its process ID and the repair if you
  stop it by force. Ordinary commands are stopped silently, as before.
- **Output that says something is left broken or locked gets a note**, read
  from the text whatever the exit code (`apt-get … | tail` ends 0): "dpkg was
  interrupted" (repair before going on, `dpkg --configure -a` then
  `apt-get install -f`, and the stop that caused it when this session made it),
  a package lock held by one of the session's background installs (wait for
  it) or by another program, git's "Unable to create '…index.lock': File
  exists" (stale: remove that file; or held by a running git command of the
  session) and pacman's `db.lck`.
- **A stopped git command that left its lock files** is named with them. git
  removes its locks on SIGTERM and SIGINT (measured); SIGKILL, an
  out-of-memory kill or a crash leaves `index.lock`, and every later git
  command that writes fails until it is removed.
- Task `stopped-install`: a stand-in apt with apt's handling of signals, an
  install the person asks to stop midway, and a check that the package
  database is consistent at the end.

### Stopping a package manager mid-install is said, with the repair
- **KillShell on `apt-get install` names what it may have broken**: a
  package install that ran past its wait moves to the background, and a model
  that then stopped it left dpkg half-configured: every later `apt-get
  install` fails with "dpkg was interrupted", on the person's own machine too,
  until it is repaired by hand. When KodAct stops a command that was installing
  or removing packages (apt, dpkg, apk, pacman, dnf, yum, zypper), the result
  now says so and gives the repair (`dpkg --configure -a`, `apk fix`, the stale
  pacman lock). A command that only reads (`apt-get update`, `apt list`), one
  that had already ended, and any other command get nothing added.
- `evals/harbor/report.py`: a trial whose tests could not start because dpkg
  was left interrupted is `left dpkg interrupted` (the agent's failure, still
  a valid trial), no longer a "false success"; tests whose own tools were
  missing (`uvx: command not found`) are a verifier error.

### The result no longer says a check passed
- **`verification.status` is gone from the result** (`passed`, `failed`,
  `unchecked`, `read_back`, `no_changes`, with `command` and `files`). In
  recorded runs it said `passed` on work that was wrong: a check piped into
  `tail` or followed by `; echo done` ends 0 whatever it did, and a check that
  does pass may cover one case of many. A script that read it should run its
  own check after `kodact` exits. `verification.missing` (files the request
  asks for that don't exist) stays, and the field is absent when nothing is
  missing. `kodact-eval`'s per-run "Changes" column is now "Missing files".
- **`finish.verified` isn't a gate either**: an unattended run that ends with
  the Finish tool records the model's claim in `finish`, and a model can claim
  `verified` for work that fails. The docs gate on your own check, not on
  `jq -e '.finish.verified'`.
- **Inside KodAct, a check whose exit code the command hides counts as run,
  not as passed** (`pytest | tail -20`, `make test; echo done`,
  `./run || true`, a check in a loop or in the background; `set -e` and
  `set -o pipefail` in the same command are honoured). Nothing is said to the
  model about it. It changes two things that acted on a pass: `/goal`'s check
  is told the outcome isn't known, and the passing-version snapshot
  (`nudges.passingSnapshots`) isn't saved from such a run (task
  `verify-long-report`).

### A server in the sandbox, and edits to CRLF files
- **A server started in the sandbox isn't called reachable**: on Linux, a
  sandbox without network access gives each shell command a network of its
  own, so a server started in one answers nothing outside it: not the next
  command, a browser, or a check run after the session. A background start
  there now says so, and the reminder an unattended run gets about commands
  still running names the start outside the sandbox
  (`dangerouslyDisableSandbox`, which asks for approval) instead of `nohup`
  alone, and asks for a plain report when that is refused (task
  `serve-after-exit`). Unsandboxed, with the network allowed, or on macOS,
  nothing changes.
- **MultiEdit on a file with CRLF line endings**: a block of several lines
  copied from Read never matched, and a replacement of several lines left the
  file with mixed endings. MultiEdit now matches and writes as Edit does (task
  `crlf-blocks`).
- `docs/PARITY.md`: Skill, the MCP resource tools and fast-mode pricing were
  listed as missing and are built.

### Signed thinking survives an edited history (N1)
- **Long sessions on models that keep their thinking no longer end in a 400
  at the first micro-compaction.** On those models a thinking block's
  signature covers everything before it. KodAct changes earlier history on
  purpose: micro-compaction clears old tool results, `/rewind` summarize
  replaces a range, and a resumed session gets a new system prompt (today's
  date, the current git status). KodAct now records what each signed block
  was written on and, before each request, leaves out the blocks whose
  history changed. The words and tool calls of those turns stay, and blocks
  written after the change go back as usual. The records are kept in the
  transcript, so `--resume` and `--fork-session` check the same way. Tests:
  `crates/kodact-engine/src/tests/prefix.rs` (the checker, the ordinary
  path, and the three cases that changed history),
  `crates/kodact-engine/src/prefix.rs`. Eval task `compaction-then-resume`
  (compaction at a 100k window, then a kill and resume). Not yet run against
  the real API: that needs a key.

### The pacing nudges are removed
- **`progress`, `progressEarly`, `pace` and `backOff` are gone.** In the runs
  that measured them, the pacing nudge spoke in 2 of 7 runs on a small model,
  with no difference it caused, and 3 times in 15 Terminal-Bench runs: twice
  in runs that passed, on a task that passes about 4 times in 5 without it, and
  once in a run that then failed. What it said ("make the change now") is
  advice the model can give itself, not a fact it lacks (docs/goal.md rule 8),
  and without a time limit it would have told a model to stop looking into a
  question after 15 steps. Nothing changes by default: all four were off.
  Their names now get the unknown-name warning in `KODACT_NUDGES` and are
  ignored in settings. The nudges that carry a fact stay, and what counts as
  progress stays for `adaptiveEffort`. `kodact-eval` reports "Nudges per
  task".

### What KodAct says it checked
- **Running the program a compile built is a check**: a build-and-test
  command that failed on a bug in its own test code stayed the last check,
  failed, though the model then ran the program cleanly, so KodAct reminded
  it to fix what wasn't broken; and a comparison script that failed after a
  passing build went unseen, the build being the last check. A run of a
  program built in the turn now counts: by its path, from inline code that
  starts it by its path, or from a script that does, and where its exit code
  is the command's own it is a pass (one hidden behind `| tail` or `; echo`
  is "ran"). Only while the program is newer than its sources and the turn's
  edits: a build that failed, or one not redone after an edit, is stale and
  checks nothing (task `verify-built-program`).
- **`report.py` doesn't blame the agent for a test that couldn't download its
  data**: a test that fetches its data in its own body (MNIST in
  `pytorch-model-cli`) and failed on the network was a `false success`, because
  pytest had started. Now a run whose every failed test's error is a network
  failure, and none an assertion, is a `verifier error`, out of the valid-run
  count; the network signatures also cover "Name or service not known", "Name
  does not resolve" and a failed `urlopen`.
- **A tool asked about itself isn't a check**: `gcc --version`, `pytest
  --help`, `make --version` or `gcc -v` after an edit counted as testing it,
  so the reminder about untested changes stayed silent and the result said
  `passed`; a compiler given nothing to compile was also taken to have built
  `a.out`. They are probes now, and change nothing.
- **The result names the files never made**: `verification.missing` lists the
  files the request asks to be made that don't exist when the turn ends, beside
  the status and never changing it (which files are asked for is read from the
  request's words, a guess). `/goal`'s check is told them too, and
  `kodact-eval` records them per run.
- **A renamed or removed file isn't called missing**: the end-of-turn
  constraint note listed a file the request asked to rename ("Rename helper.py
  to utils.py") as missing, asking the model to make it again. Only files that
  weren't there when the turn started count (task `rename-file`).
- **"Form", "platform" or "confirm" don't make a sentence a removal**: the
  command `rm` was matched as letters, so "the compressed form of data.txt" or
  "transform" dropped every file the sentence named. `rm` now counts only as a
  word of its own, and the removal words by their stem, so every form counts
  ("deleted", "deleting", "removal", "cleaned up", "got rid of"). Markup
  (`</system-reminder>`) is no longer read as a file either.
- **`np.float64` isn't a file**: a dotted code name had a file name's shape
  (`np.float64`, `self.data`, `math.log`, `app.registry.route`), so the
  constraint note said it was missing and asked the model to write it. A bare
  `name.ext` now needs evidence: the file exists, its extension is a common
  file type, a word just before it asks to write it (`Write me data.comp`), or
  it is where a sentence that writes files puts something (`... and the
  process ID to server.pid`). Of the 52 eval tasks' requests, 5 read
  differently, each losing only names that weren't files.
- **`watch.py`**: the end says what KodAct's last check ran (`KodAct's last
  check passed: $ gcc -O2 -o app app.c`) and which files the task asks for
  don't exist; a step already under way when the watch starts is timed from the
  log's last write, read from the container, or says `over` when that can't be
  told; and a trial that starts while watched shows its header before its first
  step.

### A placeholder is not written in place of the code it stands for
- Write over an existing file, Edit and MultiEdit refuse, once, a call whose
  new text adds a line such as `// ... rest of the code ...` or
  `# ... existing code ...`: written as it is, that line would replace the
  code it stands for and nothing would fail until something ran. The refusal
  names the line and nothing is written. The same call sent again goes
  through, for a file meant to hold such a line; a new file, and a line the
  replaced text already had, pass. English wording only: elsewhere nothing is
  said. No model call and no text on a turn without such a line. Pattern from
  Gemini CLI's source (docs/REFERENCES.md); no eval task provokes it yet, so
  it is covered by unit tests only.

### Practices read in other agents' prompts (docs/REFERENCES.md)
Eight practices KodAct lacked, each written in KodAct's own words. No
before/after has been run yet (no API key here): every one is owed.
- **Checks sized to the change** (prompt): the narrowest check that would
  show the change is wrong comes first, the full suite when the change
  reaches shared code; a wording change or an answered question needs none.
  Before, the prompt asked for the tests, type checker, linter or build after
  any change.
- **Edit from a search result** (`edit.withoutRead`, off by default): Edit
  changes a file never read when the call holds its exact text, so a one-line
  fix found with Grep doesn't cost a Read first (task `edit-from-search`).
- **A question is answered, not acted on** (prompt; task
  `question-leaves-code`).
- **Changes KodAct did not make are left alone** (prompt; task
  `leave-other-changes`).
- **Look before deleting or overwriting**, and a go-ahead for what can't be
  undone or reaches outside the machine (prompt).
- **The compaction summary names the approaches given up**, and why, so they
  are not tried again after it.
- **A long conversation is not a reason to stop early** (prompt): the model
  is told the earlier part gets summarized and the work continues.
- **A saved model call is worth more than a smaller result** (prompt): ask a
  search for the lines around its matches when that spares a Read.
The system prompt grows by about 1,500 characters (roughly 370 tokens) on
every request.

### Nudges that stay out of the way (docs/goal.md rules 7 to 10)
- **A quiet check**: `scripts/quiet-check.sh` runs three everyday tasks (a
  question about the code, a one-line fix, a rename; tag `quiet`) with
  KodAct's reminders off and with the configuration under test, and compares
  tokens, model calls and time. On work the model does alone, the difference
  is what KodAct costs when it has nothing to say.
- **A command in the request counts only when the request asks to run it**
  (`constraints`, `namedChecks`): "run `make test`", "make `make test`
  pass", "verify with `pytest`", "so that `x 1` prints `y`", or a check said
  to fail. One only mentioned ("see `make deploy`"), set aside ("avoid",
  "without", "instead of", "except") or ruled out is never suggested, and a
  request that isn't in English is not read at all rather than misread.
- **`backOff` keeps the nudges that work**: a pacing nudge followed by
  progress within two model calls was acted on and is kept; one followed by
  progress only later wasn't needed, and twice silences it. Notes that carry
  a fact (a missing file, an unrun check, a limit passed) never back off.
  Before, any progress after any nudge counted toward silencing it.
- **One message at the end of a turn**: the notes due as a turn would end
  (open todos, the constraint report, an unsaved result, a passing version,
  unchecked changes, no attempt yet) go together in one message, one model
  call, instead of one call each. The unsaved-result note also loses the
  stray runs of spaces in its text.
- **Waiting isn't stalling**: time spent waiting on a tool (a build, a
  download, a test run, a background command) is left out of a pacing
  stretch's share of `--max-time`, so a long command the model couldn't
  shorten brings no "no progress" nudge.

### Watching a Harbor run, as a chat
- **`evals/harbor/watch.py` shows a job from its start and then live**, with no
  flag: each trial as a chat under its task, time limit and instruction. Every
  step is numbered, with its output tokens, the context it read and, live, how
  long it took; the start and end of the reasoning; each tool call in full (the
  whole command, the start of a file written, the lines an edit changes); each
  result under its call (exit code, first and last lines of output, red when it
  failed); KodAct's reminders and events in words; how the run ended, KodAct's
  own check, and the reward with the score so far. While the model thinks or a
  command runs, a line every minute says which and for how long. `--new` follows
  only what comes next, `--once` shows a job and stops, `--task` picks tasks,
  `--compact` keeps the old one-line view.

### Thinking without acting, and benchmark runs that say what they ran
- **`nudges.thinkingTimeBox`** (opt-in): a reply still all thinking past a
  limit taken from the run's own pace (3× its usual reply, at least 2
  minutes, at most a quarter of the time left under `--max-time`) is stopped;
  its reasoning is handed back and the model asked for one concrete step. A
  reply that has started its answer or an action is never cut.
- **`adaptiveEffort` bounces back**: at `low` and still getting nowhere, the
  level returns to yours, since thinking less didn't help either; a hard
  problem isn't left at `low`.
- **Summaries ask for the lowest reasoning level**, so a summary can't spend
  its whole 32,000-token budget thinking and come back empty.
- **Harbor**: job names show `KODACT_HARBOR_ARGS` (`-args-effort-low`), and
  each job asks the endpoint which model it serves behind the alias and its
  window, records both in the report, and sets `KODACT_CONTEXT_WINDOW` from it
  when unset.

### The reasoning level, as the endpoint takes it, and adapting step by step
- **A refused level becomes the closest one the endpoint takes.** Servers say
  which levels they take when they refuse one ("must be low, high, xhigh, max,
  or an integer within [1, 100] in chat_template_kwargs"): KodAct sends the
  closest, and remembers it for the model. `--effort medium` on such a server
  used to be dropped, leaving the model at its default; it now goes as the
  middle of its scale. Never `none` on your behalf: thinking stays on.
- **`nudges.adaptiveEffort`** (opt-in): the level follows the thinking. A reply
  that was all thinking, or two steps of long thinking that made no
  progress, lower it a step; two steps of progress raise it back. Never above
  your level, never below `low`. The server enforces it, so a model can't
  think past it whatever it decides. Off until measured.
- **The window from vLLM's other words**: `max_model_len=524288` (a reply
  asked for that is larger than the whole window) and "maximum model length
  of 524288" (a prompt that is) are learned like "maximum context length is
  N", and the request goes again sized to fit.

### Small-window reasoning models behind vLLM: runs that ended, and time lost
Seen on a 100,000-token reasoning model behind LiteLLM and vLLM, in runs that
ended on an error or ran out of time:
- **A refusal for length whose prompt fits is sent again with half the
  reply** (down to 4,096), for the conversation's requests and the summary's,
  before anything is compacted. vLLM states the prompt as "at least" the
  window less the reply asked for, plus one, so a request sized to that was
  refused again by the same margin, every time.
- **One turn compacts again each time the conversation overflows after
  growing.** A `-p` run is one turn: the second overflow used to end it.
- **A reply cut off while it was all reasoning gets no larger output cap.**
  It is asked to act on what it has; the doubled cap let its next pass reason
  twice as long first.

### Reasoning level for OpenAI-compatible endpoints
- **`--effort` reaches an OpenAI-compatible endpoint as `reasoning_effort`**,
  for every model, one KodAct has no table entry for included. Before, only
  Gemini models got it, and a model KodAct didn't know lost the effort
  altogether, so a slow reasoning model behind a gateway couldn't be told to
  think less on a time-limited task. `xhigh` and `max` are sent as `high`. An
  endpoint that refuses the parameter gets the request again without it. No
  effort set: nothing sent, as before.

### A stated size limit, told at the write that breaks it (`nudges.constraints`)
- A file the request limits in size ("must be <5000 bytes") that a write takes
  over the limit is told with that write's result, with the size, the limit
  and how far over. Before, only the end-of-turn report said so, which a run
  cut by its time limit never reaches.
- Size limits read better: `<5000 bytes` and `< 5000 bytes` count; "under",
  "less than", "below" and `<` are strict (4,999 for `<5000`); and a limit
  whose sentence names no file is the one new file's the request asks for, or
  the one of the type the sentence names ("your c program": `gpt2.c`).

### Harbor behind a proxy that blocks Hugging Face
- **`KODACT_HARBOR_HF_ENDPOINT`** (opt-in): the tests and the agent fetch from
  another Hub address (`HF_ENDPOINT`, with Xet off), with the CA bundle when
  `KODACT_HARBOR_CA_BUNDLE` is set. On a network whose proxy blocks
  huggingface.co, `reshard-c4-data` is then checked instead of failing before
  its first test. Jobs get `-hfmirror`. A mirror is a third party: public test
  data only, never a default.

### Writing a file the session made itself
- **Write and Edit no longer refuse a file created during the session**
  because it wasn't read first: a file made by a shell command
  (`cat > solver.py`) or by a script the model ran can be rewritten at once,
  instead of costing a Read step. Files that existed when the session
  started, and files put back by a checkpoint restore, must still be read
  first, and any file read and then changed by something else must be read
  again, as before. Where the file system records no creation time, the rule
  is as strict as it was.

### KodAct's own name on the wire
- **Model API requests send `User-Agent: kodact/<version>`** (was the old
  name, `forgecli/<version>`). A gateway or proxy that filters or logs by
  User-Agent sees KodAct; WebFetch already sent `KodAct/<version>`.

### A network drop doesn't end a script
- **`kodact -p` retries an endpoint it hasn't reached yet**, 3 times over
  about half a minute: a failed name lookup or a VPN reconnecting no longer
  ends a scripted or eval run at its first call. At the prompt it is still
  said at once, since there it is most often a URL to fix.

### Measurements held against the request's limits (opt-in)
- **`nudges.limitVerdicts`**: when the request states numeric limits (a table
  of thresholds per file, `error_rate <= 0.01`, `at most 6 slot sizes across
  both files`) and a command prints `key=value` numbers, the result says, line
  by line, which limits are met and by how much the others are not. A limit
  across all files together is said apart, since per-file counts don't show
  the combined number. Limits, names and files all come from the request's
  own words.
- **`nudges.staleDeliverables`**: a printed result met every limit, and a few
  steps later the files the request asks for still haven't changed: the
  model is told to write it now if it is the best so far. Off by default
  until measured.

### Failed commands explained, and built programs run
- **`command not found` (exit 127) names what is installed instead**, by job:
  `xxd` missing → `od`; `python3` missing → `perl`; `jq` missing → none of
  `jq`, `python3`, `node` either. The model uses one at once instead of
  probing for tools one call at a time (minimal containers, fresh boxes).
- **Exit 137 says the command was killed** (SIGKILL), which, unless you
  stopped it, is almost always the system ending a process out of memory, so
  the same command isn't run again as is.
- **A program built and never run is run once before finishing**: when the
  last passing check compiled a program (`gcc -o image image.c`, a plain
  `gcc x.c` making `a.out`, `rustc`, `go build -o`) and nothing ran it since,
  the verification check asks once to run it and look at its output.
  Building and running in one command, or running the tests, needs nothing.

### The "to build" list in GOALS.md
- **Notes that outlast the context** (the Notes tool): given back after
  every compaction and kept in `<session>.notes.md` next to the transcript.
- **Checkpoints the model goes back to itself** (the Checkpoint tool):
  `save` before an approach that may fail, `restore` to put the files back
  as they were. In a git repository a checkpoint is the whole working tree,
  shell changes included, kept under `refs/kodact/checkpoints/` without
  touching the index or a branch, and it survives a resume.
- **Formatting after edits**, opt-in (`verification.format: true`): the
  edited file is run through its formatter and the result says what moved.
  Only a file that was already in the formatter's style is touched.
- **The language server tells only the errors an edit added**
  (`verification.languageServer`): the file before the edit is the baseline.
  A file a shell command rewrote is checked too, when the session had
  already had it checked.
- **Errors that say the next step:** a missing path names the nearest
  directory and the close names in it; an empty Glob or Grep says what was
  searched and the usual cause. LS and NotebookEdit return structured
  results, as every other tool does.
- `scripts/eval-descriptions.sh` measures a set of tool descriptions against
  the built-in ones. First pass recorded in
  `evals/baselines/2026-11-11-tool-descriptions/`: two candidates, neither
  better than noise, none adopted.
- **A missing path names a close file elsewhere in the tree** (another
  folder, another case), not only in the nearest directory.
- **A wait stated only in an error's body is honoured** ("Please retry in
  1h36m48s", `retryDelay`), where an endpoint sends no `retry-after` header:
  a spent daily quota is no longer retried after a few seconds.

### Three gaps closed from a source comparison with other harnesses
- **A session killed mid-turn says so when resumed.** The transcript records
  where each turn starts and ends. Resuming one whose last turn never ended
  tells the model which tool calls returned nothing and to check the real
  state before continuing.
- **Sub-agents can work in their own worktree.** `isolation: "worktree"` on
  a Task call gives the agent a copy of the last commit on a new branch, so
  agents changing files in parallel can't overwrite each other. The result
  names the branch when something changed.
- **Edit by line range** (`edit.lineRanges`, off by default). Edit can name
  the lines to replace, with the first line's text as an anchor, instead of
  retyping them.

### Checks on the agent's work, second round
From `docs/VERIFICATION_COMPARISON_2.md`.
- **Language-server diagnostics after edits**, opt-in
  (`verification.languageServer: true`): the errors `rust-analyzer`,
  `typescript-language-server`, `pyright-langserver` or `pylsp`, `gopls` or
  `clangd` report for the edited file's new text are added to the edit's
  result, other files' included. Off by default; a missing or slow server
  adds nothing.
- **A fallback model takes over on a limit that doesn't clear**: after three
  rate limits waited out in a row, on a `retry-after` over a minute, and on a
  spent quota, as it already did on overload.
- **The loop guard reminds again** at 5 and at 8 when the same call goes on.
- **A reply that repeats itself is stopped while it streams**, and the model
  is told, instead of running to the output cap.
- **A tool call written as text is run** (`<tool_call>…</tool_call>`, or a
  reply that is only the call's JSON), as small and local models write them.
- **`/goal` works with these checks:** its check is told what KodAct recorded
  of the turn's changes (unchecked files, the last check's outcome); a turn
  whose calls were all refused counts as a turn without a tool; a `-p` run
  with a goal gets no end-of-run todo reminder.

### Checks on the agent's work
From `docs/VERIFICATION_COMPARISON.md`, a comparison with four other CLIs.
- **A spent quota is not retried.** A 402, or a 429 or 403 that says the
  credit or spending limit is used up, is shown at once with what to do,
  instead of ten tries a minute apart. A plain 429 is still waited out.
- **A long `retry-after` is not shortened.** When the endpoint asks for more
  than a minute, KodAct says how long ("asks to wait 48m") and ends the turn.
  Unattended runs take the wait in full when it fits `--max-time`, or up to
  ten minutes with no limit.
- **The result says whether the changes were checked:** `verification.status`
  in the `json` and stream-json result is `passed`, `failed`, `unchecked`,
  `read_back` or `no_changes`, with the check's command or the unchecked
  files. A script can gate on it.
- **A file an edit breaks is said at once.** When a file no longer parses
  after an Edit, MultiEdit or Write, the edit's result says so, using the
  language's own parser when installed (Python, JavaScript, shell, Ruby, PHP,
  JSON). `verification.syntaxCheck: false` turns it off.
- **The loop guard stops a repeat it warned about.** After its reminder, the
  same failing call to a file tool is answered "Not run" with the last error
  until something changes. Shell, web and MCP calls are never refused.
- **`-p` runs don't end quietly with todos open.** A run that takes no further
  input is sent back once to finish its open TodoWrite items or say what is
  left.

### A new mark: from code to impact
- KodAct's mark is a step and a chevron, "from code to impact", and the
  front pages draw it, in KodAct's lime. The taglines say "From code to
  impact", and the wordmark's A is pointed. The logo files in
  `design/brand` are redrawn (the name in Outfit), and kodact.exe carries
  the new icon.

### Progress nudges, off until measured
Seven nudges that follow what the model does (never the task or the model's
name), each its own setting under `nudges` (or `KODACT_NUDGES`), all off by
default and all off with `reminders: false` (CLI.md, "Progress nudges"):
pacing by progress and its earlier form for budgeted runs, a parse check after
shell edits that break a file, a report of files and commands the request names
that aren't there, the request's own checks named by the verification loop,
passing versions saved with Checkpoint and reported (never restored), and
back-off for a nudge followed by progress twice. Each nudge is a `system/nudge`
event; `kodact-eval` reports nudges, tokens and tool calls before the first
write per run. Six eval tasks measure them.

Progress means the files the request names: when it names files that don't
exist yet, writing helper scripts is not progress, and a script that writes a
named file is (seen on disk). `nudges.pace` gives the time-limit warning the
steps left at the run's own pace, brings it earlier when about 5 are left, and
names the requested files that don't exist yet. Both come from a scheduling run
that spent 99.9% of its 29.5 minutes thinking, wrote five helper scripts, and
first wrote its two plan files in the last five minutes.

### Fixed: the pre-production pass on the terminal
Found by driving the real binary in a pseudo-terminal and reading the screen
(`scripts/terminal-matrix/run.py`; the report is
`docs/PREPRODUCTION_REPORT.md`):
- **Nothing typed, pasted or printed can command the terminal.** A paste
  holding escape sequences cleared the screen (it could switch screens or
  set the title), and a pasted tab left the prompt in pieces. Control
  characters are drawn as their pictures (`␛`, `␉`); what you send is as you
  typed it.
- **With the sidebar open (Ctrl+B), new prompts and answers appear.** They
  were sent and answered but never drawn until the window was resized.
- **SIGTERM, SIGHUP and SIGINT leave the terminal usable.** `kill` left it
  in raw mode on the alternate screen with the mouse captured. The UI now
  ends as `/exit` does, with exit status 143, 129 or 130.
- **A picker longer than the window scrolls.** `/model` on an endpoint that
  lists 40 models ran off the top and lost its title and filter. PgUp,
  PgDn, Home and End move in every picker and question.
- **The inline display survives a resize.** A smaller window left the input
  box, its border and the status line on one row.
- **`?`, `/status` or `/help` on the front page show their whole answer**,
  not its last six rows.
- **A queued message can be taken back:** Up on an empty prompt puts the
  newest one in the input.
- **PgUp in a short window no longer skips rows**; the saved `theme` and
  style apply from the first frame (`theme: none` drew colour); the status
  line's branch follows `/cd` and branch switches; `/keybindings` lists
  rebound keys on its two-action rows; a narrow front page no longer cuts
  its hints mid-word; `@` completion keeps top-level files in a project
  with a 25,000-file folder.
- **`/model one two` is refused** instead of saved as a model named "one
  two"; an id the endpoint doesn't list still switches, and says so.
- **A dead MCP server shows as failed** in `/mcp` (it stayed "connected"),
  and the tool error says `/mcp reconnect <name>`.
- **A session store that can't be written no longer stops KodAct from
  starting:** the conversation runs unsaved and says so.
- A plugin's command fills `${KODACT_PLUGIN_ROOT}` / `${CLAUDE_PLUGIN_ROOT}`;
  a 429 that ended the turn no longer says "waits and retries"; a 404 from a
  wrong base URL points at the URL, not only at `/model`.

### Changed (security)
- **A link out of the project is outside it.** A file read through a
  symlink that leads outside the working directories is asked about like
  any other path outside; deny and ask rules cover the folder a link leads
  to, and an allow rule on a folder does not cover what a link in it points
  to elsewhere.
- **A shell command that names a path outside the working directories is
  asked about**, read-only ones too: `cat ~/.ssh/id_rsa` ran unasked in
  every mode, plan and don't-ask included. Accept-edits no longer lets
  `touch ~/.profile` through. An allow rule (`Bash(cat:*)`) still allows.
  You will see a prompt where `ls /usr/lib` used to run.
- **The state directory is private (0700):** session transcripts, debug
  logs and feedback bundles were readable by other users of the machine.
- `--no-input` takes a value only as `--no-input=false`: `kodact -p
  --no-input "prompt"` took the prompt as the flag's value.

### Fixed
- `/clear` (and `/reset`, `/new`) left the old conversation on screen in the
  terminal UI: only the answer said it was cleared. Now the screen starts
  over, on the front page, with the conversation it left as the one to
  `/resume`. `/resume` and `/rewind` had the same fault and now show the
  conversation they moved to, as does starting with `--resume` or
  `--continue`, which showed an empty screen.

### On crates.io
- `cargo install kodact-cli` installs the `kodact` command (and
  `cargo install kodact-eval` the eval harness); the other `kodact-*` crates
  are published as its libraries. `/release-notes` shows this file there too.

### ForgeCLI is now KodAct
- The command is `kodact`, the crates are `kodact-*`, the settings are
  `KODACT_*` variables, `.kodact/` and `KODACT.md`, and the user folders are
  `~/.config/kodact` (state and cache likewise). Entries below say "Forge":
  they are from before the rename.
- Nothing set up before is lost: every `FORGE_*` variable still works (as
  its `KODACT_*` name, when that is unset); a project's `.forge/` and its
  `FORGE.md`, and the user's `~/.config/forge`, state and cache folders, are
  used until KodAct's own exist; `.forge/<kind>` resources are still read;
  `${FORGE_*}` placeholders in skills and plugins still fill in.

### Skills written for the reference CLI
- They load unchanged: Forge also reads `~/.claude/skills` (or
  `$CLAUDE_CONFIG_DIR/skills`) and the project's `.claude/skills` from the
  repository root down. The same goes for commands, agents and output
  styles. A plugin's `.claude-plugin/plugin.json` is its manifest too.
- A skill's text is filled in before the model reads it: `$ARGUMENTS`,
  `$ARGUMENTS[N]` and `$N` (from 0, with quoting: `/migrate "Button group"`),
  `$name` from `arguments:`, and the skill's, project's and plugin's folders
  (`${FORGE_SKILL_DIR}`, or `${CLAUDE_SKILL_DIR}` as such skills write it).
  Before, those reached the model as literal text.
- Frontmatter reads as there: `allowed-tools: Bash(git add *) Bash(git commit *)`
  is two rules, not five words; a skill without a description uses its first
  line; `when_to_use` joins the description.
- A skill's frontmatter now acts as there. Its ``!`command` `` lines and
  ` ```! ` blocks run first and their output takes their place (allowed by
  its `allowed-tools` or your rules; a deny rule or a failure stops it).
  For the turn it runs in, `allowed-tools` needs no prompt, `disallowed-tools`
  is hidden, and `model` and `effort` apply; its `hooks` stay for the
  session. `context: fork` runs it in a sub-agent (`agent:`) and brings back
  the report.
- Which skill runs: yours replaces a built-in command or a command file of
  the same name; names match regardless of case. Skills in a subfolder's
  `.claude/skills` appear once a file there is used, `--add-dir` folders'
  skills load, `paths:` holds a skill back until a matching file is used,
  and `skillOverrides` and `disableSkillShellExecution` are read. Loaded
  skills come back after compaction.
- Plugins: extra component paths in the manifest, and a `SKILL.md` at the
  plugin's root.

### Windows
- Commands run on Windows: Forge finds Git Bash (next to `git.exe`, the
  standard install directories, scoop), else PowerShell; `FORGE_SHELL` picks
  one. Hooks, `apiKeyHelper`, the status line and custom-command `!` lines use
  the same shell. The Bash tool's description and the environment prompt name
  the shell and how to write paths for it.
- No shell is a clear, non-retryable answer naming where Forge looked and the
  fix, a startup warning and a failing `forge doctor` row, instead of an OS
  error the model kept retrying.
- Timeouts, Esc and KillShell stop a command with everything it started (a job
  object). Paths are shown without the `\\?\` prefix. File tools and
  permission rules accept Git Bash paths and compare paths without regard to
  case, as the filesystem does. `forge doctor` finds `git.exe`.

### Gemini and gateways (OmniRoute)
- Gemini 3 tool loops work through OpenAI-compatible endpoints: each tool
  call's thought signature (`extra_content`, or a gateway's
  `thought_signature`) is kept and sent back on that call. Before, the
  second step of any tool loop failed with "Function call is missing a
  thought_signature".
- Through OmniRoute, tool-call ids go back exactly as issued and a
  message-level signature goes back on its message; Gemini's reasoning goes
  back as `reasoning_content`.
- Parallel tool calls an endpoint sends at the same `index` are kept apart.
- Gemini models get tool names, schemas and effort they accept: long MCP
  tool names are aliased, schemas trimmed to Gemini's subset, effort sent as
  `reasoning_effort`, `<thought>` tags read as thinking.
- A native Gemini wire: `FORGE_GEMINI_API_KEY=... forge -m
  gemini-3-flash-preview` talks to Google AI Studio directly;
  `FORGE_GEMINI_BASE_URL` points it at Vertex AI or a gateway. Thought
  signatures go back on the parts they came with, effort becomes Gemini's
  thinking level (or 2.5's budget), PDFs go as files, a malformed function
  call is asked again once, and Google's retry delay is honoured. `forge
  doctor` shows the Gemini endpoint and key.
- Switching to a Gemini 3 model mid-session (`/model`), or resuming an old
  session on one, no longer fails on the first tool call: calls Gemini
  never signed get Google's placeholder signature, and one model's
  signatures are never sent to another.

### OpenAI-compatible endpoints
- OpenAI's reasoning models (the o-series, GPT-5 and Codex) are sent
  `max_completion_tokens` and no `temperature`, also behind a gateway's
  provider prefix (`cx/gpt-5-codex`, `openai/o3` through OmniRoute,
  OpenRouter or LiteLLM). They refused `max_tokens` with a 400.
- A 400 that refuses a parameter Forge sets (`max_tokens` for
  `max_completion_tokens` or back, `temperature`, `stream_options`) is
  answered by sending the request once more as asked, and remembered for
  that model's later turns. A value refused ("max_tokens is too large") is
  left alone.
- When both length names are refused (a gateway that turns
  `max_completion_tokens` back into `max_tokens`, as OmniRoute does for some
  providers), the request goes without a length limit.
- Endpoints that stray from OpenAI's stream are read anyway: a turn with tool
  calls ends in tool use even when the stream says `stop` or nothing; a call
  without an id gets its own; arguments sent as a JSON object are kept (they
  were read as empty); and a whole JSON completion in answer to a streaming
  request reads as the stream would have ("stream ended without a message").

### Auth
- Each endpoint gets only its own key: `FORGE_API_KEY` and `FORGE_AUTH_TOKEN`
  never reach an OpenAI-compatible endpoint, and `FORGE_OPENAI_API_KEY` never
  reaches the Messages API. A key set for the wrong one is named at startup.
- Errors name the URL that answered and the variable the active endpoint
  reads: a 401 from an OpenAI-compatible endpoint points at
  `FORGE_OPENAI_API_KEY`, a 404 or connection failure at the URL variable in
  use.
- A refused key (401/403) exits with `3`, also mid-run and in JSON modes.
- `forge doctor` reports the provider, the URL and where it came from, and
  the key's source, masked; `forge doctor --probe` checks them with one
  request. `/status` shows the same.
- Quoted values are unquoted; an invalid URL or key, or a failing key helper,
  stops startup with a clear message. `openai.apiKeyHelper` is new.
- Project settings can no longer choose the endpoint or run a key helper.

### Models and context
- `modelLimits` (and `FORGE_CONTEXT_WINDOW`) set the context window and output
  cap of models Forge doesn't know; an OpenAI-compatible endpoint's model list
  is read for them too. Without either, Forge says once that it is guessing.
- Compaction measures the request about to be sent, not the last response: a
  batch of large tool results no longer pushes a turn, or a sub-agent, past
  the window. Servers that report no usage get an estimate instead of 0.
- Context-overflow errors are recognised in the wording OpenAI-compatible
  servers use (`maximum context length`, `context_length_exceeded`, ...), so
  the conversation is compacted and retried instead of failing.
- Costs of models without a known price show as unknown (`cost ?`, `$1.20+`,
  `price unknown` in `/usage`) instead of `$0.00`.

### Rate limits
- One limit on model requests in flight for the whole session
  (`maxConcurrentRequests`, default 4): requests wait for a slot instead of
  failing. A 429 halves the limit, says so, waits (honouring `Retry-After`)
  and retries; the limit grows back after a run of successes.
- The OpenAI-compatible provider honours `Retry-After` and
  `FORGE_MAX_RETRIES`, and stops waiting when interrupted.
- Unattended runs (`--autonomous`) don't end on an announced step: a reply
  without a tool call whose last line says what comes next ("Let me check the
  logs.") is sent back to do it, and a run that would end with TodoWrite items
  open is shown the list. Each at most twice a turn. The unattended prompt also
  says to install missing tools, to give long commands a longer timeout or the
  background (not `| tail`), and the tools prompt to fix a failed call's cause
  before retrying.
- Unattended runs (`--autonomous`) retry a model call that failed for a
  reason that passes (429, 5xx, overloaded, a lost connection, a stream that
  broke mid-reply) instead of ending the run: waits from 5 s, doubling, capped
  at a minute, up to 10 in a row, or with `--max-time` while a minute is left.
  Each retry is a warning and a `system/api_retry` event.
- `maxParallelAgents` (default 4) caps Task sub-agents running at once.
- A failed sub-agent's result starts with `FAILED:` and tells the model its
  task is not done, so it can't be reported as finished.

### Goals and side questions
- The `/goal` check treats the agent's own messages as claims: a `met` verdict
  must cite the tool output that proves it, failed tool calls and sub-agents
  are listed first so a long session can't hide them, and the final summary is
  marked as claims to check. It runs on the session's model (`goalCheckModel`
  picks another), not the small one.
- `/btw` mid-turn knows what is running: the tool calls (sub-agents included)
  that haven't returned, how long the turn has run, and the background tasks.
- On an OpenAI-compatible endpoint, WebFetch summaries and side requests use
  the session's model unless `smallFastModel` names one, instead of a model
  the endpoint may not serve.

### Fixes
- `/model` (and any picker or dialog taller than the input) no longer ends the
  session with "The cursor position could not be read": the terminal UI
  tracks its own position instead of asking the terminal while it reads keys.
- Injection notes: Forge's own Read notes ("Showing lines 1-2000 of N", "unchanged
  since you last read") are no longer flagged; a note appears once per file or
  page, reads calmly for local files, and isn't wrapped twice.
- After a denied tool call the model is told not to reach the same result another
  way (another command, shell or tool); after an environment failure, to stop
  and say what is missing. Replies are kept short for short messages.

### Terminal UI: Forge's look
- Forge's palette: every colour is a Pajamas stop from Forge for VS Code
  (brand purple, link blue, CI-style green, amber and red), with its own value
  for dark and light backgrounds. 24-bit where the terminal says it has it
  (`COLORTERM`, Windows Terminal, iTerm, WezTerm, VS Code), else the nearest of
  256, else the 16 ANSI colours; `FORGE_COLOR_DEPTH` overrides. The `auto`
  theme (now the default) follows `COLORFGBG`.
- Forge's F-and-cube logo opens a session, drawn in half blocks beside
  ForgeCLI's version, the model and where it runs, the directory and the keys
  to know, then Forge's welcome line. Narrow terminals get the small logo, or
  none; without colour it keeps its shape.
- With no endpoint set up, a first-run card (the logo, what ForgeCLI is, the
  servers it works with, the variables to set and what Forge found) replaces
  the bare error. It still exits with 3.
- The transcript is Forge's timeline: a tool call shows its turning blue dot
  while it runs, then goes to scrollback with a green or red dot and its
  result under `└`; paths are in link colour, commands follow a `$`. Answers
  have a quiet dot, prompts a purple `❯` on a raised surface; notices lead with
  `·`, `▲` or `✕`.
- The spinner speaks Forge (Forging, Hammering, Tempering…) with a shimmer
  sweeping the word; waiting for an answer is amber. The composer asks "What
  shall we forge today?". The status line starts with Forge's `▛◆` and shows a
  context meter (`━━━─────`) that turns amber at 70% and red at 90%.

### Terminal UI
- Results say what happened: `Read 40 lines`, `Updated calc.py: 1 addition,
  1 removal` with the changed lines in red and green, the first lines of a
  command's output. Permission dialogs for edits show the change; the spinner
  says `Waiting for your answer` while one is open.
- Tables are drawn with aligned columns; code blocks without their fences;
  wrapped lines continue under their text instead of at column 0.
- The header is one line (`ForgeCLI 0.1.0 · model · ~/project`); the cost reads
  `cost ?` from the start for a model without a known price.
- Two-column output (`/status`, `/doctor`, the shortcuts) wraps under its value
  column; a long path breaks in place instead of leaving a bare label.
  `/status` and `/doctor` show paths under the home directory as `~/…`.
- `/doctor` colours only its failed rows; an unknown model is one `note` row
  (guessed limits, no price) instead of two failures.
- The context meter follows each request of a turn, and reads `<1%` rather
  than `0%` once something is in the window. `/context` puts the measured size
  on its own line; screens with nothing to choose scroll without a pointer.
- A streaming answer is set apart from the prompt from its first words. Without
  colour (`NO_COLOR`), inline code keeps its backticks.
- HTTP errors read `API Error: HTTP 401 (…) from <url>: …` (no doubled "API
  error").
- `forge` opens a terminal UI when stdin and stdout are terminals: answers
  stream into the terminal's own scrollback, and a live region at the bottom
  holds the spinner, dialogs, queued messages, a multiline input box, the
  `/` command menu and a status line (mode, model, context, cost).
- Permission, question and plan-approval dialogs; Esc and Ctrl-C interrupt;
  Shift+Tab cycles the permission mode; messages typed during a turn are
  queued.
- Prompt history per project directory in `<state>/history.jsonl`; Ctrl+R
  searches it.
- Pickers for `/model`, `/resume`, `/rewind`, `/output-style` and
  `/permissions` typed without an argument; `@` completes project paths.
- UI-only commands: `/theme` (dark, light, none), `/copy [n]` (OSC 52),
  `/keybindings` (also `?`), `/statusline <command>` and `/terminal-setup`.
- `--no-tui` or `FORGE_TUI=0` keeps the line REPL.
- Forge's own marks: a braille spinner (`⠋⠙⠹…`), `•` before answers, `›`
  before tool calls, `↳` before results, `»` and `‖` for the accept-edits and
  plan modes. `/export` and the line REPL use the same marks.
- `/model` lists only the models your endpoint reports (an OpenAI-compatible
  `/models` listing); with the default backend it shows the current model and
  takes any id. Messages name models by id.

### Slash commands
- One command registry for every mode: `/help`, `/status`, `/usage` (`/cost`,
  `/stats`), `/doctor`, `/hooks`, `/memory`, `/skills`, `/agents`,
  `/plugin`, `/mcp`, `/tasks` (`/bashes`), `/release-notes`, `/clear`,
  `/compact` and `/exit`. Unknown commands report `Unknown command: /name`,
  and skills chain (`/a /b text`).
- Settings commands that apply at once: `/model`, `/effort`, `/fast`,
  `/config key=value` (`/settings`), `/output-style`, `/autocompact`,
  `/sandbox`, `/permissions` (`/allowed-tools`) and `/add-dir`. In the REPL
  they also save the default; in `-p` they change only that run, except
  `/config`, which always saves and says which settings layer would override it.
- Session commands: `/rename` (Forge suggests a name), `/export`, `/diff`,
  `/context [all]` and `/debug`, which turns on a session debug log and
  has Forge read it.
- Session commands: `/rewind` (restore code, conversation or both, or
  summarize part of the conversation), `/branch`, `/resume`, `/cd`,
  `/reload-skills`, `/reload-plugins`; `/clear` now starts a new session
  and keeps the old one resumable.
- `/advisor <model>`: an Advisor tool the model can consult; it sees the
  conversation so far, and its cost counts toward the session.
- `/import`: MCP servers and instructions from Codex, Gemini CLI and
  Cursor, planned first and applied with `--yes`, following the MCP trust
  rules.
- `/subtask <task>`: a background agent forked from the conversation (C20).
  Its report comes back with the next prompt; `-p` waits for it.
- `/mcp reconnect|enable|disable <server|all>`: restart a server, or turn it
  off and on. Disabling hides its tools and prompts at once and is saved in
  local settings; stream-json hosts get the same through `mcp_reconnect` and
  `mcp_toggle`.
- `/feedback` (`/bug`, `/share`): a private bug-report bundle on this
  machine, with secrets masked, including those inside free text. Nothing
  is uploaded.
- Bundled skills, in Forge's own words: `/init`, `/code-review` (`/review`),
  `/security-review`, `/simplify`, `/verify`, `/run`, `/run-skill-generator`,
  `/batch`, `/fewer-permission-prompts`, `/update-config`.
- Scheduled prompts (C19): `/loop` with an interval or self-paced, and the
  `CronCreate`, `CronList`, `CronDelete` and `ScheduleWakeup` tools. `-p`
  keeps running while tasks are pending.
- `/goal`: Forge keeps working until a small-model check finds the goal
  met, with guards against loops (C18). `/plan`, `/btw` side questions,
  `/recap`, and `!command` shell mode.
- Fast mode on models that offer it (`fastMode` setting, `/fast`).
- Custom commands, skills, output styles and plugins (`--plugin-dir`).

### `@` mentions
- `@path` in any prompt (TUI, REPL, `-p`, stream-json) attaches the file:
  the prompt stays as typed, and the contents follow in a system reminder.
  `@"quoted paths"`, `~/`, directory listings, and notes for images, PDFs,
  binaries and files the permission rules keep out. Limits: 50,000 characters
  per file, 10 files and 200,000 characters per message. Attached files count
  as read, so the model can edit them at once.
- One implementation (`forge_agents::attach`) serves prompts and custom
  commands. UserPromptSubmit hooks see the prompt without the attachments.
- `/loop` and scheduled prompts attach their `@` mentions too, read afresh
  on each run.
- An attached file can't close its own wrapper, and text that looks like
  instructions to an agent is flagged as data, as for Read output.
- TUI: completion quotes paths with spaces, and a dim `(attached: ...)` line
  follows the prompt.

### Fixes from checking the four together
- A screen that arrives while a permission prompt is open (`/context` typed
  mid-turn) waits for the answer instead of replacing the prompt, which
  denied the tool and interrupted the turn.
- The TUI's `/` menu follows a reload that keeps the session
  (`/reload-skills`, `/hooks add`, `/agents create`): new skills show up.

### The manual check run
- `docs/CHECKLIST.md` is one ordered run of 55 checks in three parts (print
  mode, interactive, special setups), with exact commands, what to see,
  where to look and a results table. The old numbers stay in brackets.
- `scripts/check-setup.sh [dir] [--force]` builds the scratch repository it
  uses.
- Tests keep it honest: every `forge ...` command in it parses, every slash
  command exists, and the print-mode checks a mock can stand in for are run.
- `docs/BASELINE.md` explains how to record the first real `forge-eval`
  baseline; a test checks its commands.

### TUI screens
- Screens: a command typed without arguments can open a scrolling viewer in
  the live region (still an inline viewport, no alternate screen). Up/Down,
  PageUp/PageDown, Home/End scroll; Enter acts on a row; Esc goes back.
- `/diff` lists the changed files with their +/- counts; Enter jumps to a
  file's hunks, coloured by side.
- `/context` is a 10×10 grid coloured by what fills the window, with the
  same numbers as the text form; it opens mid-turn too.
- `/hooks add <Event> <matcher> <command> [--scope ..]` and
  `/hooks remove <Event> <n>` change hooks in settings and reload the
  session; `/hooks` numbers each event's hooks and names their source. The
  TUI's hooks screen adds (a form) and removes (Enter, then confirm).
- `/agents create <name> --description .. [--tools ..] [--model ..]
  [--scope project|user]` writes an agent definition and reloads; the TUI's
  agents screen has a wizard for it.
- Key rebinding: `<config>/keybindings.json` maps keys to named actions
  (`submit`, `historySearch`, `cycleMode`, ...; `none` unbinds).
  `/keybindings` shows the keys in effect; bad entries are warnings.
- `/color <name>` sets the accent colour for this session; `/focus` keeps
  tool calls out of the scrollback until turned off.

### Immediate commands mid-turn
- `/status`, `/usage` (`/cost`, `/stats`), `/tasks`, `/context`, `/mcp` and
  `/btw` answer at once while a turn runs, in the TUI and for stream-json
  hosts, instead of waiting for the turn to end. The spinner keeps going.
- They read a session view the engine and driver publish (after each model
  call and tool batch, and when idle), so mid-turn numbers are current:
  `/usage` counts the turn so far.
- Stream-json marks their `result` lines `"immediate": true`, so hosts can
  tell them from the running turn's result.
- `/btw` mid-turn answers from the conversation so far; its cost and
  exchange, and `/mcp`'s refresh of instructions and `system/init`, are
  applied when the turn ends. A second `/btw` in the same turn sees the
  first.

### Fixes from the front-end review
- A prompt that starts with a file name (`/package.json has the wrong
  version`) is a prompt, not `Unknown command`.
- A stream-json host's `set_model` now reaches the system prompt too, which
  told the new model it was the old one.
- A host's `set_permission_mode: bypassPermissions` is refused unless the
  session was launched in that mode or with
  `--allow-dangerously-skip-permissions`, and always when managed settings
  set `disableBypassPermissionsMode`.
- The REPL said nothing when `--max-turns`, `--max-budget-usd` or a failed
  compaction stopped a turn: these now arrive as error notices, like API
  errors (print mode still reports them once, with the result).
- Secret masking (`forge config`, `/feedback`) now also catches a bearer
  token inside a value whose key looks harmless (custom headers), quoted
  keys in JSON or YAML text (`"api_key": "..."`), private key blocks, and
  more token shapes (Google, npm, GitLab, Stripe, Slack). The `/feedback`
  report and doctor files are masked too.
- `/import codex` no longer fails on a `config.toml` with several `[[x]]`
  entries that share a key.
- An advisor model priced through `modelPricing` counted as free against
  `--max-budget-usd`; its requests are now priced like the session's own.
- A subtask's edits made while a later prompt ran were checkpointed under
  that prompt: `/rewind <that prompt> code` undid them, and the verification
  loop counted them as the prompt's own writes. They now keep their own turn.
- `/tasks` lists subtasks that have finished (the last 20), not only the
  running ones: they were handed back before the list was made.
- In the terminal UI, `/keybindings` and `/terminal-setup` answer at once
  while a turn runs instead of waiting in the queue.
- Ctrl-D during a turn in the terminal UI interrupts it before leaving,
  instead of leaving it running while Forge waits to exit.

### Fixes from the slash-command review
- Scheduling: cron times no longer hang or skip a day across daylight-saving
  changes; the scheduler follows the wall clock after the machine sleeps;
  `/loop 50m` rounds to hourly and `/loop 20h` to daily; a `-p` run stopped
  by a limit while tasks are scheduled exits 4; after stdin ends, host
  permission prompts are refused instead of hanging.
- Goals: Ctrl-C during the goal check stops the loop; a hook's
  `continue: false` pauses it; `--max-turns` and `--max-budget-usd` count
  across the loop, and the check's cost is counted; a first-call credential
  error clears the goal; pauses are reported as `system/goal` events.
- Sessions: `/reload-*`, `/branch` and `/cd` keep the goal, scheduled tasks,
  background shells and the sandbox; rules allowed for the session survive
  `/clear`; `/cd` brings the new directory's memory files; forks keep their
  file checkpoints; `/rewind` skips synthetic messages and finds checkpoints
  for prompts that changed nothing; `--resume` of another directory's session
  is refused (use `--fork-session`); a torn transcript line no longer makes a
  session unresumable; SessionStart runs with source `resume`.
- Settings: a settings file Forge can't parse is never overwritten, and
  writes are atomic; `/permissions remove` can't drop managed or
  command-line rules; `/config` reports its notes and applies
  `verification.enabled` at once; `.forge/settings.local.json` is kept out
  of git; a custom `anthropic-beta` header joins the beta list; fast mode is
  priced at its rate.
- `!command` runs only in the interactive session (on `-p` it is an ordinary
  prompt), can be interrupted, and keeps at most 8 MB of output.

### Agent harness
- Long commands are no longer killed at the Bash timeout (C22). A build,
  install or test run still going after the call's wait (2 minutes by
  default) moves to the background; the call returns its ID and the output so
  far, and the model is told when it exits. A turn about to end waits for such
  commands (up to 10 minutes), so the result isn't lost and the command isn't
  run twice. Background shells started with `run_in_background` are reported
  when they exit too. Before, a wrong guess at the timeout killed the
  command, could leave a half-installed environment, and cost the whole run
  again.
- Prompt: what the user specifies (names, paths, formats) is a requirement;
  no stubs, mock data or hard-coded answers in place of the real thing; fix a
  failing tool's cause, not its source or installed packages; learn what an
  existing thing does before reimplementing it; checks are run again only
  after another change; one-sentence updates between tool calls.
- Verification loop: changes don't end a turn until checks have run (C12).
- Recovery:
  - Edit "did you mean" hints;
  - a loop guard;
  - continuation after `max_tokens`, with a higher output cap (C13);
  - tool calls the model ended itself with invalid JSON arguments (a missing
    comma, quote or brace, raw newlines in a string) are repaired when a small
    fix is safe (`system/tool_input_repaired`); otherwise the error says the
    JSON was invalid and shows where, instead of blaming the output limit.
- Safety:
  - dangerous-command patterns always ask;
  - injected instructions in tool output are marked as data (C14);
  - an OS sandbox for Bash (`--sandbox`).
- Context:
  - long output is saved to a file and the result points to it;
  - no repeated reads;
  - the task list survives compaction and resume (C15);
  - an agent can set its own `effort` (frontmatter or `--agents` JSON); the
    built-in Explore agent searches at `low` effort, so exploration costs
    less. Models without effort levels ignore it.

### Measurement
- `--autonomous` (`FORGE_AUTONOMOUS=1`, setting `autonomous`), for runs nobody
  watches:
  - no question or plan-approval tools;
  - a "Running unattended" prompt section;
  - one reminder to make a real attempt when a turn would end without any tool
    call;
  - two verification reminders.
  The Harbor agent turns it on; `FORGE_HARBOR_AUTONOMOUS=0` turns it off.
- `--max-time` (`900`, `15m`, `1h`): the model is told its time limit up
  front and reminded to wrap up when a fifth of it is left. The Harbor agent
  passes each Terminal-Bench task's own limit, so runs end with a report
  instead of being cut off mid-change.
- The Harbor agent can trust extra certificate authorities in task containers
  (`FORGE_HARBOR_CA_BUNDLE`), for networks that inspect TLS; such jobs are
  named `-hostca` and are for local comparisons only.
- `evals/harbor/report.py` writes `jobs/<job>/report.md` when a job ends:
  setup, score, a verdict per task (passed, false success, gave up, refused,
  API error, timeout, ...), signals (429s, retries, tool errors by kind,
  reminders), and per task the instruction, verifier failures and the whole
  conversation. Several jobs also give a side-by-side comparison. The agents
  save the instruction and their settings for it; keys are removed.
- `evals/harbor/run.sh` rebuilds the static binary when the code changed
  since it was built, so a job named after a commit runs that commit's code.
  Jobs labelled 02195fd probably ran an older binary. `FORGE_STATIC_BIN`
  jobs are named `-custombin`.
- `evals/harbor/capture-prompts.sh` saves the system prompt and tool
  descriptions that ForgeCLI, Forge Code and Ante send, through a fake
  endpoint on localhost, into `evals/harbor/prompts/`. Captured so far:
  ForgeCLI (1056 words, 19 tools) and Forge Code 2.13.21 (1660 words, 13
  tools).
- `evals/harbor/watch.py` shows a run like a conversation: what the model
  says, one short line per tool call, failed calls with their error, how the
  run ended and why, and each task's reward. `--thinking` adds the reasoning,
  `--full` the tool input and output. Other agents' logs are followed too.
  A multi-line command shows the model's description of it, and a task that
  starts after `watch.py` is shown from its first step.
- `evals/harbor/ante_agent.py` and `run-ante.sh` run Ante on the same tasks,
  for side-by-side comparisons; `forgecode_agent.py` and `run-forgecode.sh` do
  the same for Forge Code (forgecode.dev), downloading its static Linux build.
  Ante runs with `--yolo --no-session-save --no-skills`, as Ante's own Harbor
  agent does (`ANTE_HARBOR_ARGS` overrides).
  `FORGE_HARBOR_MODEL` sets the model for all three run scripts (default
  `deep-thinking`; a `provider/` prefix is optional).
- The Harbor agent rewrites a `localhost` endpoint to this machine's address
  (`FORGE_HARBOR_HOST_IP` to set it), so self-hosted models can be used. A
  login in the URL is kept, and when the address can't be found the error
  says to set `FORGE_HARBOR_HOST_IP`.
- `evals/harbor/`: a Harbor agent that runs ForgeCLI on Terminal-Bench, with
  a static (musl) build script and a run script for a 10-task subset or the
  full 5-attempt run, each job named after its commit.
- The `slugify-trap` eval now checks that behaviour is unchanged, as its
  instruction says.

### Tools and integrations
- MCP client (stdio, streamable HTTP, SSE), `forge mcp` and `forge mcp serve` (C16).
- WebFetch (small-model answers), WebSearch, AskUserQuestion, plan mode, `--worktree`.
- Sub-agents (the Task tool) with the built-in Explore and Plan agents, plus custom agents.

### Core
- Headless print mode with text, json and stream-json output, and the SDK
  control protocol.
- Sessions:
  - JSONL transcripts;
  - resume and fork;
  - file checkpoints and rewind;
  - compaction.
- `forge-eval`, a task suite measuring pass rate, cost, turns, recovery and
  false finishes.
