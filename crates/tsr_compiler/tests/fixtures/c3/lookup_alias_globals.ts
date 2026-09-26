namespace NS {
  export type Record<K extends string, V> = { [P in K]: V };
  export const NaN = 0;
}
import Record = NS.Record;
import NaN = NS.NaN;
declare const obj: { a: 1 } | { b: 2 };
function narrows(o: typeof obj) {
  if ("a" in o) {
    return o.a;
  }
  return o.b;
}
function compares(n: number) {
  return n === NaN;
}
