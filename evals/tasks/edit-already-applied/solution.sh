python3 - <<'PY'
p = "services.conf"
s = open(p).read()
open(p, "w").write(s.replace("host = api.internal\ntimeout = 30", "host = api.internal\ntimeout = 60", 1))
PY
