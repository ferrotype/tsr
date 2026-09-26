// Cross-product limit recovery and pattern-literal retention.
type Digit = 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9;
type Hundred = `${Digit}${Digit}`;
type Thousand = `${Digit}${Digit}${Digit}`;
type TooMany = `${Thousand}${Hundred}`;
declare const tooMany: TooMany;
const showTooMany: never = tooMany;
type Left = { [K in Thousand]: { left: K } }[Thousand];
type Right = { [K in Hundred]: { right: K } }[Hundred];
type TooManyIntersections = Left & Right;
declare const intersection: TooManyIntersections;
const showIntersection: never = intersection;
declare const left: Left;
declare const right: Right;
const spread = { ...left, ...right };
const showSpread: never = spread;
type Rows = { [K in Thousand]: [K] }[Thousand];
type Columns = { [K in Hundred]: [K] }[Hundred];
type Combine<A extends unknown[], B extends unknown[]> = [...A, ...B];
type TooManyTuples = Combine<Rows, Columns>;
declare const tuple: TooManyTuples;
const showTuple: never = tuple;
declare const pattern: "foo" | (`f${string}` & {});
const showPattern: never = pattern;
declare const reversed: "foo" | ({} & `f${string}`);
const showReversed: never = reversed;
function generic<T extends string>(value: "foo" | (`f${T}` & {})) {
    const showGeneric: never = value;
}
const after: number = "still checked";
