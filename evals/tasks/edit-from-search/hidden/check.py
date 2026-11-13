import sys
sys.path.insert(0, ".")
from clients import auth, billing, catalog, email, ledger, search, shipping, storage, users, webhooks

want = {auth: 3, billing: 20, catalog: 8, email: 10, ledger: 15, search: 30, shipping: 2, storage: 12, users: 6, webhooks: 9}
for mod, t in want.items():
    got = mod.request("/ping")["timeout"]
    if got != t:
        sys.exit(f"{mod.__name__}: timeout {got}, want {t}")
if billing.request("/ping")["retries"] != 2:
    sys.exit("billing retries changed")
print("ok")
