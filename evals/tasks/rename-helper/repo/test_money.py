import unittest

from money import fmt_price, parse_price
from receipt import receipt_lines


class MoneyTest(unittest.TestCase):
    def test_format(self):
        self.assertEqual(fmt_price(1250), "12.50 EUR")

    def test_round_trip(self):
        self.assertEqual(parse_price(fmt_price(905)), 905)

    def test_receipt_total(self):
        self.assertTrue(receipt_lines([("tea", 350), ("cake", 420)])[-1].endswith("7.70 EUR"))


if __name__ == "__main__":
    unittest.main()
