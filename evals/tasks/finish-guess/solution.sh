python3 - <<'PY'
p = "bill.py"
s = open(p).read()
s = s.replace(
    "    raise NotImplementedError",
    "    base, extra = divmod(total_cents, people)\n    return [base + 1 if i < extra else base for i in range(people)]",
)
open(p, "w").write(s)
PY
