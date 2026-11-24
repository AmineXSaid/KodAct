#!/usr/bin/env bash
# Terminal-Bench on Ante CLI through Harbor (counterpart of run.sh).
#   KODACT_HARBOR_MODEL=deep-thinking evals/harbor/run-ante.sh smoke|subset|full [harbor args...]
# Settings: KODACT_HARBOR_MODEL (default deep-thinking, as for run.sh; ANTE_MODEL also works),
#           ANTE_BIN (the static Linux binary uploaded to each container, default ~/.ante/bin/ante),
#           ANTE_PROVIDER (openai-compatible), ANTE_INSTALL_ARGS (empty = stable),
#           ANTE_HARBOR_ARGS (default "--yolo --no-session-save --no-skills", as Ante's own
#           Harbor agent runs it, minus JSON output),
#           KODACT_HARBOR_JOBS (tasks in parallel, default 2), KODACT_HARBOR_DATASET,
#           KODACT_HARBOR_CA_BUNDLE (label only for now)
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
mode="${1:-subset}"; shift || true

: "${KODACT_OPENAI_BASE_URL:?source ~/.kodact-env-kpit first}"
: "${KODACT_OPENAI_API_KEY:?source ~/.kodact-env-kpit first}"

# ante_agent.py uploads a prebuilt Ante binary into each container; it doesn't install one. A
# binary or CA bundle that isn't there ended every trial in setup (RuntimeError, 2026-11-24): say
# so before any trial runs.
ante_bin="${ANTE_BIN:-$HOME/.ante/bin/ante}"
if [ ! -f "$ante_bin" ]; then
  echo "error: no Ante binary at $ante_bin: install Ante on this machine, or set ANTE_BIN to its static Linux binary" >&2
  exit 1
fi
if [ -n "${KODACT_HARBOR_CA_BUNDLE:-}" ] && [ ! -f "$KODACT_HARBOR_CA_BUNDLE" ]; then
  echo "error: KODACT_HARBOR_CA_BUNDLE: $KODACT_HARBOR_CA_BUNDLE not found" >&2
  exit 1
fi

case "$mode" in
  smoke)  scope=(-l 1 --n-attempts 1) ;;
  subset) scope=(-l 10 --n-attempts 1) ;;
  full)   scope=(--n-attempts 5) ;;
  *) echo "usage: $0 smoke|subset|full [harbor run args...]" >&2; exit 2 ;;
esac

model="${KODACT_HARBOR_MODEL:-${ANTE_MODEL:-deep-thinking}}"
model_tag="$(printf '%s' "${model#*/}" | tr -c 'A-Za-z0-9._-' '-')"
provider="${ANTE_PROVIDER:-openai-compatible}"
install="${ANTE_INSTALL_ARGS:-}"
. "$repo/evals/harbor/dataset.sh"  # which Terminal-Bench (2.1 by default): before caches.sh and the job name
. "$repo/evals/harbor/hf-endpoint.sh"  # which Hugging Face Hub: before the job name, which says it
. "$repo/evals/harbor/caches.sh"  # the caches made on this machine, when their folders exist
ca=""; [ -n "${KODACT_HARBOR_CA_BUNDLE:-}" ] && ca="-hostca"
[ -n "${KODACT_HARBOR_UV_CACHE:-}" ] && ca="${ca}-uvcache"
[ -n "${KODACT_HARBOR_PIP_CACHE:-}" ] && ca="${ca}-pipcache"
[ -n "${KODACT_HARBOR_APT_CACHE:-}" ] && ca="${ca}-aptcache"
[ -n "${KODACT_HARBOR_HF_ENDPOINT:-}" ] && ca="${ca}-hfmirror"
job="ante-${mode}-${dataset_tag}-${model_tag}-${install:-stable}${ca}-$(date +%Y%m%d-%H%M%S)"
echo "job: $job"

# ante_agent.py reads these on this machine too (endpoint host, container URL), not only in the container.
export OPENAI_COMPATIBLE_BASE_URL="$KODACT_OPENAI_BASE_URL" OPENAI_COMPATIBLE_API_KEY="$KODACT_OPENAI_API_KEY"

# The report is written even when harbor fails part way.
status=0
# The agent is evals/harbor/ante_agent.py; ~/ante-src/ante-harbor, if there, comes after it.
. "$repo/evals/harbor/verifier-ca.sh"
task_args "$@"
PYTHONPATH="$repo/evals/harbor:$HOME/ante-src/ante-harbor${PYTHONPATH:+:$PYTHONPATH}" harbor run \
  -d "$dataset" \
  -m "$model" \
  --agent ante_agent:AnteAgent \
  --ak provider="$provider" --ak install_args="$install" \
  --ae "OPENAI_COMPATIBLE_BASE_URL=$KODACT_OPENAI_BASE_URL" \
  --ae "OPENAI_COMPATIBLE_API_KEY=$KODACT_OPENAI_API_KEY" \
  --n-concurrent "${KODACT_HARBOR_JOBS:-2}" \
  --job-name "$job" \
  ${verifier_ca[@]+"${verifier_ca[@]}"} "${scope[@]}" ${task_args_out[@]+"${task_args_out[@]}"} || status=$?
python3 "$repo/evals/harbor/report.py" "jobs/$job" || true
exit "$status"
