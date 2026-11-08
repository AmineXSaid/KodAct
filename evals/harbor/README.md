# Terminal-Bench with Harbor

`kodact-eval` (in `evals/tasks/`) is the quick local suite. Terminal-Bench is
the external benchmark: 89 tasks, each in its own container, graded by hidden
tests. [Harbor](https://github.com/laude-institute/harbor) runs it. This
directory holds a KodAct agent for Harbor (`kodact_agent.py`) and two scripts.

How a task runs:
1. Harbor starts the task's container.
2. The agent copies a static `kodact` into it (`/usr/local/bin/kodact`).
3. The agent runs `kodact -p "<task>" --dangerously-skip-permissions --output-format stream-json`
   once.
4. Harbor runs the hidden tests.

The stream ends up in each trial's agent logs (`kodact.jsonl`,
`kodact.stderr.txt`). Tokens and cost come from its final `result` line.

## Setup (once)

On Debian or Ubuntu, WSL included, `scripts/setup-debian.sh` does all of this,
plus the certificate and DNS steps under "Things to check first". It writes
only where nothing rewrites it later: not WSL's `/etc/resolv.conf` or
`/etc/hosts`, which WSL regenerates at each start, and not this repository. It
also only adds what is missing: a setting already there, yours or an earlier
run's, is never changed. So every later run keeps the setup, and what you set
by hand. `scripts/setup-debian.sh --check` only reports.

By hand, you need Docker, `uv` and Harbor:

```bash
docker run --rm hello-world
curl -LsSf https://astral.sh/uv/install.sh | sh && source "$HOME/.local/bin/env"
uv tool install --python 3.12 harbor==0.24.0   # the version kodact_agent.py is written for; Python 3.12: some 3.14 builds crash loading PyYAML
harbor --help
```

Build the static binary:

```bash
evals/harbor/build-static.sh             # Docker rust:alpine -> target-static/release/kodact
```

`run.sh` rebuilds it by itself when the code changed since the last build
(`build-static.sh` records the source it built from in `kodact.source`), so a job
named after a commit always runs that commit's code. `KODACT_STATIC_BIN` runs
another binary as is; its jobs are named `-custombin`.

## Running

The `KODACT_*` variables of your shell (endpoint, key, `KODACT_CONTEXT_WINDOW`,
...) are passed into every container. The key is never put on a command line.

```bash
source ~/.kodact-env
evals/harbor/run.sh subset               # first 10 tasks, 1 attempt: check that it works
evals/harbor/run.sh full                 # all tasks, 5 attempts: the leaderboard setup
evals/harbor/run.sh subset -i 'hello*'   # extra args go to `harbor run`
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `KODACT_HARBOR_MODEL` | `deep-thinking` | the model as the endpoint names it (a `provider/` prefix is dropped); the same variable sets it for `run-forgecode.sh`, `run-ante.sh` and `run-pi.sh` |
| `KODACT_HARBOR_DATASET` | `terminal-bench@2.0` | Harbor dataset |
| `KODACT_HARBOR_JOBS` | `2` | Tasks in parallel |
| `KODACT_HARBOR_ARGS` | | Extra `kodact` flags, e.g. `--max-turns 100` |
| `KODACT_STATIC_BIN` | `target-static/release/kodact` | Binary copied into containers |
| `KODACT_HARBOR_AUTONOMOUS` | `1` | `kodact --autonomous`; `0` turns it off (job name gets `-noauto`) |
| `KODACT_HARBOR_TIME_LIMIT` | the task's own limit | Seconds for `kodact --max-time` |
| `KODACT_HARBOR_CA_BUNDLE` | | Extra CAs to trust inside containers (see below) |
| `KODACT_HARBOR_HOSTS` | the endpoint's host | Names to resolve here and pin in each container's `/etc/hosts` (`none` turns it off) |
| `KODACT_HARBOR_HOST_IP` | found automatically | This machine's address, used in place of a `localhost` endpoint |

Each task has an agent time limit, and Harbor stops the agent when it's up.
The agent passes that limit, less 30 seconds, as `--max-time`. The model then
knows its deadline from the start and gets a wrap-up reminder near the end.

The job name records the commit, so every result is pinned to a build. A
`-dirty` suffix means uncommitted changes to `crates/`.

## Self-hosted models

A server on this machine (vLLM, llama.cpp, Ollama, ...) works through its
OpenAI-compatible endpoint:

```bash
export KODACT_OPENAI_BASE_URL=http://localhost:8000/v1
export KODACT_OPENAI_API_KEY=none            # any value, if the server doesn't check
export KODACT_CONTEXT_WINDOW=131072          # the context the server was started with
export KODACT_HARBOR_MODEL=<served model name>
evals/harbor/run.sh subset
```

- Inside a task container `localhost` is the container itself. The agent
  replaces it with this machine's address (`KODACT_HARBOR_HOST_IP` to choose
  one). Start the server on `0.0.0.0`, not only `127.0.0.1`, and allow the
  port in the firewall. Check with
  `docker run --rm curlimages/curl -s http://<that address>:8000/v1/models`.
- Set `KODACT_CONTEXT_WINDOW` (or `modelLimits`) to what the server really
  serves. KodAct otherwise assumes 200,000 tokens and compacts too late.
- Cost shows as 0 or unknown: KodAct has no price for the model. That
  changes nothing in the run. Spending limits are opt-in: no budget is set
  unless you pass `--max-budget-usd`, and the Harbor agent doesn't. If you
  pass it for a model KodAct has no price for, `kodact` refuses to start
  rather than run without a limit. Give the model a price under
  `modelPricing` (`{"input": 0, "output": 0}` for a free one).
- A slow server makes tasks hit their time limit. Lower `KODACT_HARBOR_JOBS`
  so tasks don't share the GPU, and look at `--max-time` warnings in
  `watch.py`.

## Watching a run live

```bash
python3 evals/harbor/watch.py              # newest job; --from-start replays it first
python3 evals/harbor/watch.py --thinking   # also the model's reasoning
python3 evals/harbor/watch.py --full       # also tool input and output
```

One colour per task. For KodAct it shows what the model says, one short line
per tool call (`$ ls -la /app`, `Read /app/x.py`), failed tool calls, KodAct's
reminders, how the run ended (with the error when it failed) and the task's
reward once the verifier writes it. Pi's events are shown the same way. Other
agents' logs (Ante, ...) are shown as they are. A step shows up once the model finishes it.

## The report of a run

When a job ends, `run.sh`, `run-forgecode.sh`, `run-ante.sh` and `run-pi.sh` write
`jobs/<job>/report.md`: one file with everything needed to judge the run.

- **Setup:** harness, version, model, tasks in parallel, start and end, agent
  flags and settings (keys left out).
- **Score:** passed, valid trials (those that reached the model and whose
  verifier actually ran), and one verdict per task: passed, failed, false success
  (said done, checker failed), gave up, refused, api error (before any work or
  later), verifier error (the test phase could not fetch its own packages or
  data — a cert/network failure before any test ran — so it judged nothing;
  set apart like an api error, not counted as the agent failing), timeout,
  crashed.
- **Trials table:** reward, verdict, turns, agent time against the limit,
  tokens, tool calls and errors, API retries, KodAct's own reminders
  (verification, loop guard, act, attempt, next step, todo), how the run ended.
- **Signals:** HTTP 429s and retries, run-ending errors, tool errors by kind and
  by tool, unknown tool names, KodAct's reminders by kind (for example
  `loop_guard:repeated_failure after changes`) and in how many trials, and other
  events.

Each reminder is either needed or a false positive, so audit the ones a change
leaves: fewer reminders only count as better when the pass rate holds. The
comparison of several jobs shows, per job, the agent time per solved task (all
agent time, failed trials included, divided by the tasks passed) and the
reminders per trial.
- **Per task:** the instruction, the agent's last words, the verifier's failing
  tests and output, tool use, and the whole conversation (reasoning, messages,
  tool calls and results, shortened where long), for KodAct and Pi; other
  agents' logs as text.
- **Data:** the same numbers as JSON, for scripts.

```bash
python3 evals/harbor/report.py                         # the newest job
python3 evals/harbor/report.py jobs/A jobs/B jobs/C    # each report, plus jobs/comparison-<time>.md
python3 evals/harbor/report.py jobs/*/ --export reports/   # copies to commit or send
```

The agents save the task's instruction (`agent/instruction.txt`) and how they
were started (`agent/run.json`) for it. API keys are removed from everything
the report writes.

## Comparing harnesses

To compare harnesses, keep the model and the tasks fixed and change only the
agent:
- `-a opencode` or another built-in Harbor agent, against this one;
- Ante (`run-ante.sh`, `ante_agent.py`), Forge Code (`run-forgecode.sh`,
  `forgecode_agent.py`) or Pi (`run-pi.sh`, `pi_agent.py`), against the same
  endpoint and model;
- two KodAct builds;
- `KODACT_VERIFY=0` against the default.

```bash
export KODACT_HARBOR_MODEL=deep-thinking
evals/harbor/run.sh subset
evals/harbor/run-forgecode.sh subset
evals/harbor/run-ante.sh subset
evals/harbor/run-pi.sh subset
python3 evals/harbor/report.py jobs/kodact-subset-* jobs/pi-subset-*   # side by side
```

`run-forgecode.sh` downloads Forge Code's static Linux build
(`forge-x86_64-unknown-linux-musl`, latest release) to `~/.forgecode/forge` on
first use; `FORGECODE_BIN` points it at another binary. It passes the endpoint
and key as `OPENAI_URL` and `OPENAI_API_KEY`, and none of KodAct's `KODACT_*`
settings, which Forge Code would read as its own. `run-forgecode.sh smoke` runs
one task: try it first.

### Pi

`run-pi.sh` runs [Pi](https://github.com/badlogic/pi-mono) (`pi-coding-agent`).
Harbor has a Pi agent of its own (`-a pi`), but it installs Node and Pi from the
internet in every task container and has none of the network handling above. So
`run-pi.sh` builds a bundle once per Pi version on this machine
(`~/.pi-harbor/pi-<version>-node22-linux-x64.tar.gz`, about 70 MB: Node's
official Linux x64 binary, checksum verified, and Pi's npm package) and
`pi_agent.py` copies it into each container. This machine needs `python3` and
`curl` (and `npm` only on a non-Linux machine); containers need `tar` and glibc (Node's binary doesn't run on Alpine).

```bash
evals/harbor/run-pi.sh smoke             # 1 task first
evals/harbor/run-pi.sh subset
```

Pi runs once per task as `pi -p --mode json --no-session --provider kodact-endpoint
--model <model> "<task>"`. Its `models.json` declares one provider for
`KODACT_OPENAI_BASE_URL`, API `openai-completions`, with `KODACT_CONTEXT_WINDOW`
as the context window (200,000 when unset, what KodAct assumes; Pi's own
default is 128,000) and `KODACT_MAX_OUTPUT_TOKENS` as the most tokens per reply
(32,000 when unset, KodAct's; Pi's own 16,384 cut a reasoning model off mid-thought,
before its first tool call). The key stays in an environment variable. Pi's defaults
are kept otherwise: reasoning off for a custom model unless
`PI_HARBOR_THINKING=high` (or another level; the job name gets `-think-high`),
no time limit of its own (Harbor stops it), its 4 tools. `PI_VERSION` picks the
version, `PI_HARBOR_ARGS` adds flags, and `PI_MODELS_JSON` replaces the
`models.json` (keep the provider name `kodact-endpoint`), for example to set
`compat` for a server that rejects the `developer` role. Pi's events go to
`agent/pi.jsonl`, without the per-token updates, and `report.py` and `watch.py`
read them like KodAct's stream.

Run the jobs one after another, not at the same time: an endpoint with a limit
on concurrent requests answers the extra ones with HTTP 429, and the runs that
get them are not comparable. Ten tasks are enough to see a large difference.
Use the full run before you claim a small one.

### Comparing prompts

`capture-prompts.sh` saves what each harness actually sends a model: the system
prompt (`system.md`) and its tools with their descriptions (`tools.json`), in
`evals/harbor/prompts/<harness>/`, with the harness's version. Each agent runs
once against `capture_prompt.py`, a fake endpoint on localhost that records the
request and answers "Done.", so nothing leaves the machine and no key is needed.

```bash
evals/harbor/capture-prompts.sh          # forgecli, forgecode, ante and pi, those installed
evals/harbor/capture-prompts.sh ante     # one of them
```

Ante's default prompt is in its binary, not in its public repository, so this
is how to read it. When nothing is captured, `agent.log` and `server.log` in
the same directory say why.

## Things to check first

- **Network.** The containers must reach your endpoint. Docker's DNS doesn't know
  names that only company DNS resolves, so the agent pins the endpoint's host,
  as this machine resolves it, in each container's `/etc/hosts`. The address
  itself must still be reachable from Docker. A gateway reachable only
  on a company network or VPN may not be reachable from Docker.
- **Rate limits and cost.** The full run is 89 × 5 trials. Run the subset first.
- **Keys.** The key is copied into every task container. Use a key made for
  benchmark runs.

`kodact_agent.py` was written against Harbor 0.24.0 and checked against its
installed-agent API with a fake environment. It has not been run against real
task containers here.

## Behind a company proxy

If `harbor run` fails with `CERTIFICATE_VERIFY_FAILED`, your network
re-signs HTTPS traffic with its own certificate authority, and Harbor's Python
doesn't trust it. Point Python at the system's trust store, which should
already hold that authority:

```bash
export SSL_CERT_FILE=/etc/pki/tls/certs/ca-bundle.crt      # Fedora/CentOS/RHEL
# export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt  # Debian/Ubuntu
```

If that file doesn't have it either, ask IT for the root certificate and add
it to the system trust store (`/etc/pki/ca-trust/source/anchors/` then
`sudo update-ca-trust`, or `/usr/local/share/ca-certificates/` then
`sudo update-ca-certificates`).

On Debian or Ubuntu, `scripts/setup-debian.sh` does this. It looks at the
certificates a few sites send. When the system doesn't trust them, it takes
the authority that signed them from Windows' trust store (under WSL) or from
the file `KODACT_CA_FILE` names, and never from the network. It saves that
authority in `/usr/local/share/ca-certificates/`, which every later
`update-ca-certificates` keeps. Then it adds to `~/.kodact-certs.sh`, which
every new shell reads, the lines that file lacks. They point Python, pip, uv,
Node and cargo at the system store and, on such a network, set
`KODACT_HARBOR_CA_BUNDLE` for every later run. A line you change or comment out
there stays that way.

The task containers have the same problem. Their test scripts download tools
(`uv`, packages) over HTTPS, so on such a network they fail and every task
scores 0, whatever the agent did. To work around it:

```bash
export KODACT_HARBOR_CA_BUNDLE=/etc/pki/tls/certs/ca-bundle.crt
evals/harbor/run.sh subset
```

The agent adds those certificates to the trust store of the container it runs
in, including for any later `update-ca-certificates`, and turns on `native-tls`
for `uv`. A file of only the proxy's certificates is enough (for example
Zscaler's, exported from the Windows store): the bundle the tests get is the
container's own roots plus these, so hosts the proxy lets through untouched
(`download.pytorch.org` behind Zscaler) are trusted too. This changes the task environment, so:
- these jobs are named `-hostca`;
- their scores only compare KodAct builds with each other;
- they aren't comparable with the leaderboard, and must not be submitted.

Harbor runs each task's tests in that same container, but the tools test
scripts use to fetch packages and data bring their own certificate lists (uv's
rustls, pip and requests through certifi, huggingface_hub). So the run scripts
also pass the bundle to the tests (`harbor run --ve`, see `verifier-ca.sh`):
`SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE`, `CURL_CA_BUNDLE`, `PIP_CERT`,
`GIT_SSL_CAINFO`, `NODE_EXTRA_CA_CERTS` and `UV_NATIVE_TLS=1`. Without them,
`pytorch-model-cli` (Torch from download.pytorch.org) and `reshard-c4-data`
(the C4 dataset from huggingface.co) scored 0 before any test ran. A trial whose
test phase still fails this way is marked `verifier error` by `report.py` and
left out of the valid-trial count, as is one whose tests run out of time
(`VerifierTimeoutError`). Downloading Torch through a slow proxy can take longer
than the task's 15-minute verifier limit; give the tests more time, not the
agent, with `--verifier-timeout-multiplier 3` after the mode. For real scores, run from a network without
TLS inspection.

### Slow network: cache what the agent installs too

Some tasks withhold a package the agent then installs itself (`pytorch-model-cli`
withholds Torch, so the agent runs `pip install torch`). On a slow proxy that
download can outlast the task limit, or fall back to the multi-GB CUDA build and
never finish. Download the CPU wheels once and let every run reuse them:

```bash
python3 evals/harbor/warm_pip_cache.py           # Torch CPU + deps, ~180 MB
export KODACT_HARBOR_PIP_CACHE=~/.kodact-harbor/pip-wheelhouse
```

Each agent then copies the wheelhouse into its container and points pip at it
(`PIP_FIND_LINKS`), so the agent's own `pip install` comes from disk; anything
not cached still falls back to the index. The task still withholds the package
and every agent gets the same cache, so the comparison stays even; only the
download speed changes. Jobs get `-pipcache`. This is a local-comparison aid,
not for the leaderboard.

### Containers can't resolve names

If runs fail at random with `Name does not resolve` (for your endpoint) or
`Could not resolve host` (in the tests), check the DNS servers Docker hands to
containers: by default, those of the host. A public resolver such as `8.8.8.8`
in that list can't resolve internal names, and a company network may block or
slow it, so any lookup sent there fails. Find servers that answer both an
internal and a public name:

```bash
for s in $(grep nameserver /etc/resolv.conf | cut -d' ' -f2); do echo "$s: $(timeout 4 nslookup example.com $s >/dev/null 2>&1 && echo ok || echo FAIL)"; done
```

Then list only those in `/etc/docker/daemon.json` (`"dns": ["10.x.x.x", ...]`)
and restart Docker; with Docker Desktop, in Settings > Docker Engine. Check with
`docker run --rm alpine nslookup <your endpoint's host>`.

`scripts/setup-debian.sh` makes this check when containers can't resolve
public names. It tries each server from a container (`docker run --dns`),
before writing anything:
- the ones in `KODACT_DOCKER_DNS`;
- this machine's;
- under WSL, Windows' own.

When the file has no `"dns"` key yet, it adds one with the servers that work
and leaves the other keys alone. If containers still can't resolve names after
that, it puts the file back as it was. A `"dns"` key already there is yours
(company servers for the VPN, say), so it is never replaced: the script prints
the servers that work instead. Don't edit WSL's `/etc/resolv.conf` either: WSL
rewrites it at every start.

### Slow proxy: download the tests' packages once

Some tests download large packages every time they run (`pytorch-model-cli`
installs Torch, torchvision and OpenCV). Through a slow proxy that can take
longer than the verifier limit. Download them once on this machine, then let
every run reuse them:

```bash
python3 evals/harbor/warm_uv_cache.py pytorch-model-cli
export KODACT_HARBOR_UV_CACHE=~/.kodact-harbor/uv-cache
```

`warm_uv_cache.py` reads each named task's own test script and runs its `uvx`
line once, with the same uv version, into that cache (with
`KODACT_HARBOR_CA_BUNDLE`, uv here trusts it too). It drops the environments uv
built on this machine's Python and keeps the packages. With the variable set,
the agent copies the cache into each task container and the tests' uv uses it
(`UV_CACHE_DIR`), so the packages come from disk; uv only checks the index for
changes. Jobs get `-uvcache` in their name. This saves the tests' downloads only:
data a test fetches itself (`reshard-c4-data` loads C4 from Hugging Face) still
goes through the network.
