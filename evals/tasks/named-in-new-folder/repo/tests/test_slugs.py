import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))
from slugs import slugify  # noqa: E402


class SlugTest(unittest.TestCase):
    def test_words_are_joined_by_dashes(self):
        self.assertEqual(slugify("Hello, World!"), "hello-world")


if __name__ == "__main__":
    unittest.main()
