declare function pair<T>(arg: [(n: number) => T, (x: T) => void]): T;
const numberResult = pair([_n => 0, x => x.toFixed()]);
const stringResult = pair([_n => "text", x => x.toUpperCase()]);
const showNumber: never = numberResult;
const showString: never = stringResult;
pair([_n => 0, x => x.toUpperCase()]);

declare function nested<T>(arg: { pair: [(n: number) => T, (x: T) => void] }): T;
const nestedResult = nested({ pair: [_n => true, x => x.valueOf()] });
const showNested: never = nestedResult;

function constrained<T extends [(n: number) => number]>(arg: T): T { return arg; }
const constrainedResult = constrained([x => x]);
const showConstrained: never = constrainedResult;
