"""Saving the inventory as JSON between CLI runs."""

import json
import os
from decimal import Decimal

from .models import Item
from .store import Inventory


def load(path: str) -> Inventory:
    inventory = Inventory()
    if not os.path.exists(path):
        return inventory
    with open(path, encoding="utf-8") as f:
        for row in json.load(f):
            inventory.add(
                Item(
                    sku=row["sku"],
                    name=row["name"],
                    category=row["category"],
                    quantity=int(row["quantity"]),
                    unit_price=Decimal(row["unit_price"]),
                )
            )
    return inventory


def save(inventory: Inventory, path: str) -> None:
    rows = [
        {
            "sku": item.sku,
            "name": item.name,
            "category": item.category,
            "quantity": item.quantity,
            "unit_price": str(item.unit_price),
        }
        for item in inventory.items()
    ]
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(rows, f, indent=2)
    os.replace(tmp, path)
