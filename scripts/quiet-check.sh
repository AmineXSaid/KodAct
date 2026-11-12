#!/usr/bin/env bash
# What KodAct's reminders and nudges cost when the model needs no help (docs/goal.md, "How quiet
# is checked"): the everyday tasks (tag `quiet`: a question about the code, a one-line fix, a
# rename) run twice with one build and one model, A with KodAct's reminders off
# (KODACT_REMINDERS=0) and B with the configuration under test, then the A/B table.
#
#   source ~/.kodact-env                       # endpoint and key, as for kodact-eval
#   scripts/quiet-check.sh [--repeat N] [--label NAME] [--nudges LIST] [--only TASK]... -- KODACT_ARGS
#
#   scripts/quiet-check.sh --repeat 3 -- --model <model> --dangerously-skip-permissions
#   scripts/quiet-check.sh --repeat 3 --nudges all -- --model <model> --dangerously-skip-permissions
#
# B is your settings and environment as they are (reminders on), plus KODACT_NUDGES=LIST when
# --nudges is given. Reports land in evals/results/<NAME>-<time>-{off,on}/ (ignored by git); the
# table is printed and saved as <NAME>-<time>-compare.md next to them.
#
# Reading it: these tasks are ones a model solves alone, so B should cost what A costs. At the
# same pass rate, any difference in tokens, model calls or minutes is what the configuration
# costs a turn in which it has nothing to say; goal.md rule 9 wants it near zero, and a change
# states it. With 1 run per task, a run either way is noise: use --repeat 3 or more.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

label=quiet
repeat=1
nudges=""
only=()
while [ $# -gt 0 ]; do
  case "$1" in
    --repeat) repeat="$2"; shift 2 ;;
    --label) label="$2"; shift 2 ;;
    --nudges) nudges="$2"; shift 2 ;;
    --only) only+=(--only "$2"); shift 2 ;;
    --) shift; break ;;
    -h | --help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 0 ;;
    -*) echo "quiet-check: unknown option $1 (try --help)" >&2; exit 2 ;;
    *) echo "quiet-check: unexpected $1 (kodact's arguments go after --)" >&2; exit 2 ;;
  esac
done
if [ $# -eq 0 ]; then
  echo "quiet-check: give kodact's arguments after --, at least --model" >&2
  exit 2
fi
if [ -z "${KODACT_OPENAI_BASE_URL:-}${KODACT_BASE_URL:-}${KODACT_API_KEY:-}${KODACT_AUTH_TOKEN:-}${KODACT_GEMINI_API_KEY:-}" ]; then
  echo "quiet-check: no endpoint or key in the environment: source your env file first" >&2
  exit 1
fi
[ ${#only[@]} -eq 0 ] && only=(--only quiet)

echo "quiet-check: building"
cargo build --release -q -p kodact-cli -p kodact-eval
stamp="$(date +%Y%m%d-%H%M%S)"
base="evals/results/$label-$stamp"
kodact_args=("$@")

run() { # out-dir, what, then env's arguments for the side (-u NAME first, then NAME=VALUE)
  local out="$1" what="$2"
  shift 2
  mkdir -p "$out"
  printf 'side: %s\nargs: %s\nrepeat: %s\ncommit: %s\n' "$what" "${kodact_args[*]}" "$repeat" \
    "$(git rev-parse --short HEAD)" >"$out/ab.txt"
  env "$@" target/release/kodact-eval run --label "$(basename "$out")" --out "$out" \
    --kodact "$repo/target/release/kodact" --repeat "$repeat" "${only[@]}" \
    -- "${kodact_args[@]}" 2>&1 | tee "$out/run.log" | sed -u 's/^/  /' || {
    echo "quiet-check: $(basename "$out") failed; see $out/run.log" >&2
    return 1
  }
}

echo "quiet-check: A, reminders off; log in $base-off/run.log"
run "$base-off" "KODACT_REMINDERS=0" -u KODACT_NUDGES KODACT_REMINDERS=0
if [ -n "$nudges" ]; then
  echo "quiet-check: B, the configuration under test with KODACT_NUDGES=$nudges; log in $base-on/run.log"
  run "$base-on" "configuration under test, KODACT_NUDGES=$nudges" -u KODACT_REMINDERS KODACT_NUDGES="$nudges"
else
  echo "quiet-check: B, the configuration under test; log in $base-on/run.log"
  run "$base-on" "configuration under test${KODACT_NUDGES:+, KODACT_NUDGES=$KODACT_NUDGES}" -u KODACT_REMINDERS
fi
target/release/kodact-eval compare "$base-off" "$base-on" | tee "$base-compare.md"
echo "quiet-check: reports in $base-*"
