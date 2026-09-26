interface A { readonly a: 1 }
interface B { readonly b: 2 }

declare const matched: "getX" & `get${string}`;
const showMatched: never = matched;
declare const reversed: `get${string}` & "getX";
const showReversed: never = reversed;
declare const incompatible: "setX" & `get${string}`;
const checkIncompatible: never = incompatible;

declare const lower: "foo" & Lowercase<string>;
const showLower: never = lower;
declare const invalidLower: "FOO" & Lowercase<string>;
const checkInvalidLower: never = invalidLower;

declare const ordered: "foo" & A & Lowercase<string> & B & `f${string}`;
const showOrdered: never = ordered;
declare const reordered: B & `f${string}` & "foo" & Lowercase<string> & A;
const showReordered: never = reordered;

function generic<T extends string>(template: "getX" & `get${T}`, mapping: "foo" & Lowercase<T>) {
    const showTemplate: never = template;
    const showMapping: never = mapping;
}

declare const anyMismatch: any & "FOO" & Lowercase<string>;
const checkAnyMismatch: never = anyMismatch;
declare const anyMatch: any & "foo" & Lowercase<string>;
const showAnyMatch: never = anyMatch;
