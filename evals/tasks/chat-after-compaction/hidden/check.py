"""chat-after-compaction: the last turn's file needs facts from the first two turns (the setting,
its old value, the failure count before the change), told across compactions."""
import pathlib, re, sys

settings = pathlib.Path("service/settings.toml").read_text()
want = {"pool_max_connections": "64", "worker_threads": "40", "listen_port": "8080", "cache_size_mb": "256"}
for key, value in want.items():
    m = re.search(rf"^{key} = (.+)$", settings, re.M)
    if not m or m.group(1).strip() != value:
        sys.exit(f"{key} should be {value}, is {m.group(1) if m else 'missing'}")
if settings.count("\n") != 22:
    sys.exit("other settings were added or removed")
changes = pathlib.Path("CHANGES.md")
if not changes.exists():
    sys.exit("CHANGES.md is missing")
lines = [l.strip().strip("`") for l in changes.read_text().splitlines() if l.strip()]
if lines != ["pool_max_connections: 8 -> 64", "failures: 3 -> 0"]:
    sys.exit(f"CHANGES.md is {lines!r}")
print("ok")
