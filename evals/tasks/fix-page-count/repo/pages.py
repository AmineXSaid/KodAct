def page_count(items, per_page):
    """How many pages `items` things fill at `per_page` a page; the last page may be partly full."""
    if per_page <= 0:
        raise ValueError("per_page must be positive")
    return items // per_page


def page_of(index, per_page):
    """The page (from 1) that holds the item at `index` (from 0)."""
    return index // per_page + 1
