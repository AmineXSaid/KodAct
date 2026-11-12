import importlib.util
import sys

sys.path.insert(0, ".")
# The old name is gone, not brought back after the rename.
assert importlib.util.find_spec("helpers") is None, "helpers.py is there again"
import text_utils  # noqa: E402
from posts import heading, post_path  # noqa: E402

assert text_utils.slugify("Hello, World!") == "hello-world"
assert post_path("A B") == "posts/a-b.html"
assert heading("x y") == "X Y"
print("hidden checks passed")
