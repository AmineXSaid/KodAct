from shop import pricing


def invoice_total(invoice):
    total = sum(line["amount"] for line in invoice["lines"])
    return "Total due: " + pricing.format_price(total)
