import unittest

from textutil import slugify, truncate


class TextUtil(unittest.TestCase):
    def test_slugify(self):
        self.assertEqual(slugify("Hello,  World!"), "hello-world")
        self.assertEqual(slugify("  KodAct 2 "), "kodact-2")

    def test_truncate(self):
        self.assertEqual(truncate("abcdef", 4), "abc…")


if __name__ == "__main__":
    unittest.main()
