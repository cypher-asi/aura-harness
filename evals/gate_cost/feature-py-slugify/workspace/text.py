def slugify(title):
    """Turn a title into a URL slug.

    Accented Latin letters are first reduced to their base letter
    ("Café" -> "cafe"). Lowercase ASCII letters and digits are kept;
    every run of other characters becomes a single hyphen; the slug
    has no leading or trailing hyphens.
    """
    raise NotImplementedError
