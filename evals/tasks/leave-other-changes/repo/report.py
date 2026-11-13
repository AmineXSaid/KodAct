from textutil import truncate

PAGE_SIZE = 50
TITLE_WIDTH = 40


def pages(rows):
    return [rows[i : i + PAGE_SIZE] for i in range(0, len(rows), PAGE_SIZE)]


def heading(title):
    return truncate(title, TITLE_WIDTH).upper()
