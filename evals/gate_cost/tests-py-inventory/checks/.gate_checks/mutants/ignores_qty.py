class Inventory:
    """Stock counts by item name."""

    def __init__(self):
        self._stock = {}

    def add(self, item, qty=1):
        """Add `qty` units of `item`."""
        self._stock[item] = self._stock.get(item, 0) + 1

    def remove(self, item, qty=1):
        """Take `qty` units of `item`; ValueError when fewer are in stock."""
        have = self._stock.get(item, 0)
        if qty > have:
            raise ValueError(f"only {have} {item} in stock")
        self._stock[item] = have - qty

    def count(self, item):
        """Units of `item` in stock."""
        return self._stock.get(item, 0)
