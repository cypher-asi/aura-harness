import json
import pathlib


def load():
    """Settings from config.json next to this file."""
    return json.loads((pathlib.Path(__file__).parent / "config.json").read_text())
