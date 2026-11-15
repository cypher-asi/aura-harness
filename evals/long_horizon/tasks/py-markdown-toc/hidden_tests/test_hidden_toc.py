import contextlib
import io
import os
import tempfile
import unittest

from mdtools import cli
from mdtools.headings import parse_headings
from mdtools.slug import SlugRegistry, slugify
from mdtools.toc import build_toc, insert_toc

DOC = """# Project

<!-- toc -->
old contents
<!-- tocstop -->

## Install

```sh
# not a heading
## nor this
```

## Usage ##

### Options & flags

~~~~
## still code
~~~
## still code too
~~~~

## Usage

#### Deep detail
"""


class HiddenHeadingsTest(unittest.TestCase):
    def test_fences_and_closing_hashes(self):
        texts = [(h.level, h.text) for h in parse_headings(DOC)]
        self.assertEqual(
            texts,
            [(1, "Project"), (2, "Install"), (2, "Usage"), (3, "Options & flags"), (2, "Usage"), (4, "Deep detail")],
        )

    def test_unclosed_fence_runs_to_the_end(self):
        self.assertEqual([h.text for h in parse_headings("# A\n```\n# B\n")], ["A"])

    def test_indented_fence(self):
        self.assertEqual([h.text for h in parse_headings("   ```\n# B\n   ```\n# C\n")], ["C"])

    def test_hash_inside_title_is_kept(self):
        self.assertEqual(parse_headings("## C# tips\n")[0].text, "C# tips")


class HiddenSlugTest(unittest.TestCase):
    def test_slugify(self):
        self.assertEqual(slugify("Hello, World! 2.0"), "hello-world-20")
        self.assertEqual(slugify("  Options & flags "), "options--flags")
        self.assertEqual(slugify("snake_case-and-dash"), "snake_case-and-dash")

    def test_registry(self):
        registry = SlugRegistry()
        self.assertEqual([registry.unique("Usage") for _ in range(3)], ["usage", "usage-1", "usage-2"])
        self.assertEqual(registry.unique("Other"), "other")


class HiddenTocTest(unittest.TestCase):
    def test_build_toc(self):
        self.assertEqual(
            build_toc(DOC),
            "- [Install](#install)\n"
            "- [Usage](#usage)\n"
            "  - [Options & flags](#options--flags)\n"
            "- [Usage](#usage-1)",
        )

    def test_levels(self):
        self.assertEqual(
            build_toc(DOC, min_level=1, max_level=1),
            "- [Project](#project)",
        )
        self.assertIn("    - [Deep detail](#deep-detail)", build_toc(DOC, min_level=2, max_level=4))

    def test_insert_toc(self):
        updated = insert_toc(DOC)
        self.assertNotIn("old contents", updated)
        self.assertIn("<!-- toc -->\n- [Install](#install)\n", updated)
        self.assertIn("- [Usage](#usage-1)\n<!-- tocstop -->\n", updated)
        self.assertTrue(updated.endswith("#### Deep detail\n"))
        self.assertEqual(insert_toc(updated), updated)

    def test_missing_marker(self):
        with self.assertRaises(ValueError):
            insert_toc("# A\n<!-- toc -->\n")


class HiddenCliTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.path = os.path.join(self.dir.name, "doc.md")

    def run_cli(self, text, *argv):
        with open(self.path, "w", encoding="utf-8") as f:
            f.write(text)
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["toc", self.path, *argv])
        return code, out.getvalue(), err.getvalue()

    def test_print(self):
        code, out, _ = self.run_cli(DOC, "--max", "2")
        self.assertEqual(code, 0)
        self.assertEqual(out.strip().splitlines(), ["- [Install](#install)", "- [Usage](#usage)", "- [Usage](#usage-1)"])

    def test_in_place(self):
        code, out, _ = self.run_cli(DOC, "--in-place")
        self.assertEqual(code, 0)
        self.assertEqual(out, "")
        with open(self.path, encoding="utf-8") as f:
            self.assertEqual(f.read(), insert_toc(DOC))

    def test_in_place_without_markers(self):
        code, _, err = self.run_cli("# A\n", "--in-place")
        self.assertEqual(code, 1)
        self.assertTrue(err.startswith("error: "))


if __name__ == "__main__":
    unittest.main()
