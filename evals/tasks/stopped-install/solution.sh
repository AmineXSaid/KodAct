# The install runs to its end (nothing is left half-updated), then the note the person asked for.
./tools/apt-get install fastcc fastlib fastld >/dev/null
echo skipped > status.txt
