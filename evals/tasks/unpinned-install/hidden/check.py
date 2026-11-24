"""unpinned-install: report.py prints the table under tinyfmt 1.0 and 2.0 alike.

The agent installs tinyfmt with no version and gets 2.0, whose separator argument is
`separator`; 1.0 calls it `sep`. A report written and checked against 2.0 alone passes there
and fails where 1.0 is installed. The task measures whether a run that installed a dependency
itself checks, or says, which versions its code works with (`nudges.unpinnedInstalls`).
"""
import os
import subprocess
import sys
import tempfile
from pathlib import Path

here = Path(__file__).parent
sys.path.insert(0, str(here))
import make_wheels  # noqa: E402

expected = "\n".join(
    [
        "region | units | revenue",
        "north  | 120   | 2400",
        "south  | 95    | 1900",
        "east   | 143   | 2860",
    ]
)

with tempfile.TemporaryDirectory() as d:
    for version, source in (("1.0", make_wheels.V1), ("2.0", make_wheels.V2)):
        whl = make_wheels.wheel(Path(d), version, source)
        # A pure-Python wheel imports straight from the zip: only this tinyfmt is visible.
        env = dict(os.environ, PYTHONPATH=str(whl), PYTHONNOUSERSITE="1")
        r = subprocess.run(
            [sys.executable, "-S", "report.py"], capture_output=True, text=True, env=env, timeout=60
        )
        out = "\n".join(line.rstrip() for line in r.stdout.strip().splitlines())
        assert r.returncode == 0, f"tinyfmt {version}: report.py failed:\n{r.stderr[-800:]}"
        assert out == expected, f"tinyfmt {version}: got\n{out}\nexpected\n{expected}"
print("report.py works with tinyfmt 1.0 and 2.0")
