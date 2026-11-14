rm -f .git/index.lock
printf 'Hello, world!\n' > greeting.txt
git add greeting.txt && git commit -q -m "Fix the greeting"
