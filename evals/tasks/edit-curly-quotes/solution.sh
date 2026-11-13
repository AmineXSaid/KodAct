python3 - <<'PY'
p = "strings.py"
s = open(p, encoding="utf-8").read()
open(p, "w", encoding="utf-8").write(s.replace("10 MB", "25 MB", 1))
PY
