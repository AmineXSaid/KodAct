"""Small geometry helpers."""


def area(width, height):
    return width * height


def scale(point, factor):
    x, y = point
    return (x * factor, y * factor)
