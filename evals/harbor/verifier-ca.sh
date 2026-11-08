# Sourced by run.sh, run-pi.sh, run-ante.sh and run-forgecode.sh.
#
# With KODACT_HARBOR_CA_BUNDLE, the agent uploads the bundle, adds it to the container's trust store
# and keeps a copy at /usr/local/share/kodact/ca-bundle.pem (KEPT_CA in kodact_agent.py, outside
# /tmp, which a task or the agent may clear). Harbor then runs the task's tests in that same container, but
# the tools the test scripts use to fetch packages and data bring their own certificate lists:
# uv (rustls), pip and requests (certifi), huggingface_hub. Behind a TLS-inspecting proxy they
# reject its certificate, and tasks such as pytorch-model-cli (Torch from download.pytorch.org)
# and reshard-c4-data (the C4 dataset from huggingface.co) score 0 before any test runs.
# These verifier variables (`harbor run --ve`) point each of those tools at the bundle, which the
# agent keeps as the container's own roots plus the extra ones (a proxy-only bundle is enough).
verifier_ca=()
if [ -n "${KODACT_HARBOR_CA_BUNDLE:-}" ]; then
  for var in SSL_CERT_FILE REQUESTS_CA_BUNDLE CURL_CA_BUNDLE PIP_CERT GIT_SSL_CAINFO NODE_EXTRA_CA_CERTS; do
    verifier_ca+=(--ve "$var=/usr/local/share/kodact/ca-bundle.pem")
  done
  verifier_ca+=(--ve UV_NATIVE_TLS=1 --ve UV_SYSTEM_CERTS=1)  # the second is the newer uv's name
fi
# With KODACT_HARBOR_UV_CACHE (made by warm_uv_cache.py), the agent copies that uv cache into the
# container; the tests' uv uses it, so Torch and the like come from disk, not through the proxy.
if [ -n "${KODACT_HARBOR_UV_CACHE:-}" ]; then
  verifier_ca+=(--ve UV_CACHE_DIR=/usr/local/share/kodact/uv-cache)
fi
# uv gives up on a download after 30 s without data, too soon for a slow proxy.
if [ -n "${KODACT_HARBOR_CA_BUNDLE:-}${KODACT_HARBOR_UV_CACHE:-}" ]; then
  verifier_ca+=(--ve UV_HTTP_TIMEOUT=600)
fi
