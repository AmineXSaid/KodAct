python3 - <<'PY'
import re
p = "textutil.py"
s = open(p).read()
s = s.replace('return re.sub(r"[^a-z0-9]", "-", title.lower())', 'return re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")')
open(p, "w").write(s)
PY
