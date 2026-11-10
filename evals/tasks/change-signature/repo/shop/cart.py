from shop.pricing import total

TAX = 0.2


class Cart:
    def __init__(self):
        self.items = []

    def add(self, name, cents):
        self.items.append((name, cents))

    def checkout(self):
        return total([cents for _, cents in self.items], TAX)
