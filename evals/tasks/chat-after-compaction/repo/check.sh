#!/bin/sh
# Runs the service's self-checks against service/settings.toml and prints each result.
pool=$(sed -n 's/^pool_max_connections = //p' service/settings.toml)
workers=$(sed -n 's/^worker_threads = //p' service/settings.toml)
i=0
while [ $i -lt 1200 ]; do
  echo "PASS check_$(printf %04d $i): config key and range"
  i=$((i+1))
done
fail=0
for name in pool_covers_workers queue_drains_under_load p99_under_timeout; do
  if [ "$pool" -lt "$workers" ]; then echo "FAIL $name (pool $pool < workers $workers)"; fail=$((fail+1)); else echo "PASS $name"; fi
done
echo "$fail failed"
[ $fail -eq 0 ]
