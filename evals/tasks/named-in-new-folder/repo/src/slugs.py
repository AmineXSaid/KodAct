"""Slugs for page titles."""


def slugify(title):
    """`Hello, World!` -> `hello-world`."""
    words = "".join(c if c.isalnum() else " " for c in title.lower()).split()
    return "-".join(words)
