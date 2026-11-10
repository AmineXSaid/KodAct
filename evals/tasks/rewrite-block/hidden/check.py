import sys

sys.path.insert(0, ".")
import vat

WANT = {'AT': 20, 'BE': 21, 'BG': 20, 'HR': 25, 'CY': 19, 'CZ': 21, 'DK': 25, 'EE': 22, 'FI': 25.5, 'FR': 20, 'DE': 19, 'GR': 24, 'HU': 27, 'IE': 23, 'IT': 22, 'LV': 21, 'LT': 21, 'LU': 17, 'MT': 18, 'NL': 21, 'PL': 23, 'PT': 23, 'RO': 19, 'SK': 23, 'SI': 22, 'ES': 21, 'SE': 25}
if getattr(vat, "RATES", None) != WANT:
    sys.exit("RATES is missing or does not hold every rate")
for c, r in WANT.items():
    if vat.vat_rate(c) != r or vat.vat_rate(c.lower()) != r:
        sys.exit(f"vat_rate({c!r}) is wrong")
try:
    vat.vat_rate("XX")
except ValueError as e:
    if str(e) != "unknown country: XX":
        sys.exit(f"wrong message: {e}")
else:
    sys.exit("an unknown country must raise ValueError")
if "elif" in open("vat.py").read():
    sys.exit("the chain is still there")
if vat.with_vat(100, "FI") != 125.5 or not vat.vat_rate.__doc__:
    sys.exit("with_vat or the docstring changed")
print("ok")
