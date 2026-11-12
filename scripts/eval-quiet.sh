#!/usr/bin/env bash
# What does KodAct cost when it isn't needed (docs/goal.md, "How quiet is checked")? The
# everyday tasks a model solves without help, once with every reminder off (A) and once with
# the configuration under test (B), then the A/B table.
#
#   scripts/eval-quiet.sh [--nudges LIST] [--label NAME] [--repeat N] [--only TAG]... -- KODACT_ARGS
#
#   source ~/.kodact-env
#   scripts/eval-quiet.sh --repeat 3 -- --model sonnet --dangerously-skip-permissions
#   scripts/eval-quiet.sh --repeat 3 --nudges all -- --model sonnet --dangerously-skip-permissions
#
# A runs with KODACT_REMINDERS=0. B runs with KodAct's defaults, or with the nudges named
# (KODACT_NUDGES). Both use this checkout's release build and the tasks tagged `quiet` (give
# --only for others). Reading it: at the same pass rate, tokens, turns and wall time per task
# should match. What B has more of is what that configuration costs a turn in which it has
# nothing to say, and is stated with the change that caused it.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

nudges=""
label=quiet
repeat=1
only=()
while [ $# -gt 0 ]; do
  case "$1" in
    --nudges) nudges="$2"; shift 2 ;;
    --label) label="$2"; shift 2 ;;
    --repeat) repeat="$2"; shift 2 ;;
    --only) only+=(--only "$2"); shift 2 ;;
    --) shift; break ;;
    -h | --help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 0 ;;
    *) echo "eval-quiet: unknown option $1 (try --help)" >&2; exit 2 ;;
  esac
done
if [ ${#only[@]} -eq 0 ]; then only=(--only quiet); fi
if [ $# -eq 0 ]; then
  echo "eval-quiet: give kodact's arguments after --, at least --model" >&2
  exit 2
fi
if [ -z "${KODACT_OPENAI_BASE_URL:-}${KODACT_BASE_URL:-}${KODACT_API_KEY:-}${KODACT_AUTH_TOKEN:-}${KODACT_GEMINI_API_KEY:-}" ]; then
  echo "eval-quiet: no endpoint or key in the environment: source your env file first" >&2
  exit 1
fi
cargo build --release -q -p kodact-cli -p kodact-eval

stamp="$(date +%Y%m%d-%H%M%S)"
kodact_args=("$@")
run() { # side, reminders (0 or 1), nudges
  local out="evals/results/$label-$stamp-$1"
  mkdir -p "$out"
  {
    echo "side: $1"
    echo "commit: $(git rev-parse --short HEAD)"
    echo "reminders: $2"
    echo "nudges: ${3:-(none)}"
    echo "args: ${kodact_args[*]}"
    echo "repeat: $repeat"
  } >"$out/ab.txt"
  echo "eval-quiet: running $1 (reminders $2, nudges ${3:-none})"
  KODACT_REMINDERS="$2" KODACT_NUDGES="$3" target/release/kodact-eval run --label "$label-$stamp-$1" \
    --out "$out" --repeat "$repeat" "${only[@]}" -- "${kodact_args[@]}" >"$out/run.log" 2>&1 || {
    tail -n 20 "$out/run.log" >&2
    return 1
  }
}
run silent 0 ""
run tested 1 "$nudges"
target/release/kodact-eval compare "evals/results/$label-$stamp-silent" "evals/results/$label-$stamp-tested"
echo "eval-quiet: reports in evals/results/$label-$stamp-silent and -tested"
