"""Inventory records."""

from dataclasses import dataclass
from decimal import Decimal


@dataclass
class Item:
    sku: str
    name: str
    category: str
    quantity: int
    unit_price: Decimal

    def __post_init__(self) -> None:
        if not self.sku or not self.sku.strip():
            raise ValueError("sku is required")
        if self.quantity < 0:
            raise ValueError("quantity cannot be negative")
        if self.unit_price < 0:
            raise ValueError("unit_price cannot be negative")

    @property
    def value(self) -> Decimal:
        return self.unit_price * self.quantity
