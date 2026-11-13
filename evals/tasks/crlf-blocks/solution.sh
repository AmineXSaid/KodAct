python3 - <<'PY'
data = open("servers.ini", "rb").read()
old = b"retries = 3\r\nbackoff = 1\r\n"
new = b"retries = 5\r\nbackoff = 2\r\njitter = true\r\n"
for host in (b"b.example.org", b"c.example.org"):
    data = data.replace(host + b"\r\n" + old, host + b"\r\n" + new, 1)
open("servers.ini", "wb").write(data)
PY
