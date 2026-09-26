declare let value: number;
declare let promised: Promise<number>;
declare let otherPromise: Promise<number>;
declare const promisedOne: Promise<1>;
declare const promisedTwo: Promise<2>;
declare const promisedText: Promise<string>;
declare const promisedObject: Promise<{ value: number }>;
declare let bigintValue: bigint;
declare let promisedBigint: Promise<bigint>;
declare const invalidThen: { then(callback: number): void };
declare const ordinaryThen: { then: number };

value + promised;
promised + value;
promised + otherPromise;
value += promised;
value < promised;
promised > value;
promisedOne <= promisedTwo;
value === promised;
promised === value;

promised < promisedText;
promised + promisedObject;
promised + true;
invalidThen + value;
ordinaryThen + value;

promisedBigint - bigintValue;
bigintValue - promisedBigint;
promisedBigint - promisedBigint;
bigintValue >>> bigintValue;
bigintValue - value;

// Valid after explicit awaits, plus a comparison that needs no await.
async function recovered() {
    value + await promised;
    await promised + value;
    await promised + await otherPromise;
    await promisedOne <= await promisedTwo;
    bigintValue - await promisedBigint;
}
promised < otherPromise;
