cat > src/members.py <<'PY'
"""Read the rows of a members export: one `id,name` per line."""


def parse_rows(lines):
    """The (id, name) pairs of the rows, in order, with the id as an int.

    A row without an id is skipped, and a repeated id keeps its first row.
    """
    rows, seen = [], set()
    for line in lines:
        line = line.strip()
        if not line:
            continue
        id_text, name = line.split(",", 1)
        if not id_text.strip():
            continue
        row_id = int(id_text)
        if row_id in seen:
            continue
        seen.add(row_id)
        rows.append((row_id, name.strip()))
    return rows
PY
