def page_count(items, per_page):
    """How many pages a listing of `items` entries needs."""
    return items // per_page


def page(entries, number, per_page):
    """The entries on page `number` (1 is the first)."""
    start = (number - 1) * per_page
    return entries[start:start + per_page]
