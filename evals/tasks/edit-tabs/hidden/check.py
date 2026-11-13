import importlib.util

lines = open("fetch.py").read().splitlines()
spaced = [i + 1 for i, l in enumerate(lines) if l.startswith(" ")]
assert not spaced, f"fetch.py is indented with tabs; lines {spaced} start with spaces"

spec = importlib.util.spec_from_file_location("fetch", "fetch.py")
fetch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fetch)


class Client:
    def __init__(self, behaviour):
        self.behaviour, self.calls = behaviour, {}

    def get(self, url):
        self.calls[url] = self.calls.get(url, 0) + 1
        b = self.behaviour[url]
        if isinstance(b, type) and issubclass(b, BaseException):
            raise b()
        if isinstance(b, list):
            r = b.pop(0)
            if isinstance(r, type):
                raise r()
            return r
        return b


c = Client({"ok": "A", "denied": PermissionError, "slow": TimeoutError, "flaky": [TimeoutError, "B"]})
out = fetch.fetch_all(c, ["ok", "denied", "slow", "flaky"], retries=3)
assert out == {"ok": "A", "denied": None, "slow": None, "flaky": "B"}, out
assert c.calls["denied"] == 1, f"a PermissionError is not retried: {c.calls['denied']} calls"
assert c.calls["slow"] == 3, f"timeouts are still retried: {c.calls['slow']} calls"
print("hidden checks passed")
