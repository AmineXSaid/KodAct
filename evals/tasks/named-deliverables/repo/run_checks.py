"""The project's checks."""
from parser import parse

cases = [
    ("a=1\nb=2", {"a": "1", "b": "2"}),
    ("# note\n\nname = kod", {"name": "kod"}),
    ("url=http://x/?q=1", {"url": "http://x/?q=1"}),
]
for text, want in cases:
    got = parse(text)
    assert got == want, f"parse({text!r}) = {got!r}, want {want!r}"
print("all checks pass")
