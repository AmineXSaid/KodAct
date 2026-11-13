import re


def slugify(title):
    """Lower-case words joined by single dashes: "Hello,  World!" -> "hello-world"."""
    return re.sub(r"[^a-z0-9]", "-", title.lower())


def truncate(text, width):
    if len(text) <= width:
        return text
    return text[: width - 1] + "…"
