# The Terminal-Bench dataset the run scripts use. Sourced by run.sh, run-ante.sh, run-forgecode.sh
# and run-pi.sh, before caches.sh.
#
# KODACT_HARBOR_DATASET, by default Terminal-Bench 2.1 (terminal-bench/terminal-bench-2-1): the
# 89 tasks of 2.0 with 28 of them repaired, so that every task can be solved
# (https://www.tbench.ai/news/terminal-bench-2-1). 2.0's board stopped taking submissions in May
# 2026; KODACT_HARBOR_DATASET=terminal-bench@2.0 still runs it, to compare with older jobs.
# terminal-bench/terminal-bench-2-1@<revision> pins a revision of 2.1 (unpinned: the latest).
#
# Sets:
#   dataset       what goes to `harbor run -d`
#   dataset_tag   its short form for job names (tb2.1, tb2.0), so jobs of two versions are told apart
#   KODACT_HARBOR_DATASET, exported: warm_uv_cache.py reads the task scripts of this version
#   task_args     a function: the arguments given to a run script, as harbor needs them (below)
dataset="${KODACT_HARBOR_DATASET:-terminal-bench/terminal-bench-2-1}"
export KODACT_HARBOR_DATASET="$dataset"
case "$dataset" in
  terminal-bench@2.0) dataset_tag="tb2.0" ;;
  terminal-bench/terminal-bench-2-1 | terminal-bench/terminal-bench-2-1@*) dataset_tag="tb2.1" ;;
  *) dataset_tag="$(printf '%s' "$dataset" | tr -c 'A-Za-z0-9.' '-' | sed 's/^-*//; s/-*$//')" ;;
esac

# task_args ARGS...: ARGS into the array task_args_out, for `harbor run`. A registry dataset
# (org/name, as 2.1 is) names its tasks org/task (terminal-bench/mteb-leaderboard), and harbor
# matches -i and -x against that whole name, so `-i mteb-leaderboard` (as the reports print it)
# would match nothing. A pattern without a slash gets "*/" in front; one with a slash, and every
# other argument, goes as it is. A legacy dataset (name@version, as 2.0 is) changes nothing.
task_args() {
  task_args_out=()
  local namespaced="" args=("$@") i=0 arg value
  case "${dataset%%@*}" in */*) namespaced=1 ;; esac
  while [ "$i" -lt "${#args[@]}" ]; do
    arg="${args[$i]}"
    case "$arg" in
      -i | --include-task-name | -x | --exclude-task-name)
        task_args_out+=("$arg")
        i=$((i + 1))
        if [ "$i" -lt "${#args[@]}" ]; then
          value="${args[$i]}"
          if [ -n "$namespaced" ] && [[ "$value" != */* ]]; then value="*/$value"; fi
          task_args_out+=("$value")
        fi
        ;;
      --include-task-name=* | --exclude-task-name=*)
        value="${arg#*=}"
        if [ -n "$namespaced" ] && [[ "$value" != */* ]]; then value="*/$value"; fi
        task_args_out+=("${arg%%=*}=$value")
        ;;
      *) task_args_out+=("$arg") ;;
    esac
    i=$((i + 1))
  done
}
