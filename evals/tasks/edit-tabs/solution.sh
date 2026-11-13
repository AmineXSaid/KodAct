python3 - <<'PY'
p = "fetch.py"
s = open(p).read()
old = "\t\t\texcept TimeoutError:\n"
new = "\t\t\texcept PermissionError:\n\t\t\t\tresults[url] = None\n\t\t\t\tbreak\n\t\t\texcept TimeoutError:\n"
open(p, "w").write(s.replace(old, new, 1))
PY
