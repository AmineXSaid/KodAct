from shop.cart import Cart
from shop.report import daily

if __name__ == "__main__":
    cart = Cart()
    cart.add("pen", 250)
    cart.add("ink", 1000)
    print("cart:", cart.checkout())
    print("day:", daily([[250, 1000], [500]]))
