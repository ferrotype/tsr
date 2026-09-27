function* gen(): Generator<number, string, boolean> {
  const sent: boolean = yield 1;
  return sent ? "yes" : "no";
}
function overMap(m: Map<string, number>) {
  for (const [k, v] of m) { k.toUpperCase(); v.toFixed(); }
  const entries = [...m.entries()];
  const keys = [...m.keys()];
  return entries.length + keys.length;
}
function* delegate() {
  const result: string = yield* gen();
  return result;
}
async function asyncIter(items: AsyncIterable<number>) {
  for await (const item of items) { item.toFixed(); }
  for (const bad of items) { bad; }
}
function spreadSet(s: Set<string>) {
  const [first] = [...s];
  return first.length;
}
const wrongFor = () => { for (const x of 5) { x; } };
