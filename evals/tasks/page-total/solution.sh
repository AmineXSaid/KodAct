sed -i 's/items.reduce((a, b) => a.price \* a.qty + b.price \* b.qty)/items.reduce((sum, item) => sum + item.price * item.qty, 0)/' app.js
