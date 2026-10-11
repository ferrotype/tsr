export const value: number = 1 + 2;
export const tuple: [number, string] = [value, "text"];
export function identity<T>(value: T): T { return value; }
