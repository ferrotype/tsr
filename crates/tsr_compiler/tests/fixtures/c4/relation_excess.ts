interface A { kind: "a"; x: number }
interface B { kind: "b"; y: string }
declare function take(p: { a: number }): void;
export const one: { a: number } = { a: 1, b: 2 };
export const nested: { inner: { a: number } } = { inner: { a: 1, extra: true } };
export const union: A | B = { kind: "a", x: 1, y: "no" };
export const both: { a: number } & { b: string } = { a: 1, b: "", c: 0 };
export const list: { a: number }[] = [{ a: 1, q: 1 }];
export const wrong: { a: number } = { a: "no", d: 1 };
take({ a: 1, z: 3 });
