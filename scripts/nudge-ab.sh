#!/usr/bin/env bash
# Measure the nudges, each against its own eval task: one build, nudge off (A) and on (B), then
# the A/B table per nudge. This is the measurement each nudge waits for before it can be turned
# on by default (docs/CLI.md, "Nudges").
#
#   source ~/.kodact-env                       # endpoint and key, as for kodact-eval
#   scripts/nudge-ab.sh [--repeat N] [--label NAME] [GROUP]... -- KODACT_ARGS
#
#   scripts/nudge-ab.sh --repeat 3 -- --model <model> --dangerously-skip-permissions
#   scripts/nudge-ab.sh --repeat 3 limits snapshots -- --model <model> --dangerously-skip-permissions
#   scripts/nudge-ab.sh --repeat 1 suite -- --model <model> --dangerously-skip-permissions
#
# GROUP (default: shellEdit, constraints, snapshots, limits):
#   shellEdit    rename-keyword        none                 vs shellEdit
#   constraints  named-deliverables    none                 vs constraints,namedChecks
#   snapshots    optimize-under-time   none                 vs passingSnapshots
#   limits       two-plan-scheduler    none                 vs limitVerdicts,staleDeliverables
#   effort       two-plan-scheduler    none                 vs adaptiveEffort (give both an --effort)
#   timebox      two-plan-scheduler    none                 vs thinkingTimeBox
#   suite        every task            none                 vs all (do the nudges hurt other work?)
#
# Reports land in evals/results/<NAME>-<time>-<group>-{off,on}/ (ignored by git); the table
# for each group is printed and saved as compare.md next to them.
#
# Reading it (docs/BASELINE.md): with 1 run per task, a run either way is noise; use
# --repeat 3 or more. A nudge earns "on by default" when, on its task, B's pass rate is at
# least A's, and tokens, minutes and cost don't grow by more than the gain is worth; and when
# `suite` shows no task that passed with A failing with B. Otherwise it stays off.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

label=nudges
repeat=1
groups=()
while [ $# -gt 0 ]; do
  case "$1" in
    --repeat) repeat="$2"; shift 2 ;;
    --label) label="$2"; shift 2 ;;
    --) shift; break ;;
    -h | --help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 0 ;;
    -*) echo "nudge-ab: unknown option $1 (try --help)" >&2; exit 2 ;;
    *) groups+=("$1"); shift ;;
  esac
done
if [ $# -eq 0 ]; then
  echo "nudge-ab: give kodact's arguments after --, at least --model" >&2
  exit 2
fi
if [ -z "${KODACT_OPENAI_BASE_URL:-}${KODACT_BASE_URL:-}${KODACT_API_KEY:-}${KODACT_AUTH_TOKEN:-}${KODACT_GEMINI_API_KEY:-}" ]; then
  echo "nudge-ab: no endpoint or key in the environment: source your env file first" >&2
  exit 1
fi
[ ${#groups[@]} -eq 0 ] && groups=(shellEdit constraints snapshots limits)

# group -> "task|off|on"
spec() {
  case "$1" in
    shellEdit) echo "rename-keyword|none|shellEdit" ;;
    constraints) echo "named-deliverables|none|constraints,namedChecks" ;;
    snapshots) echo "optimize-under-time|none|passingSnapshots" ;;
    limits) echo "two-plan-scheduler|none|limitVerdicts,staleDeliverables" ;;
    effort) echo "two-plan-scheduler|none|adaptiveEffort" ;;
    timebox) echo "two-plan-scheduler|none|thinkingTimeBox" ;;
    suite) echo "*|none|all" ;;
    *) echo "nudge-ab: unknown group $1 (try --help)" >&2; return 1 ;;
  esac
}
for g in "${groups[@]}"; do spec "$g" >/dev/null; done

echo "nudge-ab: building"
cargo build --release -q -p kodact-cli -p kodact-eval
stamp="$(date +%Y%m%d-%H%M%S)"
kodact_args=("$@")

run() { # out-dir, task, nudges
  local only=()
  [ "$2" != "*" ] && only=(--only "$2")
  mkdir -p "$1"
  printf 'nudges: %s\nargs: %s\nrepeat: %s\ncommit: %s\n' "$3" "${kodact_args[*]}" "$repeat" \
    "$(git rev-parse --short HEAD)" >"$1/ab.txt"
  KODACT_NUDGES="$3" target/release/kodact-eval run --label "$(basename "$1")" --out "$1" \
    --kodact "$repo/target/release/kodact" --repeat "$repeat" ${only[@]+"${only[@]}"} \
    -- "${kodact_args[@]}" 2>&1 | tee "$1/run.log" | sed -u 's/^/  /' || {
    echo "nudge-ab: $(basename "$1") failed; see $1/run.log" >&2
    return 1
  }
}

for g in "${groups[@]}"; do
  IFS='|' read -r task off on <<<"$(spec "$g")"
  base="evals/results/$label-$stamp-$g"
  echo "nudge-ab: $g on ${task/\*/every task}: A=$off, B=$on"
  echo "nudge-ab: A ($off), $repeat run(s); log in $base-off/run.log"
  run "$base-off" "$task" "$off"
  echo "nudge-ab: B ($on), $repeat run(s); log in $base-on/run.log"
  run "$base-on" "$task" "$on"
  target/release/kodact-eval compare "$base-off" "$base-on" | tee "$base-compare.md"
done
echo "nudge-ab: reports in evals/results/$label-$stamp-*"
