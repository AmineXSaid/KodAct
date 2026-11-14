"""Read the rows of a members export: one `id,name` per line."""


def parse_rows(lines):
    """The (id, name) pairs of the rows, in order, with the id as an int."""
    rows = []
    for line in lines:
        line = line.strip()
        if not line:
            continue
        id_text, name = line.split(",", 1)
        rows.append((int(id_text), name.strip()))
    return rows
