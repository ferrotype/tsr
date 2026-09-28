interface Props { a: string; b: number; c: boolean }
interface Defaults { a: string }
declare function pick<P, D>(p: P, d: D): Pick<P, Extract<keyof P, keyof D>>;
declare function omit<P, D>(p: P, d: D): Pick<P, Exclude<keyof P, keyof D>>;
declare function keys<P>(p: P): Extract<keyof P, string>;
type StringKeys<P> = Extract<keyof P, string>;
declare const props: Props;
declare const defaults: Defaults;
declare const aliased: StringKeys<Props>;
export const picked: number = pick(props, props);
export const omitted: number = omit(props, defaults);
export const unaliased: number = keys(props);
export const alias: number = aliased;
