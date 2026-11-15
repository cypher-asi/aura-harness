import contextlib
import io
import os
import tempfile
import unittest

from mdtools import cli
from mdtools.headings import Heading, parse_headings


class HeadingsTest(unittest.TestCase):
    def test_levels_and_lines(self):
        text = "# Title\n\nSome text\n## Part one\n### Detail\n"
        self.assertEqual(
            parse_headings(text),
            [Heading(1, "Title", 1), Heading(2, "Part one", 4), Heading(3, "Detail", 5)],
        )

    def test_needs_a_space_after_hashes(self):
        self.assertEqual(parse_headings("#hashtag\n"), [])

    def test_outline(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "doc.md")
            with open(path, "w", encoding="utf-8") as f:
                f.write("# A\n## B\n")
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                self.assertEqual(cli.main(["outline", path]), 0)
            self.assertEqual(out.getvalue(), "A\n  B\n")


if __name__ == "__main__":
    unittest.main()
