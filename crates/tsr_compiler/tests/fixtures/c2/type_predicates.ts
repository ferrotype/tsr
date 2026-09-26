function objectBinding({ item }: { item: unknown }): item is string { return true; }
function renamedBinding({ source: local }: { source: unknown }): local is string { return true; }
function propertyName({ source: local }: { source: unknown }): source is string { return true; }
function nestedBinding({ outer: [, { value: inner }] }: { outer: [unknown, { value: unknown }] }): inner is string { return true; }
function arrayBinding([, item]: unknown[]): item is string { return true; }
function restBinding([...items]: unknown[]): items is string[] { return true; }
function missingBinding([known]: unknown[]): missing is string { return true; }
function missingParameter(value: unknown): absent is string { return true; }
declare const fallbackValue: unknown;
function initializerName({ value = fallbackValue }: { value?: unknown }): fallbackValue is string { return true; }
function assertionBinding({ value }: { value: unknown }): asserts value is string {}
function ordinary(value: unknown): value is string { return typeof value === "string"; }
function ordinaryWithPattern({ other }: { other: unknown }, value: unknown): value is string { return true; }
function restParameter(...values: unknown[]): values is string[] { return true; }
const after: number = "still checked";
