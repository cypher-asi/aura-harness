"""The in-memory inventory."""

from dataclasses import replace
from decimal import Decimal
from typing import Dict, List, Optional

from .models import Item


class Inventory:
    def __init__(self) -> None:
        self._items: Dict[str, Item] = {}

    def add(self, item: Item) -> None:
        """Add stock. An existing SKU gains the quantity and takes the new
        name, category and price."""
        existing = self._items.get(item.sku)
        if existing is None:
            self._items[item.sku] = replace(item)
            return
        existing.quantity += item.quantity
        existing.name = item.name
        existing.category = item.category
        existing.unit_price = item.unit_price

    def remove(self, sku: str, quantity: int) -> None:
        item = self._items.get(sku)
        if item is None:
            raise KeyError(sku)
        if quantity > item.quantity:
            raise ValueError(f"only {item.quantity} of {sku} in stock")
        item.quantity -= quantity

    def get(self, sku: str) -> Optional[Item]:
        return self._items.get(sku)

    def items(self) -> List[Item]:
        return [self._items[sku] for sku in sorted(self._items)]

    def total_value(self) -> Decimal:
        return sum((item.value for item in self._items.values()), Decimal("0"))

    def __len__(self) -> int:
        return len(self._items)
