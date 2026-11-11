#!/bin/sh
mkdir -p reports
python3 - <<'PY'
import importlib, os, pkgutil, shared
for pkg in ["billing", "catalog", "shipping"]:
    for m in pkgutil.iter_modules([pkg]):
        importlib.import_module(f"{pkg}.{m.name}")
for pkg in ["billing", "catalog", "shipping"]:
    names = sorted(n for mod, n, _ in shared.DEPRECATED if mod.startswith(pkg + "."))
    open(f"reports/{pkg}.txt", "w").write("".join(n + "\n" for n in names))
PY
