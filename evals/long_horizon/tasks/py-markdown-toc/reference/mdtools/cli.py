"""`python -m mdtools.cli outline FILE` / `toc FILE`"""

import argparse
import sys
from typing import List, Optional

from .headings import parse_headings
from .toc import build_toc, insert_toc


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="mdtools")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("outline").add_argument("file")
    toc = sub.add_parser("toc")
    toc.add_argument("file")
    toc.add_argument("--min", type=int, default=2)
    toc.add_argument("--max", type=int, default=3)
    toc.add_argument("--in-place", action="store_true")
    return parser


def main(argv: Optional[List[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    with open(args.file, encoding="utf-8") as f:
        text = f.read()
    if args.command == "outline":
        for heading in parse_headings(text):
            print("  " * (heading.level - 1) + heading.text)
        return 0
    if args.command == "toc":
        if not args.in_place:
            print(build_toc(text, args.min, args.max))
            return 0
        try:
            updated = insert_toc(text, args.min, args.max)
        except ValueError as e:
            print(f"error: {e}", file=sys.stderr)
            return 1
        with open(args.file, "w", encoding="utf-8") as f:
            f.write(updated)
        return 0
    return 2


if __name__ == "__main__":
    sys.exit(main())
