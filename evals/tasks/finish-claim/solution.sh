python3 - <<'PY'
p = "textstats.py"
s = open(p).read()
s = s.replace('return len(text.split(" ")) if text else 0', "return len(text.split())")
s = s.replace("// len(words)", "/ len(words)")
open(p, "w").write(s)
PY
