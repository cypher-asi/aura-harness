"""`python -m mdtools.cli outline FILE`"""

import argparse
import sys
from typing import List, Optional

from .headings import parse_headings


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="mdtools")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("outline").add_argument("file")
    return parser


def main(argv: Optional[List[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    with open(args.file, encoding="utf-8") as f:
        text = f.read()
    if args.command == "outline":
        for heading in parse_headings(text):
            print("  " * (heading.level - 1) + heading.text)
        return 0
    return 2


if __name__ == "__main__":
    sys.exit(main())
