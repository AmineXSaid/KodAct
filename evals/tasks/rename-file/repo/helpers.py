import re


def slugify(text):
    """`Hello, World!` becomes `hello-world`."""
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


def title_case(text):
    return " ".join(word.capitalize() for word in text.split())
