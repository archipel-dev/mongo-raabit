// Sample data for mongo-raabit tests and screenshots.
// Usage: mongosh mongodb://127.0.0.1:27017 plugins/mongo-raabit/scripts/seed-sample-data.js
const shop = db.getSiblingDB("raabit_shop");
shop.dropDatabase();
const cities = ["Berlin", "Lisbon", "Oslo", "Toronto", "Osaka", "Austin", "Lyon", "Utrecht"];
const tiers = ["free", "pro", "enterprise"];
const products = [
  { sku: "RB-100", name: "Carrot Keyboard", price: NumberDecimal("89.90"), tags: ["hardware", "input"], stock: 120 },
  { sku: "RB-200", name: "Burrow Monitor 27\"", price: NumberDecimal("329.00"), tags: ["hardware", "display"], stock: 34 },
  { sku: "RB-300", name: "Hop Mouse", price: NumberDecimal("39.50"), tags: ["hardware", "input"], stock: 410 },
  { sku: "RB-400", name: "Warren Dock", price: NumberDecimal("149.00"), tags: ["hardware", "usb-c"], stock: 0 },
  { sku: "RB-500", name: "Meadow Cloud Plan", price: NumberDecimal("12.00"), tags: ["subscription"], stock: null },
];
shop.products.insertMany(products);
const customers = [];
for (let i = 0; i < 240; i++) {
  customers.push({
    name: `Customer ${String(i).padStart(3, "0")}`,
    email: `customer${i}@example.com`,
    tier: tiers[i % 3],
    signedUp: new Date(Date.UTC(2023, i % 12, (i % 27) + 1)),
    address: { city: cities[i % cities.length], zip: String(10000 + i * 7), geo: { type: "Point", coordinates: [13.4 + (i % 10) / 10, 52.5 - (i % 7) / 10] } },
    newsletter: i % 4 !== 0,
    ...(i % 5 === 0 ? { phone: `+49 30 ${100000 + i}` } : {}),
  });
}
const ids = shop.customers.insertMany(customers).insertedIds;
const orders = [];
for (let i = 0; i < 1200; i++) {
  const items = [];
  for (let k = 0; k <= i % 3; k++) {
    const p = products[(i + k) % products.length];
    items.push({ sku: p.sku, qty: 1 + ((i + k) % 4), price: p.price });
  }
  orders.push({
    customerId: ids[i % customers.length],
    status: ["placed", "shipped", "delivered", "returned"][i % 4],
    createdAt: new Date(Date.UTC(2024, i % 12, (i % 28) + 1, i % 24)),
    items,
    total: items.reduce((s, it) => s + it.qty * parseFloat(it.price.toString()), 0),
  });
}
shop.orders.insertMany(orders);
shop.customers.createIndex({ email: 1 }, { unique: true });
shop.customers.createIndex({ "address.city": 1, tier: 1 });
shop.orders.createIndex({ customerId: 1, createdAt: -1 });
shop.orders.createIndex({ createdAt: 1 }, { expireAfterSeconds: 60 * 60 * 24 * 3650, name: "orders_ttl" });
shop.createView("bigOrders", "orders", [{ $match: { total: { $gt: 500 } } }, { $sort: { total: -1 } }]);
shop.createCollection("events", { timeseries: { timeField: "ts", metaField: "source" } });
shop.events.insertMany(Array.from({ length: 100 }, (_, i) => ({ ts: new Date(Date.UTC(2024, 5, 1, 0, i)), source: { host: `web-${i % 3}` }, latencyMs: 20 + (i * 37) % 180 })));
shop.createCollection("fs.files");
shop.createCollection("fs.chunks");
print(`seeded raabit_shop: ${shop.customers.countDocuments()} customers, ${shop.orders.countDocuments()} orders`);
