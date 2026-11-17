sed -i 's/products.sort((a, b) => a.price.localeCompare(b.price));/products.sort((a, b) => Number(a.price) - Number(b.price));/' prices.js
