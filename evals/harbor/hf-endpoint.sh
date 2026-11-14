# Sourced by run.sh, run-pi.sh, run-ante.sh and run-forgecode.sh, before the job is named.
#
# Settles KODACT_HARBOR_HF_ENDPOINT (hf_endpoint.py): exported when the agent and the tests use
# another Hugging Face Hub address (the job then gets -hfmirror), unset when they use
# huggingface.co. Unset by the person, that is a mirror only where this machine's proxy blocks
# huggingface.co; `off` is never a mirror; a URL is used as given.
hf_endpoint="$(python3 "$repo/evals/harbor/hf_endpoint.py")" || hf_endpoint=""
if [ -n "$hf_endpoint" ]; then
  export KODACT_HARBOR_HF_ENDPOINT="$hf_endpoint"
else
  unset KODACT_HARBOR_HF_ENDPOINT
fi
unset hf_endpoint
