import os
import tempfile
import unittest
from decimal import Decimal

from inventory import cli, persistence
from inventory.models import Item
from inventory.store import Inventory


class InventoryTest(unittest.TestCase):
    def test_add_merges_existing_sku(self):
        inv = Inventory()
        inv.add(Item("A1", "Bolt", "hardware", 10, Decimal("0.25")))
        inv.add(Item("A1", "Bolt M4", "hardware", 5, Decimal("0.30")))
        item = inv.get("A1")
        self.assertEqual(item.quantity, 15)
        self.assertEqual(item.unit_price, Decimal("0.30"))
        self.assertEqual(item.name, "Bolt M4")

    def test_total_value(self):
        inv = Inventory()
        inv.add(Item("A1", "Bolt", "hardware", 10, Decimal("0.25")))
        inv.add(Item("B2", "Glue", "supplies", 2, Decimal("3.50")))
        self.assertEqual(inv.total_value(), Decimal("9.50"))

    def test_rejects_negative_quantity(self):
        with self.assertRaises(ValueError):
            Item("A1", "Bolt", "hardware", -1, Decimal("1"))

    def test_persistence_round_trip(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "inv.json")
            inv = Inventory()
            inv.add(Item("A1", "Bolt", "hardware", 10, Decimal("0.25")))
            persistence.save(inv, path)
            self.assertEqual(persistence.load(path).get("A1").quantity, 10)

    def test_cli_add_and_list(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "inv.json")
            self.assertEqual(cli.main(["--file", path, "add", "A1", "Bolt", "hardware", "3", "0.5"]), 0)
            self.assertEqual(persistence.load(path).get("A1").quantity, 3)


if __name__ == "__main__":
    unittest.main()
