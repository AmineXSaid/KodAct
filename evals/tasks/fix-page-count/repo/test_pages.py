import unittest

from pages import page_count, page_of


class PagesTest(unittest.TestCase):
    def test_full_pages(self):
        self.assertEqual(page_count(20, 10), 2)

    def test_partly_full_last_page(self):
        self.assertEqual(page_count(25, 10), 3)

    def test_page_of(self):
        self.assertEqual(page_of(0, 10), 1)
        self.assertEqual(page_of(10, 10), 2)


if __name__ == "__main__":
    unittest.main()
