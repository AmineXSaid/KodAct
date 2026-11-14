import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))
from members import parse_rows  # noqa: E402


class ParseRows(unittest.TestCase):
    def test_rows_are_read_in_order(self):
        self.assertEqual(parse_rows(["1,Ann", "2, Bo"]), [(1, "Ann"), (2, "Bo")])

    def test_a_row_without_an_id_is_skipped(self):
        self.assertEqual(parse_rows(["1,Ann", ",Bo", "3,Cy"]), [(1, "Ann"), (3, "Cy")])

    def test_a_repeated_id_keeps_its_first_row(self):
        self.assertEqual(parse_rows(["1,Ann", "1,Bo", "2,Cy"]), [(1, "Ann"), (2, "Cy")])


if __name__ == "__main__":
    unittest.main()
