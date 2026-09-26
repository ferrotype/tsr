declare const x1: <T>() => (T extends infer U extends number ? 1 : 0);
export function f1() { return x1; }
type ExpectNumber<T extends number> = T;
declare const x2: <T>() => (T extends ExpectNumber<infer U> ? 1 : 0);
export function f2() { return x2; }
export const identity = <T>(value: T) => value;
export const nested = <T>(value: T) => <U>(other: U) => [value, other] as const;
export const sibling = <T>(value: T) => value;
export const privateClass = class { private secret = 0; };
