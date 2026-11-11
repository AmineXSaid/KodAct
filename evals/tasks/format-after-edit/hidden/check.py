import shutil
import subprocess
import sys

sys.path.insert(0, ".")
import geometry

if geometry.clamp(5, 0, 3) != 3 or geometry.clamp(-1, 0, 3) != 0 or geometry.clamp(2, 0, 3) != 2:
    sys.exit("clamp is wrong")
if geometry.scale((1, 2), 50) != (10, 20) or geometry.scale((1, 2), -3) != (0, 0) or geometry.scale((1, 2), 2) != (2, 4):
    sys.exit("scale does not limit the factor to 0..10")
if geometry.area(2, 3) != 6:
    sys.exit("area changed")
# The project's own check, with whichever of its formatters this machine has.
for cmd in (["ruff", "format", "--check", "geometry.py"], ["black", "-q", "--check", "geometry.py"]):
    if shutil.which(cmd[0]):
        r = subprocess.run(cmd, capture_output=True, text=True)
        if r.returncode != 0:
            sys.exit(f"geometry.py is not formatted ({cmd[0]}):\n{r.stdout}{r.stderr}")
        break
print("ok")
