"""Parse `key=value` lines into a dict."""


def parse(text):
    out = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        key, value = line.split("=")
        out[key.strip()] = value.strip()
    return out
