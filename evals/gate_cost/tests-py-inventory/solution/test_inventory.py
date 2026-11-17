import unittest

from inventory import Inventory


class InventoryTest(unittest.TestCase):
    def test_add_and_count(self):
        inv = Inventory()
        self.assertEqual(inv.count("apple"), 0)
        inv.add("apple", 3)
        inv.add("apple")
        self.assertEqual(inv.count("apple"), 4)

    def test_remove(self):
        inv = Inventory()
        inv.add("pear", 5)
        inv.remove("pear", 2)
        self.assertEqual(inv.count("pear"), 3)

    def test_remove_too_many(self):
        inv = Inventory()
        inv.add("fig", 1)
        with self.assertRaises(ValueError):
            inv.remove("fig", 2)
        self.assertEqual(inv.count("fig"), 1)


if __name__ == "__main__":
    unittest.main()
