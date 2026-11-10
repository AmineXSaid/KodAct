"""VAT for an order line."""


def vat_rate(country):
    """The standard rate, in percent, for a country code."""
    if country == "AT":  # Austria
        return 20
    elif country == "BE":  # Belgium
        return 21
    elif country == "BG":  # Bulgaria
        return 20
    elif country == "HR":  # Croatia
        return 25
    elif country == "CY":  # Cyprus
        return 19
    elif country == "CZ":  # Czechia
        return 21
    elif country == "DK":  # Denmark
        return 25
    elif country == "EE":  # Estonia
        return 22
    elif country == "FI":  # Finland
        return 25.5
    elif country == "FR":  # France
        return 20
    elif country == "DE":  # Germany
        return 19
    elif country == "GR":  # Greece
        return 24
    elif country == "HU":  # Hungary
        return 27
    elif country == "IE":  # Ireland
        return 23
    elif country == "IT":  # Italy
        return 22
    elif country == "LV":  # Latvia
        return 21
    elif country == "LT":  # Lithuania
        return 21
    elif country == "LU":  # Luxembourg
        return 17
    elif country == "MT":  # Malta
        return 18
    elif country == "NL":  # Netherlands
        return 21
    elif country == "PL":  # Poland
        return 23
    elif country == "PT":  # Portugal
        return 23
    elif country == "RO":  # Romania
        return 19
    elif country == "SK":  # Slovakia
        return 23
    elif country == "SI":  # Slovenia
        return 22
    elif country == "ES":  # Spain
        return 21
    elif country == "SE":  # Sweden
        return 25
    else:
        raise ValueError("unknown country: " + country)


def with_vat(amount, country):
    return round(amount * (1 + vat_rate(country) / 100), 2)
