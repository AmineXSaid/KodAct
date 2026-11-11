#!/bin/sh
# The fix, the input that failed, and the changelog line.
sed -i 's/line.split("=")/line.split("=", 1)/' parser.py
printf "url=http://x/?q=1" > repro.txt
echo "- 0.3.1: values may hold =." >> CHANGES.md
