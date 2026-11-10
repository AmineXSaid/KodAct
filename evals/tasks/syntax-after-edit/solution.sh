python3 - <<'PY'
import json
c = json.load(open("services.json"))
for s in c["services"]:
    if s["name"] == "db":
        s["port"] = 5433
c["services"].append({"name": "billing", "port": 8085, "owner": "finance", "depends_on": ["auth", "db"]})
json.dump(c, open("services.json", "w"), indent=2)
PY
