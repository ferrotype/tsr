function evolving() {
  const items = [];
  items.push(1);
  items.push("two");
  return items;
}
type Shape = { kind: "circle"; r: number } | { kind: "square"; s: number };
function discriminant(shape: Shape | undefined) {
  if (shape?.kind === "circle") {
    return shape.r;
  }
  switch (shape?.kind) {
    case "square":
      return shape.s;
    default:
      return shape.kind;
  }
}
function typeofs(v: string | number | (() => void) | null) {
  if (typeof v === "function") { v(); }
  if (typeof v === "object") { v.toString(); }
  if (typeof v !== "string") { return v.toFixed(); }
  return v.toUpperCase();
}
function asserts(v: unknown): asserts v is string {}
function useAsserts(v: unknown) {
  asserts(v);
  return v.length;
}
function instanceofs(v: Date | RegExp) {
  if (v instanceof Date) { return v.getTime(); }
  return v.source;
}
function ins(v: { a: number } | { b: string }) {
  if ("a" in v) { return v.a; }
  return v.b.length;
}
