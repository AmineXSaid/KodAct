# Sourced by run.sh, run-pi.sh, run-ante.sh and run-forgecode.sh, before the job is named.
#
# The caches made once on this machine are used whenever their folder exists, with nothing to
# export: KODACT_HARBOR_UV_CACHE (the tests' packages, warm_uv_cache.py), KODACT_HARBOR_PIP_CACHE
# (what an agent pip-installs, warm_pip_cache.py) and KODACT_HARBOR_APT_CACHE (what an agent
# apt-installs when a task has no Python, warm_apt_cache.py), each in ~/.kodact-harbor. A path
# picks another folder; `off` (or none, 0, no, false) uses none. Each changes only how fast a
# download is, for every agent alike, and names the job (-uvcache, -pipcache, -aptcache).
for kodact_cache in UV_CACHE:uv-cache PIP_CACHE:pip-wheelhouse APT_CACHE:apt-cache; do
  kodact_var="KODACT_HARBOR_${kodact_cache%%:*}"
  kodact_dir="$HOME/.kodact-harbor/${kodact_cache#*:}"
  case "$(printf '%s' "${!kodact_var:-}" | tr 'A-Z' 'a-z')" in
    off|none|0|no|false) unset "$kodact_var" ;;
    "")
      if [ -d "$kodact_dir" ]; then
        export "$kodact_var=$kodact_dir"
        echo "cache: $kodact_dir ($kodact_var)"
      fi
      ;;
  esac
done
unset kodact_cache kodact_var kodact_dir
