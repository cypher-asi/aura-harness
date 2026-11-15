import sys

from billing import calc_total as total


def main(argv):
    items = [(name, float(price)) for name, price in (a.split("=") for a in argv)]
    print(total(items))


if __name__ == "__main__":
    main(sys.argv[1:])
