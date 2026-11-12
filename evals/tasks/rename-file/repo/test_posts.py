import unittest

import helpers
from posts import heading, post_path


class PostsTest(unittest.TestCase):
    def test_path(self):
        self.assertEqual(post_path("Hello, World!"), "posts/hello-world.html")

    def test_heading(self):
        self.assertEqual(heading("a short note"), "A Short Note")

    def test_slugify_trims_dashes(self):
        self.assertEqual(helpers.slugify("--x--"), "x")


if __name__ == "__main__":
    unittest.main()
