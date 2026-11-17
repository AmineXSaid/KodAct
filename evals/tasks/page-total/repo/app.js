const items = [
  { name: "Notebook", price: "4.50", qty: 2 },
  { name: "Pen", price: "1.20", qty: 5 },
  { name: "Backpack", price: "32.00", qty: 1 },
];

const table = document.getElementById("items");
for (const item of items) {
  const row = table.insertRow();
  row.insertCell().textContent = item.name;
  row.insertCell().textContent = item.qty;
  row.insertCell().textContent = item.price;
}

const total = items.reduce((a, b) => a.price * a.qty + b.price * b.qty);
document.getElementById("total").textContent = Number(total).toFixed(2);
