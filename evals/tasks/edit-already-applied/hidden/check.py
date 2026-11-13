import os
src = open(os.path.join(os.environ["KODACT_EVAL_TASK_DIR"], "repo", "services.conf")).read()
expected = src.replace("host = api.internal\ntimeout = 30", "host = api.internal\ntimeout = 60", 1)
got = open("services.conf").read()
assert got == expected, f"only the api timeout should change, to 60:\n{got}"
print("hidden checks passed")
