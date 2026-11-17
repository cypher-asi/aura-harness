from textwidth import truncate

E = "\u0301"
cases = [
    ("hello", 5, "hello"),
    ("hello world", 5, "hell…"),
    ("日本語テキスト", 7, "日本語…"),
    ("日本語テキスト", 6, "日本…"),
    ("日本語", 6, "日本語"),
    ("e" + E + "te" + E, 4, "e" + E + "te" + E),
    ("cafe" + E + " au lait", 5, "cafe" + E + "…"),
    ("abcde" + E, 5, "abcde" + E),
    ("abcdef" + E, 5, "abcd…"),
    ("日本", 1, "…"),
    ("日本", 2, "…"),
    ("日本", 4, "日本"),
    ("ＡＢＣ", 4, "Ａ…"),
    ("", 3, ""),
]
for text, width, want in cases:
    got = truncate(text, width)
    assert got == want, (text, width, got, want)
print("ok")
