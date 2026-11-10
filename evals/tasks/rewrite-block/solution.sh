cat > vat.py <<'SOLUTION'
"""VAT for an order line."""

RATES = {'AT': 20, 'BE': 21, 'BG': 20, 'HR': 25, 'CY': 19, 'CZ': 21, 'DK': 25, 'EE': 22, 'FI': 25.5, 'FR': 20, 'DE': 19, 'GR': 24, 'HU': 27, 'IE': 23, 'IT': 22, 'LV': 21, 'LT': 21, 'LU': 17, 'MT': 18, 'NL': 21, 'PL': 23, 'PT': 23, 'RO': 19, 'SK': 23, 'SI': 22, 'ES': 21, 'SE': 25}


def vat_rate(country):
    """The standard rate, in percent, for a country code."""
    try:
        return RATES[country.upper()]
    except KeyError:
        raise ValueError("unknown country: " + country) from None


def with_vat(amount, country):
    return round(amount * (1 + vat_rate(country) / 100), 2)
SOLUTION
