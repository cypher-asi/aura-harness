"""`python -m inventory.cli [--file PATH] <command> ...`"""

import argparse
import sys
from decimal import Decimal, InvalidOperation
from typing import List, Optional

from . import persistence
from .csv_io import CsvFormatError, export_csv, import_csv
from .models import Item


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="inventory")
    parser.add_argument("--file", default="inventory.json")
    sub = parser.add_subparsers(dest="command", required=True)

    add = sub.add_parser("add")
    add.add_argument("sku")
    add.add_argument("name")
    add.add_argument("category")
    add.add_argument("quantity", type=int)
    add.add_argument("unit_price")

    sub.add_parser("list")
    sub.add_parser("export").add_argument("path")
    sub.add_parser("import").add_argument("path")
    sub.add_parser("report")
    return parser


def main(argv: Optional[List[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    inventory = persistence.load(args.file)

    if args.command == "add":
        try:
            price = Decimal(args.unit_price)
            item = Item(args.sku, args.name, args.category, args.quantity, price)
        except (InvalidOperation, ValueError) as e:
            print(f"error: {e}", file=sys.stderr)
            return 1
        inventory.add(item)
        persistence.save(inventory, args.file)
        print(f"Added {item.quantity} x {item.sku}")
        return 0

    if args.command == "list":
        for item in inventory.items():
            print(f"{item.sku}\t{item.name}\t{item.category}\t{item.quantity}\t{item.unit_price:.2f}")
        return 0

    if args.command == "export":
        export_csv(inventory, args.path)
        print(f"Exported {len(inventory)} items")
        return 0

    if args.command == "import":
        with open(args.path, encoding="utf-8") as f:
            rows = max(sum(1 for _ in f) - 1, 0)
        try:
            import_csv(args.path, into=inventory)
        except CsvFormatError as e:
            print(f"error: line {e.line}: {e}", file=sys.stderr)
            return 1
        persistence.save(inventory, args.file)
        print(f"Imported {rows} rows")
        return 0

    if args.command == "report":
        for category, total in sorted(inventory.value_by_category().items()):
            print(f"{category}: {total:.2f}")
        print(f"TOTAL: {inventory.total_value():.2f}")
        return 0

    return 2


if __name__ == "__main__":
    sys.exit(main())
