from helpers import slugify, title_case


def post_path(title):
    """Where a post with this title is published."""
    return f"posts/{slugify(title)}.html"


def heading(title):
    return title_case(title)
