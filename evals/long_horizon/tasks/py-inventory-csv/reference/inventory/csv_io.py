"""CSV import and export."""

import csv
from decimal import Decimal, InvalidOperation
from typing import List, Optional

from .models import Item
from .store import Inventory

COLUMNS = ["sku", "name", "category", "quantity", "unit_price"]


class CsvFormatError(ValueError):
    def __init__(self, line: int, message: str) -> None:
        super().__init__(message)
        self.line = line


def export_csv(inventory: Inventory, path: str) -> None:
    with open(path, "w", encoding="utf-8", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(COLUMNS)
        for item in inventory.items():
            writer.writerow([item.sku, item.name, item.category, item.quantity, f"{item.unit_price:.2f}"])


def _parse(rows: List[dict], first_line: int) -> List[Item]:
    items = []
    for offset, row in enumerate(rows):
        line = first_line + offset
        sku = (row["sku"] or "").strip()
        if not sku:
            raise CsvFormatError(line, "empty sku")
        try:
            quantity = int(row["quantity"])
        except (TypeError, ValueError):
            raise CsvFormatError(line, f"bad quantity {row['quantity']!r}") from None
        if quantity < 0:
            raise CsvFormatError(line, "negative quantity")
        try:
            price = Decimal(row["unit_price"])
        except (InvalidOperation, TypeError):
            raise CsvFormatError(line, f"bad price {row['unit_price']!r}") from None
        if not price.is_finite() or price < 0:
            raise CsvFormatError(line, "bad price")
        items.append(Item(sku, row["name"], row["category"], quantity, price))
    return items


def import_csv(path: str, into: Optional[Inventory] = None) -> Inventory:
    inventory = into if into is not None else Inventory()
    with open(path, encoding="utf-8", newline="") as f:
        reader = csv.DictReader(f)
        if sorted(reader.fieldnames or []) != sorted(COLUMNS):
            raise CsvFormatError(1, f"expected columns {','.join(COLUMNS)}")
        items = _parse(list(reader), 2)
    for item in items:
        inventory.add(item)
    return inventory
