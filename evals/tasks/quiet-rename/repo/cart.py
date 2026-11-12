from pricing import calc


def cart_total(lines):
    return round(sum(calc(q, p) for q, p in lines), 2)
