#!/usr/bin/env python3
"""Rank the allocation sites of a dhat heap profile by bytes live at the
heap's peak (`gb`), grouped by the first frame inside this workspace.

Usage: python3 scripts/dhat_top.py dhat-heap-200.json [N]
"""
import json
import re
import sys
from collections import defaultdict

WORKSPACE = re.compile(r"(aura_[a-z_]+)::([^ ]+)")


def site(frames, table):
    """The first frame of this workspace's code, without generics."""
    for index in frames:
        frame = table[index]
        found = WORKSPACE.search(frame)
        if found and "dhat" not in frame:
            name = re.sub(r"<[^<>]*>", "", found.group(0))
            return name.split(" (")[0]
    return table[frames[0]] if frames else "?"


def main():
    path = sys.argv[1]
    top = int(sys.argv[2]) if len(sys.argv) > 2 else 25
    with open(path) as file:
        profile = json.load(file)
    table = profile["ftbl"]
    by_site = defaultdict(lambda: [0, 0])
    total = 0
    for pp in profile["pps"]:
        live = pp.get("gb", 0)
        total += live
        entry = by_site[site(pp["fs"], table)]
        entry[0] += live
        entry[1] += pp.get("gbk", 0)
    print(f"{total / 2**20:.1f} MiB live at the peak")
    ranked = sorted(by_site.items(), key=lambda item: -item[1][0])
    for name, (live, blocks) in ranked[:top]:
        print(f"{live / 2**20:8.1f} MiB {blocks:>9} blocks  {name}")


if __name__ == "__main__":
    main()
