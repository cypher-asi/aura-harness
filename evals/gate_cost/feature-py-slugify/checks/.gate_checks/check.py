from text import slugify

cases = {
    "Hello, World!": "hello-world",
    "  Café au lait  ": "cafe-au-lait",
    "Rust 2024 -- edition": "rust-2024-edition",
    "---": "",
    "Ünïcödé": "unicode",
    "already-a-slug": "already-a-slug",
}
for title, slug in cases.items():
    assert slugify(title) == slug, (title, slugify(title))
