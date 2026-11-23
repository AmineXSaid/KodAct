cat > shuffle.py <<'PY'
def riffle(xs):
    """A perfect riffle shuffle of the list xs: the first half (with the extra element when the
    length is odd) and the second half, alternating, starting with the first."""
    half = (len(xs) + 1) // 2
    first, second = xs[:half], xs[half:]
    out = []
    for i in range(half):
        out.append(first[i])
        if i < len(second):
            out.append(second[i])
    return out
PY
python3 test_shuffle.py
