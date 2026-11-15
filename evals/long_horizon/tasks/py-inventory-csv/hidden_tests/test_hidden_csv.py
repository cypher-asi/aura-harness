import contextlib
import io
import os
import tempfile
import unittest
from decimal import Decimal

from inventory import cli, persistence
from inventory.csv_io import CsvFormatError, export_csv, import_csv
from inventory.models import Item
from inventory.store import Inventory


def sample():
    inv = Inventory()
    inv.add(Item("B2", "Glue", "supplies", 2, Decimal("3.5")))
    inv.add(Item("A1", "Bolt", "hardware", 10, Decimal("0.25")))
    inv.add(Item("C3", "Nut", "hardware", 4, Decimal("0.1")))
    return inv


class HiddenCsvTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def path(self, name):
        return os.path.join(self.dir.name, name)

    def write(self, name, text):
        p = self.path(name)
        with open(p, "w", encoding="utf-8", newline="") as f:
            f.write(text)
        return p

    def test_export_format(self):
        p = self.path("out.csv")
        export_csv(sample(), p)
        with open(p, encoding="utf-8") as f:
            lines = [line.rstrip("\r\n") for line in f]
        self.assertEqual(lines[0], "sku,name,category,quantity,unit_price")
        self.assertEqual(lines[1], "A1,Bolt,hardware,10,0.25")
        self.assertEqual(lines[2], "B2,Glue,supplies,2,3.50")
        self.assertEqual(lines[3], "C3,Nut,hardware,4,0.10")

    def test_round_trip(self):
        p = self.path("out.csv")
        export_csv(sample(), p)
        inv = import_csv(p)
        self.assertEqual(len(inv), 3)
        self.assertEqual(inv.get("B2").unit_price, Decimal("3.50"))
        self.assertEqual(inv.get("A1").quantity, 10)

    def test_columns_in_any_order_and_merge(self):
        p = self.write("in.csv", "quantity,sku,unit_price,category,name\n5,A1,0.30,hardware,Bolt M4\n1,Z9,9.99,tools,Saw\n")
        inv = sample()
        result = import_csv(p, into=inv)
        self.assertIs(result, inv)
        self.assertEqual(inv.get("A1").quantity, 15)
        self.assertEqual(inv.get("A1").unit_price, Decimal("0.30"))
        self.assertEqual(inv.get("Z9").category, "tools")

    def test_bad_rows_report_their_line_and_change_nothing(self):
        cases = {
            "sku,name,category,quantity\nA1,Bolt,hardware,1\n": 1,
            "sku,name,category,quantity,unit_price,extra\nA1,Bolt,hardware,1,1,x\n": 1,
            "sku,name,category,quantity,unit_price\nA1,Bolt,hardware,1,1\nB2,Glue,supplies,two,1\n": 3,
            "sku,name,category,quantity,unit_price\nA1,Bolt,hardware,-1,1\n": 2,
            "sku,name,category,quantity,unit_price\nA1,Bolt,hardware,1,abc\n": 2,
            "sku,name,category,quantity,unit_price\nA1,Bolt,hardware,1,1\n,Nameless,misc,1,1\n": 3,
        }
        for text, line in cases.items():
            with self.subTest(text=text):
                inv = sample()
                p = self.write("bad.csv", text)
                with self.assertRaises(CsvFormatError) as caught:
                    import_csv(p, into=inv)
                self.assertIsInstance(caught.exception, ValueError)
                self.assertEqual(caught.exception.line, line)
                self.assertEqual(inv.get("A1").quantity, 10)
                self.assertEqual(len(inv), 3)

    def test_value_by_category(self):
        self.assertEqual(
            sample().value_by_category(),
            {"hardware": Decimal("2.90"), "supplies": Decimal("7.00")},
        )
        self.assertEqual(Inventory().value_by_category(), {})

    def run_cli(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["--file", self.path("inv.json"), *argv])
        return code, out.getvalue(), err.getvalue()

    def test_cli_export_import_report(self):
        persistence.save(sample(), self.path("inv.json"))
        code, out, _ = self.run_cli("export", self.path("x.csv"))
        self.assertEqual(code, 0)
        self.assertIn("Exported 3 items", out)

        csv_path = self.write("more.csv", "sku,name,category,quantity,unit_price\nD4,Tape,supplies,3,2.00\nA1,Bolt,hardware,10,0.25\n")
        code, out, _ = self.run_cli("import", csv_path)
        self.assertEqual(code, 0)
        self.assertIn("Imported 2 rows", out)
        self.assertEqual(persistence.load(self.path("inv.json")).get("A1").quantity, 20)

        code, out, _ = self.run_cli("report")
        self.assertEqual(code, 0)
        self.assertEqual(
            out.strip().splitlines(),
            ["hardware: 5.40", "supplies: 13.00", "TOTAL: 18.40"],
        )

    def test_cli_import_error_leaves_file_alone(self):
        persistence.save(sample(), self.path("inv.json"))
        with open(self.path("inv.json"), encoding="utf-8") as f:
            before = f.read()
        bad = self.write("bad.csv", "sku,name,category,quantity,unit_price\nA1,Bolt,hardware,1,1\nB2,Glue,supplies,x,1\n")
        code, _, err = self.run_cli("import", bad)
        self.assertEqual(code, 1)
        self.assertIn("error: line 3:", err)
        with open(self.path("inv.json"), encoding="utf-8") as f:
            self.assertEqual(f.read(), before)


if __name__ == "__main__":
    unittest.main()
