#!/usr/bin/env bash
# Run Terminal-Bench on Pi (pi-coding-agent) through Harbor, the counterpart of run.sh.
#
#   evals/harbor/run-pi.sh smoke    # 1 task: does it start and reach the endpoint?
#   evals/harbor/run-pi.sh subset   # the first 10 tasks, 1 attempt
#   evals/harbor/run-pi.sh full     # every task, 5 attempts
#
# Settings (environment):
#   KODACT_OPENAI_BASE_URL, KODACT_OPENAI_API_KEY  the endpoint and key, as for KodAct
#   KODACT_HARBOR_MODEL    the model, as the endpoint names it (default deep-thinking); the same
#                         variable sets it for run.sh, run-forgecode.sh, run-ante.sh and run-pi.sh
#   KODACT_CONTEXT_WINDOW  the model's context window (default 200000, as KodAct assumes)
#   KODACT_MAX_OUTPUT_TOKENS  the most tokens per reply (default 32000, as KodAct)
#   KODACT_HARBOR_DATASET  default terminal-bench@2.0
#   KODACT_HARBOR_JOBS     tasks in parallel (default 2)
#   KODACT_HARBOR_CA_BUNDLE   extra CAs to trust in containers (job name gets -hostca)
#   PI_VERSION            Pi's npm version (default latest)
#   PI_PACKAGE            Pi's npm package (default @earendil-works/pi-coding-agent;
#                         @mariozechner/pi-coding-agent for versions before 0.74)
#   PI_NODE_MAJOR         the Node.js major version put in the bundle (default 22)
#   PI_BUNDLE             a bundle to use as is, instead of building one
#   PI_HARBOR_THINKING    Pi's thinking level (off by default), PI_HARBOR_API (openai-completions),
#                         PI_MODELS_JSON, PI_HARBOR_ARGS: see pi_agent.py
# Anything after the mode goes to `harbor run` (e.g. -i 'some-task*').
#
# The bundle (a Linux x64 node binary and Pi's npm package) is built once per Pi version in
# ~/.pi-harbor/ and copied into each task container, so containers download nothing. On a Linux
# x64 machine (WSL included) it is built with the npm of the Node.js it downloads.
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
mode="${1:-subset}"
shift || true

: "${KODACT_OPENAI_BASE_URL:?source your env file first}"
: "${KODACT_OPENAI_API_KEY:?source your env file first}"

case "$mode" in
  smoke) scope=(-l 1 --n-attempts 1) ;;
  subset) scope=(-l 10 --n-attempts 1) ;;
  full) scope=(--n-attempts 5) ;;
  *) echo "usage: $0 smoke|subset|full [harbor run args...]" >&2; exit 2 ;;
esac

pkg="${PI_PACKAGE:-@earendil-works/pi-coding-agent}"
node_major="${PI_NODE_MAJOR:-22}"
# Node.js for Linux x64 (checksum verified), kept in ~/.pi-harbor/: its binary goes into the
# bundle, and on a Linux x64 machine its own npm builds the bundle, so no npm of this machine is
# needed (under WSL, `npm` is often the Windows one, which fails from Linux).
node_dir="$HOME/.pi-harbor/node$node_major-linux-x64"
if [ ! -x "$node_dir/bin/node" ]; then
  echo "downloading Node.js $node_major (linux-x64) to $node_dir"
  dl="$(mktemp -d)"
  base="https://nodejs.org/dist/latest-v$node_major.x"
  curl -fsSL -o "$dl/SHASUMS256.txt" "$base/SHASUMS256.txt"
  file="$(awk '/-linux-x64\.tar\.xz$/ {print $2}' "$dl/SHASUMS256.txt")"
  curl -fsSL -o "$dl/$file" "$base/$file"
  sum="$(grep " $file\$" "$dl/SHASUMS256.txt" | cut -d' ' -f1)"
  got="$( (sha256sum "$dl/$file" 2>/dev/null || shasum -a 256 "$dl/$file") | cut -d' ' -f1)"
  [ "$sum" = "$got" ] || { echo "checksum mismatch for $file" >&2; rm -rf "$dl"; exit 1; }
  tar -xJf "$dl/$file" -C "$dl"
  mkdir -p "$(dirname "$node_dir")"
  rm -rf "$node_dir" && mv "$dl/${file%.tar.xz}" "$node_dir"
  rm -rf "$dl"
fi
if [ "$(uname -s)-$(uname -m)" = "Linux-x86_64" ]; then
  npm_cmd=("$node_dir/bin/node" "$node_dir/lib/node_modules/npm/bin/npm-cli.js")
else
  npm_cmd=(npm)
fi
# Behind a TLS-inspecting proxy, Node trusts the system's CAs only when told to.
ca_file="${NODE_EXTRA_CA_CERTS:-${SSL_CERT_FILE:-}}"
[ -n "$ca_file" ] && [ -f "$ca_file" ] && export NODE_EXTRA_CA_CERTS="$ca_file"

if [ -n "${PI_BUNDLE:-}" ]; then
  version="custombundle"
else
  version="$("${npm_cmd[@]}" view "$pkg@${PI_VERSION:-latest}" version | tail -1)"
  [ -n "$version" ] || { echo "cannot find $pkg@${PI_VERSION:-latest} on npm" >&2; exit 1; }
  PI_BUNDLE="$HOME/.pi-harbor/pi-$version-node$node_major-linux-x64.tar.gz"
  if [ ! -f "$PI_BUNDLE" ]; then
    echo "building $PI_BUNDLE ($pkg@$version, Node.js $node_major, linux-x64)"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    mkdir -p "$tmp/bundle"
    cp "$node_dir/bin/node" "$tmp/bundle/node"
    # For the containers' platform, whatever this machine is.
    "${npm_cmd[@]}" install --prefix "$tmp/bundle" --os=linux --cpu=x64 --no-audit --no-fund --ignore-scripts \
      --omit=dev "$pkg@$version" > "$tmp/npm.log" 2>&1 || { cat "$tmp/npm.log" >&2; exit 1; }
    cli="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["bin"]["pi"])' \
      "$tmp/bundle/node_modules/$pkg/package.json")"
    printf '#!/bin/sh\nd="$(dirname "$0")"\nexec "$d/node" "$d/node_modules/%s/%s" "$@"\n' "$pkg" "${cli#./}" > "$tmp/bundle/pi"
    chmod 755 "$tmp/bundle/pi" "$tmp/bundle/node"
    tar -czf "$PI_BUNDLE.part" -C "$tmp/bundle" . && mv "$PI_BUNDLE.part" "$PI_BUNDLE"
  fi
fi
export PI_BUNDLE

ca=""
[ -n "${KODACT_HARBOR_CA_BUNDLE:-}" ] && ca="-hostca"
[ -n "${KODACT_HARBOR_UV_CACHE:-}" ] && ca="${ca}-uvcache"
[ -n "${KODACT_HARBOR_PIP_CACHE:-}" ] && ca="${ca}-pipcache"
[ -n "${KODACT_HARBOR_HF_ENDPOINT:-}" ] && ca="${ca}-hfmirror"
model="${KODACT_HARBOR_MODEL:-deep-thinking}"
model_tag="$(printf '%s' "${model#*/}" | tr -c 'A-Za-z0-9._-' '-')"
think=""
[ "${PI_HARBOR_THINKING:-off}" != "off" ] && think="-think-$PI_HARBOR_THINKING"
job="pi-${mode}-${model_tag}-${version}${think}${ca}-$(date +%Y%m%d-%H%M%S)"
echo "job: $job"

# The report is written even when harbor fails part way.
status=0
. "$repo/evals/harbor/verifier-ca.sh"
PYTHONPATH="$repo/evals/harbor${PYTHONPATH:+:$PYTHONPATH}" harbor run \
  -d "${KODACT_HARBOR_DATASET:-terminal-bench@2.0}" \
  -m "$model" \
  --agent pi_agent:PiAgent \
  --n-concurrent "${KODACT_HARBOR_JOBS:-2}" \
  --job-name "$job" \
  ${verifier_ca[@]+"${verifier_ca[@]}"} "${scope[@]}" "$@" || status=$?
python3 "$repo/evals/harbor/report.py" "jobs/$job" || true
exit "$status"
