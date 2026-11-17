const products = [
  { name: "Desk lamp", price: "12.50" },
  { name: "Monitor stand", price: "100.00" },
  { name: "Cable ties", price: "5.25" },
  { name: "Mouse pad", price: "9.99" },
  { name: "Webcam", price: "45.00" },
];

products.sort((a, b) => a.price.localeCompare(b.price));

const list = document.getElementById("list");
for (const p of products) {
  const li = document.createElement("li");
  li.textContent = `${p.name}: ${p.price}`;
  list.appendChild(li);
}
