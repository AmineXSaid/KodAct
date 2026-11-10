printf 'def run(text):\n    return text.upper()\n' > plugins/upper.py
printf 'def run(text):\n    return text[::-1]\n' > plugins/reverse.py
cat > registry.py <<'PY'
"""The plugins main.py can run, by name."""
from plugins import reverse, strip, upper

PLUGINS = {
    "strip": strip.run,
    "upper": upper.run,
    "reverse": reverse.run,
}
PY
