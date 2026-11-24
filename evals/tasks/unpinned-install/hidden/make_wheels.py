"""unpinned-install: two releases of a small table library, as a local package index.

tinyfmt 1.0 takes the separator as `sep`; 2.0 renamed it `separator` and dropped `sep`. Both
take it as the second positional argument. `pip install tinyfmt` installs 2.0, the latest;
the check runs the report under 1.0 as well, the version an older environment would have.
Wheels are written by hand (a zip with the module and its metadata): no network, no build
tools.
"""
import base64
import hashlib
import sys
import zipfile
from pathlib import Path

V1 = '''"""tinyfmt 1.0: plain text tables."""
__version__ = "1.0"


def fmt_table(rows, sep="  "):
    """Format rows (lists of values) as aligned text, columns joined by sep."""
    rows = [[str(c) for c in r] for r in rows]
    widths = [max(len(r[i]) for r in rows) for i in range(len(rows[0]))]
    return "\\n".join(sep.join(c.ljust(w) for c, w in zip(r, widths)).rstrip() for r in rows)
'''

V2 = '''"""tinyfmt 2.0: plain text tables. `sep` is now `separator`."""
__version__ = "2.0"


def fmt_table(rows, separator="  ", *, align="left"):
    """Format rows (lists of values) as aligned text, columns joined by separator."""
    rows = [[str(c) for c in r] for r in rows]
    widths = [max(len(r[i]) for r in rows) for i in range(len(rows[0]))]
    pad = str.rjust if align == "right" else str.ljust
    return "\\n".join(separator.join(pad(c, w) for c, w in zip(r, widths)).rstrip() for r in rows)
'''


def record_line(name, data):
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
    return f"{name},sha256={digest},{len(data)}"


def wheel(out: Path, version: str, source: str) -> Path:
    info = f"tinyfmt-{version}.dist-info"
    files = {
        "tinyfmt.py": source.encode(),
        f"{info}/METADATA": f"Metadata-Version: 2.1\nName: tinyfmt\nVersion: {version}\nSummary: Plain text tables\n".encode(),
        f"{info}/WHEEL": b"Wheel-Version: 1.0\nGenerator: hand\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
    }
    record = [record_line(n, d) for n, d in files.items()] + [f"{info}/RECORD,,"]
    files[f"{info}/RECORD"] = ("\n".join(record) + "\n").encode()
    path = out / f"tinyfmt-{version}-py3-none-any.whl"
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for n, d in files.items():
            z.writestr(n, d)
    return path


if __name__ == "__main__":
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "wheels")
    out.mkdir(parents=True, exist_ok=True)
    for v, src in (("1.0", V1), ("2.0", V2)):
        print(wheel(out, v, src))
