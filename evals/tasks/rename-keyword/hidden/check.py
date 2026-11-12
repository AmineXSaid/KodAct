"""fetch takes `attempts` now; every caller passes it; RETRIES and the clients' own parameters stay."""
import ast, importlib, inspect, os, sys

ws = os.getcwd()
sys.path.insert(0, ws)
import svc.net as net  # noqa: E402

params = list(inspect.signature(net.fetch).parameters)
if params != ["url", "attempts", "timeout"]:
    sys.exit(f"fetch's parameters are {params}")
if net.RETRIES != 3:
    sys.exit("RETRIES changed")
for i in range(24):
    name = f"svc.client_{i:02d}"
    try:
        m = importlib.import_module(name)
    except SyntaxError as e:
        sys.exit(f"{name} does not parse: {e}")
    src = open(os.path.join(ws, "svc", f"client_{i:02d}.py")).read()
    tree = ast.parse(src)
    for node in ast.walk(tree):
        if isinstance(node, ast.Call) and getattr(node.func, "id", "") == "fetch":
            kws = [k.arg for k in node.keywords]
            if "retries" in kws:
                sys.exit(f"{name} still calls fetch(retries=...)")
    r = getattr(m, f"get_{i}")()
    if "tries" not in r:
        sys.exit(f"{name}: unexpected result {r}")
    if i % 4 == 3 and "retries" not in inspect.signature(getattr(m, f"get_{i}")).parameters:
        sys.exit(f"{name}: the client's own `retries` parameter was renamed; only fetch's was asked")
print("ok")
