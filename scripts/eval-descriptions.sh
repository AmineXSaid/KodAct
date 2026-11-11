#!/usr/bin/env bash
# Measure a set of tool descriptions: the same build, tasks and model, once with KodAct's own
# descriptions and once with the ones in a directory, then the A/B table.
#
#   scripts/eval-descriptions.sh --prompts DIR [--label NAME] [--repeat N] [--only TASK]... -- KODACT_ARGS
#
#   source ~/.kodact-env
#   scripts/eval-descriptions.sh --prompts ~/kodact-prompts/shorter-edit --repeat 3 -- \
#     --model sonnet --dangerously-skip-permissions
#
# DIR is a prompt set as KODACT_PROMPTS_DIR takes it: `tools/<ToolName>.md` replaces that tool's
# description (`tools/Edit.md`, `tools/Grep.md`…), `system/*.md` the system prompt's sections.
# Keep it outside this repository. A (before) runs without it, B (after) with it; both use
# this checkout's release build. Reports land in evals/results/<NAME>-<time>-before and -after
# (git ignores evals/results/). docs/BASELINE.md says how to read the table: a description is
# kept when the pass rate rises without the false-finish rate rising, or turns, invalid calls
# or cost fall at the same pass rate, over enough runs to tell (`--repeat 3` at least).
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

prompts=""
label=descriptions
repeat=1
only=()
while [ $# -gt 0 ]; do
  case "$1" in
    --prompts) prompts="$2"; shift 2 ;;
    --label) label="$2"; shift 2 ;;
    --repeat) repeat="$2"; shift 2 ;;
    --only) only+=(--only "$2"); shift 2 ;;
    --) shift; break ;;
    -h | --help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 0 ;;
    *) echo "eval-descriptions: unknown option $1 (try --help)" >&2; exit 2 ;;
  esac
done
if [ -z "$prompts" ] || [ ! -d "$prompts" ]; then
  echo "eval-descriptions: give the prompt set with --prompts DIR (a directory with tools/<ToolName>.md)" >&2
  exit 2
fi
if [ ! -d "$prompts/tools" ] && [ ! -d "$prompts/system" ]; then
  echo "eval-descriptions: $prompts has neither tools/ nor system/: nothing would differ" >&2
  exit 2
fi
if [ $# -eq 0 ]; then
  echo "eval-descriptions: give kodact's arguments after --, at least --model" >&2
  exit 2
fi
if [ "${1:-}" = "--dry-run" ]; then
  echo "eval-descriptions: would compare KodAct's descriptions with $(cd "$prompts" && pwd)"
  exit 0
fi
if [ -z "${KODACT_OPENAI_BASE_URL:-}${KODACT_BASE_URL:-}${KODACT_API_KEY:-}${KODACT_AUTH_TOKEN:-}${KODACT_GEMINI_API_KEY:-}" ]; then
  echo "eval-descriptions: no endpoint or key in the environment: source your env file first" >&2
  exit 1
fi
prompts="$(cd "$prompts" && pwd)"
cargo build --release -q -p kodact-cli -p kodact-eval

stamp="$(date +%Y%m%d-%H%M%S)"
kodact_args=("$@")
run() { # side, prompt set ("" = KodAct's own)
  local out="evals/results/$label-$stamp-$1"
  mkdir -p "$out"
  {
    echo "side: $1"
    echo "commit: $(git rev-parse --short HEAD)"
    echo "prompts: ${2:-(built in)}"
    echo "args: ${kodact_args[*]}"
    echo "repeat: $repeat"
  } >"$out/ab.txt"
  echo "eval-descriptions: running $1 (${2:-built-in descriptions})"
  KODACT_PROMPTS_DIR="$2" target/release/kodact-eval run --label "$label-$stamp-$1" --out "$out" \
    --repeat "$repeat" ${only[@]+"${only[@]}"} -- "${kodact_args[@]}" >"$out/run.log" 2>&1 || {
    tail -n 20 "$out/run.log" >&2
    return 1
  }
}
run before ""
run after "$prompts"
target/release/kodact-eval compare "evals/results/$label-$stamp-before" "evals/results/$label-$stamp-after"
echo "eval-descriptions: reports in evals/results/$label-$stamp-before and -after"
