cat > geometry.py <<'SOLUTION'
"""Small geometry helpers."""


def area(width, height):
    return width * height


def clamp(value, low, high):
    return max(low, min(high, value))


def scale(point, factor):
    x, y = point
    factor = clamp(factor, 0, 10)
    return (x * factor, y * factor)
SOLUTION
