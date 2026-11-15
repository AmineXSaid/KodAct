# Sourced by run.sh before the job is named.
#
# Settles KODACT_HARBOR_MAX_OUTPUT, the reply limit kodact_agent.py gives the run's model as
# kodact's `modelLimits` (through `--settings`): exported when there is one (the job then gets
# -maxout<N>), unset when there is none. Unset by the person, it is 65536 for deep-thinking: that
# alias is served by vLLM, which bounds a reply only by its window and states no reply limit, so
# KodAct would keep its 32,000 default and cut a reply that is still reasoning at 32,000. Set to
# 0 or left empty, there is none; anything else must be a number of tokens.
max_output_model="${KODACT_HARBOR_MODEL:-deep-thinking}"
if [ -z "${KODACT_HARBOR_MAX_OUTPUT+set}" ] && [ "${max_output_model#*/}" = deep-thinking ]; then
  KODACT_HARBOR_MAX_OUTPUT=65536
fi
case "${KODACT_HARBOR_MAX_OUTPUT:-}" in
  "" | 0) unset KODACT_HARBOR_MAX_OUTPUT ;;
  *[!0-9]*)
    echo "KODACT_HARBOR_MAX_OUTPUT must be a number of tokens, not '$KODACT_HARBOR_MAX_OUTPUT'" >&2
    exit 2
    ;;
  *)
    export KODACT_HARBOR_MAX_OUTPUT
    echo "reply limit for ${max_output_model#*/}: $KODACT_HARBOR_MAX_OUTPUT tokens (kodact modelLimits)"
    ;;
esac
unset max_output_model
