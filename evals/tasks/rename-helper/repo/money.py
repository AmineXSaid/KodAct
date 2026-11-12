def fmt_price(cents, currency="EUR"):
    """A price in cents, as people read it: `12.50 EUR`."""
    return f"{cents // 100}.{cents % 100:02d} {currency}"


def parse_price(text):
    amount, _, _currency = text.partition(" ")
    whole, _, frac = amount.partition(".")
    return int(whole) * 100 + int(frac or 0)
