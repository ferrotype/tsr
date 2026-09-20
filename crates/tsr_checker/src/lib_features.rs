//! The lib suggestion table for properties that exist in a later `lib`
//! (`getFeatureMap` in `tsc/internal/checker/utilities.go`).

type Entry = (&'static str, &'static [&'static str]);

/// `getFeatureMap` in `tsc/internal/checker/utilities.go`, a `sync.OnceValue`.
const FEATURE_MAP: &[(&str, &[Entry])] = &[
    (
        "Array",
        &[
            (
                "es2015",
                &[
                    "find",
                    "findIndex",
                    "fill",
                    "copyWithin",
                    "entries",
                    "keys",
                    "values",
                ],
            ),
            ("es2016", &["includes"]),
            ("es2019", &["flat", "flatMap"]),
            ("es2022", &["at"]),
            (
                "es2023",
                &[
                    "findLastIndex",
                    "findLast",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "with",
                ],
            ),
        ],
    ),
    ("Iterator", &[("es2015", &[])]),
    ("AsyncIterator", &[("es2015", &[])]),
    (
        "ArrayBuffer",
        &[(
            "es2024",
            &[
                "maxByteLength",
                "resizable",
                "resize",
                "detached",
                "transfer",
                "transferToFixedLength",
            ],
        )],
    ),
    (
        "Atomics",
        &[
            (
                "es2017",
                &[
                    "add",
                    "and",
                    "compareExchange",
                    "exchange",
                    "isLockFree",
                    "load",
                    "or",
                    "store",
                    "sub",
                    "wait",
                    "notify",
                    "xor",
                ],
            ),
            ("es2024", &["waitAsync"]),
        ],
    ),
    (
        "SharedArrayBuffer",
        &[
            ("es2017", &["byteLength", "slice"]),
            ("es2024", &["growable", "maxByteLength", "grow"]),
        ],
    ),
    ("AsyncIterable", &[("es2018", &[])]),
    ("AsyncIterableIterator", &[("es2018", &[])]),
    ("AsyncGenerator", &[("es2018", &[])]),
    ("AsyncGeneratorFunction", &[("es2018", &[])]),
    (
        "RegExp",
        &[
            ("es2015", &["flags", "sticky", "unicode"]),
            ("es2018", &["dotAll"]),
            ("es2024", &["unicodeSets"]),
        ],
    ),
    ("RegExpConstructor", &[("es2025", &["escape"])]),
    (
        "Reflect",
        &[(
            "es2015",
            &[
                "apply",
                "construct",
                "defineProperty",
                "deleteProperty",
                "get",
                "getOwnPropertyDescriptor",
                "getPrototypeOf",
                "has",
                "isExtensible",
                "ownKeys",
                "preventExtensions",
                "set",
                "setPrototypeOf",
            ],
        )],
    ),
    (
        "ArrayConstructor",
        &[("es2015", &["from", "of"]), ("esnext", &["fromAsync"])],
    ),
    (
        "ObjectConstructor",
        &[
            (
                "es2015",
                &[
                    "assign",
                    "getOwnPropertySymbols",
                    "keys",
                    "is",
                    "setPrototypeOf",
                ],
            ),
            (
                "es2017",
                &["values", "entries", "getOwnPropertyDescriptors"],
            ),
            ("es2019", &["fromEntries"]),
            ("es2022", &["hasOwn"]),
            ("es2024", &["groupBy"]),
        ],
    ),
    (
        "NumberConstructor",
        &[(
            "es2015",
            &[
                "isFinite",
                "isInteger",
                "isNaN",
                "isSafeInteger",
                "parseFloat",
                "parseInt",
            ],
        )],
    ),
    (
        "Math",
        &[
            (
                "es2015",
                &[
                    "clz32", "imul", "sign", "log10", "log2", "log1p", "expm1", "cosh", "sinh",
                    "tanh", "acosh", "asinh", "atanh", "hypot", "trunc", "fround", "cbrt",
                ],
            ),
            ("es2025", &["f16round"]),
        ],
    ),
    (
        "Map",
        &[
            ("es2015", &["entries", "keys", "values"]),
            ("esnext", &["getOrInsert", "getOrInsertComputed"]),
        ],
    ),
    ("MapConstructor", &[("es2024", &["groupBy"])]),
    (
        "Set",
        &[
            ("es2015", &["entries", "keys", "values"]),
            (
                "es2025",
                &[
                    "union",
                    "intersection",
                    "difference",
                    "symmetricDifference",
                    "isSubsetOf",
                    "isSupersetOf",
                    "isDisjointFrom",
                ],
            ),
        ],
    ),
    (
        "PromiseConstructor",
        &[
            ("es2015", &["all", "race", "reject", "resolve"]),
            ("es2020", &["allSettled"]),
            ("es2021", &["any"]),
            ("es2024", &["withResolvers"]),
            ("es2025", &["try"]),
        ],
    ),
    (
        "Symbol",
        &[("es2015", &["for", "keyFor"]), ("es2019", &["description"])],
    ),
    (
        "WeakMap",
        &[
            ("es2015", &[]),
            ("esnext", &["getOrInsert", "getOrInsertComputed"]),
        ],
    ),
    ("WeakSet", &[("es2015", &[])]),
    (
        "String",
        &[
            (
                "es2015",
                &[
                    "codePointAt",
                    "includes",
                    "endsWith",
                    "normalize",
                    "repeat",
                    "startsWith",
                    "anchor",
                    "big",
                    "blink",
                    "bold",
                    "fixed",
                    "fontcolor",
                    "fontsize",
                    "italics",
                    "link",
                    "small",
                    "strike",
                    "sub",
                    "sup",
                ],
            ),
            ("es2017", &["padStart", "padEnd"]),
            ("es2019", &["trimStart", "trimEnd", "trimLeft", "trimRight"]),
            ("es2020", &["matchAll"]),
            ("es2021", &["replaceAll"]),
            ("es2022", &["at"]),
            ("es2024", &["isWellFormed", "toWellFormed"]),
        ],
    ),
    (
        "StringConstructor",
        &[("es2015", &["fromCodePoint", "raw"])],
    ),
    ("DateTimeFormat", &[("es2017", &["formatToParts"])]),
    ("Promise", &[("es2015", &[]), ("es2018", &["finally"])]),
    ("RegExpMatchArray", &[("es2018", &["groups"])]),
    ("RegExpExecArray", &[("es2018", &["groups"])]),
    (
        "Intl",
        &[
            ("es2018", &["PluralRules"]),
            ("es2020", &["RelativeTimeFormat", "Locale", "DisplayNames"]),
            ("es2021", &["ListFormat", "DateTimeFormat"]),
            ("es2022", &["Segmenter"]),
            ("es2025", &["DurationFormat"]),
        ],
    ),
    ("NumberFormat", &[("es2018", &["formatToParts"])]),
    (
        "SymbolConstructor",
        &[
            ("es2020", &["matchAll"]),
            ("esnext", &["metadata", "dispose", "asyncDispose"]),
        ],
    ),
    (
        "DataView",
        &[
            (
                "es2020",
                &["setBigInt64", "setBigUint64", "getBigInt64", "getBigUint64"],
            ),
            ("es2025", &["setFloat16", "getFloat16"]),
        ],
    ),
    ("BigInt", &[("es2020", &[])]),
    (
        "RelativeTimeFormat",
        &[("es2020", &["format", "formatToParts", "resolvedOptions"])],
    ),
    (
        "Int8Array",
        &[
            ("es2022", &["at"]),
            (
                "es2023",
                &[
                    "findLastIndex",
                    "findLast",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "with",
                ],
            ),
        ],
    ),
    (
        "Uint8Array",
        &[
            ("es2022", &["at"]),
            (
                "es2023",
                &[
                    "findLastIndex",
                    "findLast",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "with",
                ],
            ),
        ],
    ),
    (
        "Uint8ClampedArray",
        &[
            ("es2022", &["at"]),
            (
                "es2023",
                &[
                    "findLastIndex",
                    "findLast",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "with",
                ],
            ),
        ],
    ),
    (
        "Int16Array",
        &[
            ("es2022", &["at"]),
            (
                "es2023",
                &[
                    "findLastIndex",
                    "findLast",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "with",
                ],
            ),
        ],
    ),
    (
        "Uint16Array",
        &[
            ("es2022", &["at"]),
            (
                "es2023",
                &[
                    "findLastIndex",
                    "findLast",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "with",
                ],
            ),
        ],
    ),
    (
        "Int32Array",
        &[
            ("es2022", &["at"]),
            (
                "es2023",
                &[
                    "findLastIndex",
                    "findLast",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "with",
                ],
            ),
        ],
    ),
    (
        "Uint32Array",
        &[
            ("es2022", &["at"]),
            (
                "es2023",
                &[
                    "findLastIndex",
                    "findLast",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "with",
                ],
            ),
        ],
    ),
    ("Float16Array", &[("es2025", &[])]),
    (
        "Float32Array",
        &[
            ("es2022", &["at"]),
            (
                "es2023",
                &[
                    "findLastIndex",
                    "findLast",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "with",
                ],
            ),
        ],
    ),
    (
        "Float64Array",
        &[
            ("es2022", &["at"]),
            (
                "es2023",
                &[
                    "findLastIndex",
                    "findLast",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "with",
                ],
            ),
        ],
    ),
    (
        "BigInt64Array",
        &[
            ("es2020", &[]),
            ("es2022", &["at"]),
            (
                "es2023",
                &[
                    "findLastIndex",
                    "findLast",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "with",
                ],
            ),
        ],
    ),
    (
        "BigUint64Array",
        &[
            ("es2020", &[]),
            ("es2022", &["at"]),
            (
                "es2023",
                &[
                    "findLastIndex",
                    "findLast",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "with",
                ],
            ),
        ],
    ),
    ("Error", &[("es2022", &["cause"])]),
    ("ErrorConstructor", &[("esnext", &["isError"])]),
    (
        "Uint8ArrayConstructor",
        &[("esnext", &["fromBase64", "fromHex"])],
    ),
    ("DisposableStack", &[("esnext", &[])]),
    ("AsyncDisposableStack", &[("esnext", &[])]),
    ("Date", &[("esnext", &["toTemporalInstant"])]),
];

/// The first lib whose entry for `container` lists `property`.
// port: tsc/internal/checker/checker.go:Checker.getSuggestedLibForNonExistentProperty
pub(crate) fn suggested_lib_for_property(
    container: &[u8],
    property: &[u8],
) -> Option<&'static str> {
    let (_, entries) = FEATURE_MAP
        .iter()
        .find(|(name, _)| name.as_bytes() == container)?;
    entries
        .iter()
        .find(|(_, properties)| {
            properties
                .iter()
                .any(|candidate| candidate.as_bytes() == property)
        })
        .map(|(lib, _)| *lib)
}
