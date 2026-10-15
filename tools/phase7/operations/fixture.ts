// Checked, never executed. Keep contextual requests and flow shapes deterministic.
export {};

declare function mapValue<T, R>(value: T, callback: (value: T) => R): R;
declare function compose<T, U, R>(
  value: T,
  first: (value: T) => U,
  second: (value: U) => R,
): R;
declare function withHandlers<T, R>(
  value: T,
  handlers: { select(value: T): R; merge(result: R, value: T): R },
): R;
declare function visit(value: number, callback: (value: number) => string): string;
declare function visit(value: string, callback: (value: string) => number): number;
declare function stop(message: string): never;

const nested = compose(
  { title: "sample", count: 3 },
  item => mapValue(item.count, count => ({ title: item.title, count: count + 1 })),
  item => item.title.repeat(item.count),
);
const handled = withHandlers(
  { name: "reader", values: [1, 2, 3] },
  {
    select(item) { return item.values.map(value => item.name + value); },
    merge(result, item) { return result.concat(item.name); },
  },
);
const overloaded = [visit(4, value => value.toFixed()), visit("four", value => value.length)];
const tuple = mapValue([nested, handled.length] as const, ([name, length]) => ({ name, length }));

interface Callbacks<T> {
  accept(value: T): boolean;
  describe(value: T): string;
}
declare function useCallbacks<T>(value: T, callbacks: Callbacks<T>): string;
const callbacks = useCallbacks(tuple, {
  accept: value => value.length > 0,
  describe: value => mapValue(value.name, name => name.toUpperCase()),
});

type Input =
  | { kind: "text"; value: string | undefined }
  | { kind: "count"; value: number; enabled?: boolean }
  | { kind: "stop"; reason: string };

function summarize(inputs: readonly Input[], fallback?: string): string {
  let result = fallback;
  let total = 0;
  for (const input of inputs) {
    switch (input.kind) {
      case "text":
        if (input.value !== undefined) {
          result = input.value.trim();
        } else if (!result) {
          result = "empty";
        }
        break;
      case "count":
        if (!input.enabled) continue;
        total += input.value;
        break;
      case "stop":
        stop(input.reason);
      default:
        return input;
    }
    if (total > 100) break;
  }
  if (!result) stop("missing result");
  return result.toUpperCase() + total;
}

function narrowLoop(value: string | number | undefined, attempts: number): string {
  let current = value;
  while (attempts-- > 0) {
    if (current === undefined) {
      current = attempts;
      continue;
    }
    if (typeof current === "number") {
      if (current < 0) stop("negative");
      current = current.toFixed();
    } else if (current.length > 2) {
      break;
    } else {
      current += "x";
    }
  }
  if (current === undefined) stop("missing value");
  return typeof current === "string" ? current : current.toString();
}

const checked = mapValue(
  [{ kind: "text", value: callbacks }, { kind: "count", value: 2, enabled: true }] satisfies Input[],
  inputs => summarize(inputs, narrowLoop(overloaded.length, 3)),
);
void checked;
