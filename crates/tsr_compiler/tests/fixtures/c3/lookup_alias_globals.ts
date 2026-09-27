// Each changed raw lookup site meets a global alias that nothing has resolved
// yet: the uses come first in source order, the `import =` aliases that give
// the global names their meaning come last, and --noLib keeps the bundled
// globals out of the way (the required global types are declared at the end).
// The pin's getSymbol resolves such an alias to its target's flags.
function narrows(o: object) {
  // narrowTypeByInKeyword: an unknown property reaches getGlobalRecordSymbol.
  if ("c" in o) {
    return o.c;
  }
  return o;
}
function iterates(items: { size: number }) {
  // getPropertyNameForKnownSymbolName: the global Symbol value, looked up
  // before the object type is searched for its iterator method.
  for (const x of items) {
    return x;
  }
}
function keys<T>(o: T) {
  // getExtractStringType: the global Extract alias, arity-checked.
  for (const k in o) {
    return k;
  }
}
function truthy<T>(x: T) {
  // getGlobalNonNullableTypeInstantiation: the global NonNullable alias.
  if (x) {
    return x;
  }
  return null;
}
function compares(n: number) {
  // isGlobalNaN: the global NaN value (resolved by the identifier first).
  return n === NaN;
}
namespace NS {
  export type Record<K extends string | number | symbol, V> = { [P in K]: V };
  export type Extract<T, U> = T extends U ? T : never;
  export type NonNullable<T> = T & {};
  export const NaN = 0;
  export declare const Symbol: { readonly iterator: unique symbol };
}
import Record = NS.Record;
import Extract = NS.Extract;
import NonNullable = NS.NonNullable;
import NaN = NS.NaN;
import Symbol = NS.Symbol;
interface Array<T> { length: number; [n: number]: T; }
interface Boolean {}
interface CallableFunction {}
interface NewableFunction {}
interface Function {}
interface IArguments {}
interface Number {}
interface Object {}
interface RegExp {}
interface String {}
