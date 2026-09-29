interface Box<out T> { value: T }
const b: Box<string> = { value: "x" };
function f<T>(x: T): T { return x; }
const n = f(1);
let u: string | number = n;

interface Pair<T> { first: T; second: T }
declare let ps: Pair<string>;
declare let pa: Pair<"a">;
ps = pa;
declare let bs: Box<string>;
declare let ba: Box<"a">;
bs = ba;

type Keys = keyof { a: 1; b: 2 };
type Picked = { a: 1; b: 2 }["a"];
type IsString<T> = T extends string ? "yes" : "no";
type R1 = IsString<"x">;
type Deferred<T> = T extends string ? "yes" : "no";
function g<T>(value: T): Deferred<T> { return undefined as any; }
type Mapped<T> = { readonly [K in keyof T]: T[K] };
type M1 = Mapped<{ a: 1 }>;
const tuple: [number, string] = [1, "a"];
const { first } = ps;
const [one, two] = tuple;
let evolving = [];
evolving.push(1);
const out = evolving;
