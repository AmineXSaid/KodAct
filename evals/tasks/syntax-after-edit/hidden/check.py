import json
import subprocess
import sys

with open("services.json") as f:
    config = json.load(f)  # a file that no longer parses fails here
by_name = {s["name"]: s for s in config["services"]}
assert set(by_name) == {"auth", "db", "catalog", "search", "billing"}, sorted(by_name)
billing = by_name["billing"]
assert billing["port"] == 8085 and billing["owner"] == "finance", billing
assert sorted(billing["depends_on"]) == ["auth", "db"], billing
assert by_name["db"]["port"] == 5433, by_name["db"]
assert by_name["catalog"]["limits"] == {"rps": 200, "burst": [250, 300]}, "other entries are untouched"
assert config["version"] == 3 and config["defaults"] == {"retries": 2, "timeout_ms": 1500}
out = subprocess.run([sys.executable, "report.py"], capture_output=True, text=True)
assert out.returncode == 0, out.stderr
lines = out.stdout.split()
assert "billing:8085" in lines and "db:5433" in lines, out.stdout
