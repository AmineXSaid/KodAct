python3 - <<'PY'
s = open("pricing.py").read()
s = s.replace("return gross * (100 - discount_percent) // 100", "return (gross * (100 - discount_percent) + 50) // 100")
s = s.replace("150 * ((weight_grams - 1000) // 1000)", "150 * ((weight_grams - 1000 + 999) // 1000)")
open("pricing.py", "w").write(s)
PY
