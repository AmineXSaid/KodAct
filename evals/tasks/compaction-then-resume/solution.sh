python3 - <<'PY'
s = open("pipeline/stages.py").read()
s = s.replace('report."""\n', 'report."""\n\nfrom decimal import Decimal\n', 1)
s = s.replace('"amount": int(amount)}', '"amount": Decimal(amount)}')
s = s.replace('int(record["amount"] * 10)', 'int(record["amount"] * 100)')
s = s.replace('return record["region"].lower()', 'return record["region"].strip().lower()')
s = s.replace('f"{region}: {cents / 100}"', 'f"{region}: {cents // 100}.{cents % 100:02d}"')
open("pipeline/stages.py", "w").write(s)
PY
printf 'stages = ["parse", "normalize", "aggregate", "report"]\n' > FIXES.toml
