//! Group `core`: core helpers, collections, link store, stack.
//! Go: `tools/phase1/tables/go/core_columns.go`; spec:
//! `data/phase1/tables/core.json`.
use super::{decode, Column};
use serde::Deserialize;
use serde_json::{json, Value};

pub const COLUMNS: &[&str] = &[
    "core.Splice",
    "core.CheckEachDefined",
    "core.CopyMapInto",
    "core.DiffMaps",
    "core.DiffMapsFunc",
    "core.ElementOrNil",
    "core.FindLastIndex",
    "core.FirstNonNil",
    "core.FirstNonZero",
    "core.FirstOrNilSeq",
    "core.GetSpellingSuggestionWithMaxCandidateCount",
    "core.IndexAfter",
    "core.MapNonNil",
    "core.MinAllFunc",
    "core.Or",
    "core.ReplaceElement",
    "core.SameMapIndex",
    "core.ShouldRewriteModuleSpecifier",
    "core.UnorderedEqual",
    "core.LinkStore",
    "core.PagedLinkStore",
    "core.NonRelativeModuleNameForTypingCache",
    "core.Stack",
    "core.ApplyBulkEdits",
    "core.TypeAcquisition.Equals",
];

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "core.Splice" => splice(input),
        _ => return more(column, input),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpliceCase {
    start: i64,
    count: i64,
    items: Vec<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpliceInput {
    s: Vec<i64>,
    cases: Vec<SpliceCase>,
}

/// The result of every `(start, deleteCount, items)` case on one slice.
fn splice(input: &Value) -> Result<Column, String> {
    let input: SpliceInput = decode(input)?;
    Ok(Box::new(move || {
        Ok(Value::Array(
            input
                .cases
                .iter()
                .map(|case| {
                    json!(tsr_core::slices_ext::splice(
                        &input.s,
                        case.start,
                        case.count,
                        &case.items
                    ))
                })
                .collect(),
        ))
    }))
}

use crate::protocol::hex;
use std::borrow::Cow;
use std::collections::HashMap;
use tsr_core::slices_ext as slices;

/// Go's `typedValuesColumn`: the value is `f(input)`.
fn typed<I: serde::de::DeserializeOwned + 'static>(
    input: &Value,
    f: fn(&I) -> Result<Value, String>,
) -> Result<Column, String> {
    let input: I = decode(input)?;
    Ok(Box::new(move || f(&input)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lists {
    lists: Vec<Vec<i64>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases<C> {
    cases: Vec<C>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MapCase {
    #[serde(default)]
    m1: Option<HashMap<String, i64>>,
    #[serde(default)]
    m2: Option<HashMap<String, i64>>,
    #[serde(default)]
    dst: Option<HashMap<String, i64>>,
    #[serde(default)]
    nil: bool,
    #[serde(default)]
    added: bool,
    #[serde(default)]
    removed: bool,
    #[serde(default)]
    changed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SliceIndex {
    slice: Vec<i64>,
    index: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SliceThreshold {
    slice: Vec<i64>,
    threshold: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SliceOffset {
    slice: Vec<i64>,
    offset: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SeqCase {
    #[serde(default)]
    seq: Vec<i64>,
    #[serde(default)]
    nil: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpellingCase {
    name: String,
    candidates: Vec<String>,
    max: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexAfterCase {
    s: String,
    pattern: String,
    start: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrCase {
    predicates: Vec<String>,
    inputs: Vec<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceCase {
    slice: Vec<i64>,
    i: i64,
    t: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SameCase {
    slice: Vec<i64>,
    divisor: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RewriteCase {
    specifier: String,
    rewrite: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pairs {
    pairs: Vec<[Vec<i64>; 2]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreOp<K> {
    op: String,
    key: K,
    #[serde(default)]
    value: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ops<O> {
    ops: Vec<O>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Names {
    names: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StackOp {
    op: String,
    #[serde(default)]
    value: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    pos: i64,
    end: i64,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditCase {
    text: String,
    edits: Vec<Edit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Acquisition {
    enable: u8,
    #[serde(default)]
    include: Option<Vec<String>>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
    disable: u8,
    #[serde(default)]
    nil_include: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcquisitionPairs {
    pairs: Vec<[Acquisition; 2]>,
}

fn sorted_pairs(map: &HashMap<String, i64>) -> Value {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    Value::Array(
        keys.into_iter()
            .map(|key| json!([hex(key.as_bytes()), map[key]]))
            .collect(),
    )
}

/// Go's `diffEvents`.
fn diff_events(case: &MapCase, func: bool) -> Result<Value, String> {
    let empty = HashMap::new();
    let (m1, m2) = (
        case.m1.as_ref().unwrap_or(&empty),
        case.m2.as_ref().unwrap_or(&empty),
    );
    let events = std::cell::RefCell::new(Vec::new());
    let mut on_added = |key: &String, value: &i64| {
        events
            .borrow_mut()
            .push(json!(["added", hex(key.as_bytes()), value]));
    };
    let mut on_removed = |key: &String, value: &i64| {
        events
            .borrow_mut()
            .push(json!(["removed", hex(key.as_bytes()), value]));
    };
    let mut on_changed = |key: &String, v1: &i64, v2: &i64| {
        events
            .borrow_mut()
            .push(json!(["changed", hex(key.as_bytes()), v1, v2]));
    };
    let added: slices::OnEntry<'_, String, i64> = case.added.then_some(&mut on_added as _);
    let removed: slices::OnEntry<'_, String, i64> = case.removed.then_some(&mut on_removed as _);
    let changed: slices::OnChanged<'_, String, i64, i64> =
        case.changed.then_some(&mut on_changed as _);
    if func {
        slices::diff_maps_func(m1, m2, |v1, v2| v1 % 10 == v2 % 10, added, removed, changed);
    } else {
        slices::diff_maps(m1, m2, added, removed, changed);
    }
    let mut keyed = events
        .into_inner()
        .into_iter()
        .map(|event| {
            let mut key = Vec::new();
            crate::canonical::write(&mut key, &event, false).map_err(|_| "non-integer event")?;
            Ok((key, event))
        })
        .collect::<Result<Vec<_>, String>>()?;
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::Array(
        keyed.into_iter().map(|(_, event)| event).collect(),
    ))
}

fn predicate(name: &str) -> fn(&i64) -> bool {
    match name {
        "even" => |v| v % 2 == 0,
        "negative" => |v| *v < 0,
        _ => |v| *v > 100,
    }
}

fn store_result<V: Copy + Into<i64>>(value: Option<&V>) -> Value {
    value.map_or(Value::Null, |value| json!((*value).into()))
}

fn more(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "core.CheckEachDefined" => typed::<Lists>(input, |input| {
            Ok(Value::Array(
                input
                    .lists
                    .iter()
                    .map(|list| {
                        let options: Vec<Option<i64>> = list.iter().copied().map(Some).collect();
                        json!(slices::check_each_defined(&options, "undefined element")
                            .iter()
                            .flatten()
                            .collect::<Vec<_>>())
                    })
                    .collect(),
            ))
        }),
        "core.CopyMapInto" => typed::<Cases<MapCase>>(input, |input| {
            let empty = HashMap::new();
            Ok(Value::Array(
                input
                    .cases
                    .iter()
                    .map(|case| {
                        let dst = if case.nil {
                            None
                        } else {
                            Some(case.dst.clone().unwrap_or_default())
                        };
                        sorted_pairs(&slices::copy_map_into(
                            dst,
                            case.m2.as_ref().unwrap_or(&empty),
                        ))
                    })
                    .collect(),
            ))
        }),
        "core.DiffMaps" => typed::<Cases<MapCase>>(input, |input| {
            input
                .cases
                .iter()
                .map(|case| diff_events(case, false))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }),
        "core.DiffMapsFunc" => typed::<Cases<MapCase>>(input, |input| {
            input
                .cases
                .iter()
                .map(|case| diff_events(case, true))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }),
        "core.ElementOrNil" => typed::<Cases<SliceIndex>>(input, |input| {
            Ok(json!(input
                .cases
                .iter()
                .map(|c| slices::element_or_nil(&c.slice, c.index))
                .collect::<Vec<_>>()))
        }),
        "core.FindLastIndex" => typed::<Cases<SliceThreshold>>(input, |input| {
            Ok(json!(input
                .cases
                .iter()
                .map(|c| slices::find_last_index(&c.slice, |v| *v > c.threshold))
                .collect::<Vec<_>>()))
        }),
        "core.FirstNonNil" => typed::<Cases<SliceOffset>>(input, |input| {
            Ok(json!(input
                .cases
                .iter()
                .map(|c| slices::first_non_nil(&c.slice, |v| v - c.offset))
                .collect::<Vec<_>>()))
        }),
        "core.FirstNonZero" => typed::<Lists>(input, |input| {
            Ok(json!(input
                .lists
                .iter()
                .map(|list| slices::first_non_zero(list))
                .collect::<Vec<_>>()))
        }),
        "core.FirstOrNilSeq" => typed::<Cases<SeqCase>>(input, |input| {
            Ok(json!(input
                .cases
                .iter()
                .map(|c| slices::first_or_nil_seq((!c.nil).then(|| c.seq.iter().copied())))
                .collect::<Vec<_>>()))
        }),
        "core.GetSpellingSuggestionWithMaxCandidateCount" => {
            typed::<Cases<SpellingCase>>(input, |input| {
                Ok(json!(input
                    .cases
                    .iter()
                    .map(|c| {
                        let suggestion = tsr_scanner::get_spelling_suggestion(
                            c.name.as_bytes(),
                            c.candidates.iter().map(String::as_bytes),
                            |value| value,
                            Ord::cmp,
                            usize::try_from(c.max).unwrap_or(0),
                        );
                        hex(suggestion.unwrap_or(b""))
                    })
                    .collect::<Vec<_>>()))
            })
        }
        "core.IndexAfter" => typed::<Cases<IndexAfterCase>>(input, |input| {
            Ok(json!(input
                .cases
                .iter()
                .map(|c| slices::index_after(c.s.as_bytes(), c.pattern.as_bytes(), c.start))
                .collect::<Vec<_>>()))
        }),
        "core.MapNonNil" => typed::<Cases<SliceOffset>>(input, |input| {
            Ok(json!(input
                .cases
                .iter()
                .map(|c| slices::map_non_nil(&c.slice, |v| v - c.offset))
                .collect::<Vec<_>>()))
        }),
        "core.MinAllFunc" => typed::<Lists>(input, |input| {
            Ok(json!(input
                .lists
                .iter()
                .map(|list| slices::min_all_func(list, |a, b| a % 10 - b % 10))
                .collect::<Vec<_>>()))
        }),
        "core.Or" => typed::<Cases<OrCase>>(input, |input| {
            Ok(json!(input
                .cases
                .iter()
                .map(|c| {
                    let funcs: Vec<fn(&i64) -> bool> =
                        c.predicates.iter().map(|name| predicate(name)).collect();
                    let dyns: Vec<&dyn Fn(&i64) -> bool> =
                        funcs.iter().map(|f| f as &dyn Fn(&i64) -> bool).collect();
                    let or = slices::or(dyns);
                    c.inputs
                        .iter()
                        .map(|value| or.call(value))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()))
        }),
        "core.ReplaceElement" => typed::<Cases<ReplaceCase>>(input, |input| {
            Ok(json!(input
                .cases
                .iter()
                .map(|c| slices::replace_element(&c.slice, c.i, c.t))
                .collect::<Vec<_>>()))
        }),
        "core.SameMapIndex" => typed::<Cases<SameCase>>(input, |input| {
            Ok(json!(input
                .cases
                .iter()
                .map(|c| {
                    let result = slices::same_map_index(&c.slice, |v, i| {
                        if v % c.divisor == 0 {
                            v + i64::try_from(i).expect("index")
                        } else {
                            *v
                        }
                    });
                    let same = matches!(result, Cow::Borrowed(_)) && !c.slice.is_empty();
                    json!([result.into_owned(), same])
                })
                .collect::<Vec<_>>()))
        }),
        "core.ShouldRewriteModuleSpecifier" => typed::<Cases<RewriteCase>>(input, |input| {
            Ok(json!(input
                .cases
                .iter()
                .map(|c| {
                    let options = tsr_core::CompilerOptions {
                        rewrite_relative_import_extensions: tsr_core::Tristate(c.rewrite),
                        ..Default::default()
                    };
                    tsr_tspath::should_rewrite_module_specifier(c.specifier.as_bytes(), &options)
                })
                .collect::<Vec<_>>()))
        }),
        "core.UnorderedEqual" => typed::<Pairs>(input, |input| {
            Ok(json!(input
                .pairs
                .iter()
                .map(|[a, b]| slices::unordered_equal(a, b))
                .collect::<Vec<_>>()))
        }),
        "core.LinkStore" => typed::<Ops<StoreOp<String>>>(input, |input| {
            let mut store = tsr_core::linkstore::LinkStore::<String, i64>::default();
            let mut out = Vec::new();
            for op in &input.ops {
                out.push(match op.op.as_str() {
                    "get" => {
                        let value = store.get(&op.key);
                        let before = *value;
                        *value = op.value;
                        json!(before)
                    }
                    "has" => json!(store.has(&op.key)),
                    _ => store_result(store.try_get(&op.key)),
                });
            }
            Ok(Value::Array(out))
        }),
        "core.PagedLinkStore" => typed::<Ops<StoreOp<u64>>>(input, |input| {
            let mut store = tsr_core::linkstore::PagedLinkStore::<i64>::default();
            let mut out = Vec::new();
            for op in &input.ops {
                out.push(match op.op.as_str() {
                    "get" => {
                        let value = store.get(op.key);
                        let before = *value;
                        *value = op.value;
                        json!(before)
                    }
                    "has" => json!(store.has(op.key)),
                    _ => store_result(store.try_get(op.key)),
                });
            }
            Ok(Value::Array(out))
        }),
        "core.NonRelativeModuleNameForTypingCache" => typed::<Names>(input, |input| {
            Ok(json!(input
                .names
                .iter()
                .map(|name| hex(
                    tsr_core::node_modules::non_relative_module_name_for_typing_cache(
                        name.as_bytes()
                    )
                ))
                .collect::<Vec<_>>()))
        }),
        "core.Stack" => typed::<Ops<StackOp>>(input, |input| {
            let mut stack = tsr_core::stack::Stack::<i64>::default();
            let mut out = Vec::new();
            for op in &input.ops {
                out.push(match op.op.as_str() {
                    "push" => {
                        stack.push(op.value);
                        Value::Null
                    }
                    "pop" => json!(stack.pop()),
                    "peek" => json!(*stack.peek()),
                    _ => json!(stack.len()),
                });
            }
            Ok(Value::Array(out))
        }),
        "core.ApplyBulkEdits" => typed::<Cases<EditCase>>(input, |input| {
            input
                .cases
                .iter()
                .map(|c| {
                    let edits: Vec<tsr_core::TextChange> = c
                        .edits
                        .iter()
                        .map(|edit| tsr_core::TextChange {
                            range: tsr_core::TextRange::new(edit.pos, edit.end),
                            new_text: edit.text.as_bytes().to_vec(),
                        })
                        .collect();
                    let applied = edits
                        .iter()
                        .map(|edit| {
                            Ok(hex(&edit
                                .apply_to(c.text.as_bytes())
                                .map_err(|_| "unappliable edit")?))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    let bulk = tsr_core::apply_bulk_edits(c.text.as_bytes(), &edits)
                        .map_err(|_| "unappliable edits")?;
                    Ok(json!([hex(&bulk), applied]))
                })
                .collect::<Result<Vec<_>, String>>()
                .map(Value::Array)
        }),
        "core.TypeAcquisition.Equals" => typed::<AcquisitionPairs>(input, |input| {
            use tsr_tsoptions::TypeAcquisition;
            let build = |a: &Acquisition| TypeAcquisition {
                enable: tsr_core::Tristate(a.enable),
                include: if a.nil_include {
                    None
                } else {
                    a.include.as_ref().map(|list| {
                        list.iter()
                            .map(|s| tsr_jsstring::JsString::from_bytes(s.as_bytes()))
                            .collect()
                    })
                },
                exclude: a.exclude.as_ref().map(|list| {
                    list.iter()
                        .map(|s| tsr_jsstring::JsString::from_bytes(s.as_bytes()))
                        .collect()
                }),
                disable_filename_based_type_acquisition: tsr_core::Tristate(a.disable),
            };
            Ok(json!(input
                .pairs
                .iter()
                .map(|[a, b]| {
                    let (a, b) = (build(a), build(b));
                    [
                        TypeAcquisition::equals(Some(&a), Some(&b)),
                        TypeAcquisition::equals(Some(&a), Some(&a)),
                        TypeAcquisition::equals(Some(&a), None),
                        TypeAcquisition::equals(None, None),
                    ]
                })
                .collect::<Vec<_>>()))
        }),
        _ => return None,
    })
}
