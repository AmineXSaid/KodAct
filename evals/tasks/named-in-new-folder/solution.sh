mkdir -p lib/wordcount
cat > lib/wordcount/counter.py <<'PY'
"""Words in a text."""


def count_words(text):
    """How many words `text` has: they are separated by any whitespace."""
    return len(text.split())
PY
cat > tests/test_counter.py <<'PY'
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from lib.wordcount.counter import count_words  # noqa: E402


class CountTest(unittest.TestCase):
    def test_blank_text_has_no_words(self):
        self.assertEqual(count_words(" \t\n"), 0)

    def test_any_whitespace_separates_words(self):
        self.assertEqual(count_words("tabs\tand\nnew lines"), 4)


if __name__ == "__main__":
    unittest.main()
PY
