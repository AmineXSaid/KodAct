#!/bin/sh
# fetch takes attempts; every keyword call passes it.
sed -i "s/retries=RETRIES, timeout/attempts=RETRIES, timeout/; s/\"tries\": retries/\"tries\": attempts/" svc/net.py
for f in svc/client_*.py; do perl -0pi -e 's/fetch\(([^)]*?)retries=/fetch($1attempts=/gs' "$f"; done
