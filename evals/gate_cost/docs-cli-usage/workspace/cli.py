import argparse


def main():
    parser = argparse.ArgumentParser(description="Print a greeting.")
    parser.add_argument("--name", default="world", help="who to greet")
    parser.add_argument("--shout", action="store_true", help="print in capitals")
    parser.add_argument("--times", type=int, default=1, help="how many times to print it")
    args = parser.parse_args()
    line = f"hello, {args.name}"
    for _ in range(args.times):
        print(line.upper() if args.shout else line)


if __name__ == "__main__":
    main()
