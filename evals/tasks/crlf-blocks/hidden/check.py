data = open("servers.ini", "rb").read()
expected = b'[primary]\r\nhost = a.example.org\r\nretries = 3\r\nbackoff = 1\r\n\r\n[staging]\r\nhost = b.example.org\r\nretries = 5\r\nbackoff = 2\r\njitter = true\r\n\r\n[backup]\r\nhost = c.example.org\r\nretries = 5\r\nbackoff = 2\r\njitter = true\r\n'
assert data == expected, f"servers.ini differs from the expected bytes:\n{data!r}"
print("hidden checks passed")
