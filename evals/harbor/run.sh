#!/usr/bin/env bash
# Run Terminal-Bench on KodAct through Harbor.
#
#   evals/harbor/run.sh smoke    # 1 task: does it start and reach the endpoint?
#   evals/harbor/run.sh subset   # the first 10 tasks, 1 attempt: does it work?
#   evals/harbor/run.sh full     # every task, 5 attempts: the leaderboard setup
#
# Settings (environment):
#   KODACT_HARBOR_MODEL    the model, as the endpoint names it (default deep-thinking); the same
#                         variable sets it for run.sh, run-forgecode.sh and run-ante.sh
#   KODACT_HARBOR_DATASET  default terminal-bench@2.0
#   KODACT_HARBOR_JOBS     tasks in parallel (default 2)
#   KODACT_HARBOR_ARGS     extra kodact flags for every task (job name gets -args-<flags>)
#   KODACT_HARBOR_PROBE    0 skips asking the endpoint which model it serves and its window
#   KODACT_HARBOR_AUTONOMOUS  0 runs kodact without --autonomous (job name gets -noauto)
#   KODACT_HARBOR_TIME_LIMIT  seconds for --max-time (default: the task's own limit)
#   KODACT_HARBOR_CA_BUNDLE   extra CAs to trust in containers (job name gets -hostca)
#   KODACT_HARBOR_HF_ENDPOINT another Hugging Face Hub address for the agent and the tests, where
#                         a proxy blocks huggingface.co (job name gets -hfmirror)
#   KODACT_STATIC_BIN      use this binary as is (job name gets -custombin); by default
#                         target-static/release/kodact is rebuilt when the source changed
#   KODACT_NUDGES          the nudges on in every task (job name gets -n-<list>, so an A/B of
#                         nudges off and on is two jobs told apart by name)
#   KODACT_*               endpoint, key and limits, passed into the containers
# Anything after the mode goes to `harbor run` (e.g. -i 'some-task*').
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
mode="${1:-subset}"
shift || true

if [ -z "${KODACT_OPENAI_BASE_URL:-}${KODACT_BASE_URL:-}${KODACT_API_KEY:-}${KODACT_AUTH_TOKEN:-}${KODACT_GEMINI_API_KEY:-}" ]; then
  echo "error: no KODACT_* endpoint or key set; source your env file first" >&2
  exit 1
fi
# The binary must be built from the source the job is named after: rebuild it when it isn't.
custom=""
if [ -n "${KODACT_STATIC_BIN:-}" ]; then
  [ -x "$KODACT_STATIC_BIN" ] || { echo "error: KODACT_STATIC_BIN: $KODACT_STATIC_BIN not found" >&2; exit 1; }
  custom="-custombin"  # someone else's build: the job name can't vouch for it
else
  bin="target-static/release/kodact"
  want="$(evals/harbor/build-static.sh --source-id)"
  have="$(cat "$bin.source" 2>/dev/null || echo none)"
  if [ ! -x "$bin" ] || [ "$have" != "$want" ]; then
    echo "the static kodact binary was built from other source ($have, now $want): rebuilding"
    evals/harbor/build-static.sh
  fi
fi

case "$mode" in
  smoke) scope=(-l 1 --n-attempts 1) ;;
  subset) scope=(-l 10 --n-attempts 1) ;;
  full) scope=(--n-attempts 5) ;;
  *) echo "usage: $0 smoke|subset|full [harbor run args...]" >&2; exit 2 ;;
esac

commit="$(git rev-parse --short HEAD)"
dirty=""
git diff --quiet HEAD -- crates Cargo.toml Cargo.lock || dirty="-dirty"
# Runs with extra trusted CAs change the task environment: label them, they're for local comparisons.
ca=""
[ -n "${KODACT_HARBOR_CA_BUNDLE:-}" ] && ca="-hostca"
[ -n "${KODACT_HARBOR_UV_CACHE:-}" ] && ca="${ca}-uvcache"
[ -n "${KODACT_HARBOR_PIP_CACHE:-}" ] && ca="${ca}-pipcache"
[ -n "${KODACT_HARBOR_HF_ENDPOINT:-}" ] && ca="${ca}-hfmirror"
[ -n "${KODACT_NUDGES:-}" ] && ca="${ca}-n-$(printf '%s' "$KODACT_NUDGES" | tr -c 'A-Za-z0-9' '.')"
[ -n "${KODACT_HARBOR_ARGS:-}" ] && ca="${ca}-args-$(printf '%s' "$KODACT_HARBOR_ARGS" | tr -cs 'A-Za-z0-9' '-' | sed 's/^-*//; s/-*$//' | cut -c1-40)"
case "${KODACT_HARBOR_AUTONOMOUS:-1}" in 0|false|no|off) ca="${ca}-noauto" ;; esac
model="${KODACT_HARBOR_MODEL:-deep-thinking}"
model_tag="$(printf '%s' "${model#*/}" | tr -c 'A-Za-z0-9._-' '-')"
# Which model the endpoint serves behind this name, and its window, asked of the server itself
# (probe_model.py): an alias can change behind your back. Recorded in every trial's run.json and
# the report; the window becomes KODACT_CONTEXT_WINDOW when that is unset, and a different one set
# by hand is warned about. KODACT_HARBOR_PROBE=0 skips it.
if [ -n "${KODACT_OPENAI_BASE_URL:-}" ] && [ "${KODACT_HARBOR_PROBE:-1}" != 0 ]; then
  probe="$(python3 "$repo/evals/harbor/probe_model.py" "$model" 2>/dev/null || true)"
  served="${probe%%$'\t'*}"
  window="${probe#*$'\t'}"
  [ "$window" = "$probe" ] && window=""
  if [ -n "$served" ]; then
    echo "endpoint: $model is served by $served${window:+ (window $window tokens)}"
    export KODACT_HARBOR_SERVED_MODEL="$served"
  fi
  if [ -n "$window" ]; then
    export KODACT_HARBOR_SERVED_WINDOW="$window"
    if [ -z "${KODACT_CONTEXT_WINDOW:-}" ]; then
      export KODACT_CONTEXT_WINDOW="$window"
      echo "KODACT_CONTEXT_WINDOW set to $window, from the server"
    elif [ "$KODACT_CONTEXT_WINDOW" != "$window" ]; then
      echo "warning: KODACT_CONTEXT_WINDOW=$KODACT_CONTEXT_WINDOW, but the server says $window" >&2
    fi
  fi
fi
job="kodact-${mode}-${model_tag}-${commit}${dirty}${custom}${ca}-$(date +%Y%m%d-%H%M%S)"
echo "job: $job"

# The report is written even when harbor fails part way.
status=0
. "$repo/evals/harbor/verifier-ca.sh"
PYTHONPATH="$repo/evals/harbor${PYTHONPATH:+:$PYTHONPATH}" harbor run \
  -d "${KODACT_HARBOR_DATASET:-terminal-bench@2.0}" \
  -m "$model" \
  --agent kodact_agent:KodAct \
  --n-concurrent "${KODACT_HARBOR_JOBS:-2}" \
  --job-name "$job" \
  ${verifier_ca[@]+"${verifier_ca[@]}"} "${scope[@]}" "$@" || status=$?
python3 "$repo/evals/harbor/report.py" "jobs/$job" || true
exit "$status"
