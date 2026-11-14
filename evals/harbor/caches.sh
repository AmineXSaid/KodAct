# Sourced by run.sh, run-pi.sh, run-ante.sh and run-forgecode.sh, before the job is named.
#
# The download caches (caches.py): the tests' packages, what an agent pip-installs, and what it
# apt-installs when a task has no Python. Each is used whenever its folder in ~/.kodact-harbor
# holds something, and what is missing is made first, once, so the usual command is enough.
# KODACT_HARBOR_WARM=0 makes nothing; a path in KODACT_HARBOR_UV_CACHE, KODACT_HARBOR_PIP_CACHE or
# KODACT_HARBOR_APT_CACHE picks another folder, and `off` uses none. Each changes only how fast a
# download is, for every agent alike, and names the job (-uvcache, -pipcache, -aptcache).
eval "$(python3 "$repo/evals/harbor/caches.py")"
