UNITS = {"s": 1, "m": 60, "h": 3600}


def parse_duration(text):
    """Seconds in a duration such as "45s", "90m" or "2h"."""
    text = text.strip().lower()
    number = text.rstrip("smh")
    unit = text[len(number) :]
    return int(number) * UNITS.get(unit[1:], 1)
