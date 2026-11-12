"""One report per package: the functions marked @deprecated there, one per line, sorted."""
import importlib, os, pkgutil, sys

ws = os.getcwd()
sys.path.insert(0, ws)
import shared  # noqa: E402

for pkg in ["billing", "catalog", "shipping"]:
    for m in pkgutil.iter_modules([os.path.join(ws, pkg)]):
        importlib.import_module(f"{pkg}.{m.name}")
for pkg in ["billing", "catalog", "shipping"]:
    want = sorted(name for mod, name, _ in shared.DEPRECATED if mod.startswith(pkg + "."))
    path = os.path.join(ws, "reports", f"{pkg}.txt")
    if not os.path.exists(path):
        sys.exit(f"reports/{pkg}.txt is missing")
    got = [l.strip() for l in open(path) if l.strip()]
    if got != want:
        sys.exit(f"reports/{pkg}.txt: got {len(got)} names, want {len(want)}; first difference near {sorted(set(got) ^ set(want))[:3]}")
print("ok")
