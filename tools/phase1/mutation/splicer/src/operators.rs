//! Type-directed mutation operators.
//!
//! Every operator replaces one value the site produces with another value of
//! the same type, chosen from the return type as written and the workspace's
//! own definitions of it: opposite booleans, `None`/`Ok(None)`, missing nodes
//! and lists inside the parser, same-typed parameters, skipped unit bodies,
//! `0`/`1` (`0`/`!0` for subtree facts, the slice length for a `usize` position
//! over a slice), empty collections, an empty text range at the real range's
//! start, the next or previous unit variant of an enum, a flipped low bit of an
//! integer newtype, one flipped component of a tuple, and parameter mutants. A site keeps at most two operators: its
//! strong result values, then parameters, then weak result values.
//!
//! Where a mutant replaces a result:
//! - a guard (`if hit(ID) { return VALUE; }` at body entry) skips the body;
//! - a result wrapper runs the body in a closure and returns VALUE instead of
//!   its result. A `Parser` method (taking `&mut self`) that returns a node, a
//!   node list or a token kind gets wrappers, and so does one returning a
//!   parameter: the caller has seen no tokens consumed for a missing list
//!   element, or for `parse_binary_expression_rest` returning `left` early, and
//!   parses the same tokens again forever. Values computed from the real result
//!   (`{R}`) or from a parameter's entry value (`{P}`, copied before the body
//!   runs) are wrappers everywhere. Booleans, integers and tristates stay
//!   guards: an early `false` from `parse_semicolon` (nothing consumed) is what
//!   kills it, a wrapped one (consumed, result ignored) survives, and a wrapped
//!   `true` hangs a predicate's caller just as an early one does.
//!
//! Parser-specific operators:
//! - a node result becomes a missing identifier (list: a missing list); an
//!   optional node result keeps its absence and replaces only a present node,
//!   so a failed `try_parse_*` still reports failure;
//! - a token scanner (it assigns `self.token`) sets the parser token to
//!   `Unknown` as well as returning it, so callers that ignore the return value
//!   still see the change; the end of file is kept, so every loop still ends;
//! - a node result may instead toggle its `THIS_NODE_HAS_ERROR` flag, and a list
//!   result may be marked missing: both keep the node's kind, where a missing
//!   identifier in place of a source file or a template span crashes the
//!   consumer. Neither allocates, so functions in the call closure of
//!   `create_missing_identifier`/`create_missing_list` (which never get the
//!   missing-node operator, it would recurse) can have them.
//!
//! A replacement that calls a node or list constructor allocates: the plan
//! pairs it with a control that computes the same replacement and discards it.
//! At most one operator per site allocates.

use crate::index::{
    first_arg, squash, top_level_args, ArmInfo, FnInfo, Param, Receiver, StmtInfo, StmtKind,
};
use crate::types::{type_head, TypeIndex};

pub const MAX_OPERATORS: usize = 2;

/// The body's real result inside a wrapper's replacement value.
pub const RESULT: &str = "{R}";
/// The captured entry value of the wrapper's parameter.
pub const PARAM: &str = "{P}";

/// Types with an inherent `empty()` constructor.
const EMPTY_CONSTRUCTORS: &[&str] = &["NodeSlice", "JSDocRoots"];

/// The node flag the flag operator toggles.
const ERROR_FLAG: &str = "THIS_NODE_HAS_ERROR";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    /// `if hit(ID) { return VALUE; }` at body entry; `None` returns unit.
    Guard(Option<String>),
    /// Runs the body in a closure, then `if hit(ID) { return VALUE; }`. In
    /// VALUE, `{R}` is the body's result and `{P}` the entry value of
    /// `capture`, a parameter copied before the body runs.
    Wrap {
        value: String,
        capture: Option<String>,
    },
    /// `let NAME = if hit(ID) { ALT } else { NAME };` at body entry (a plain
    /// reassignment for a `mut` parameter).
    Param {
        name: String,
        mutable: bool,
        alt: String,
    },
    /// `if hit(ID) { VALUE } else { ARM BODY }`.
    Arm(String),
    /// `if hit(ID) != (COND) { .. }`.
    Negate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operator {
    pub name: String,
    pub shape: Shape,
    /// The replacement calls a node or list constructor.
    pub allocates: bool,
}

/// Whether `text` calls a `create_*`/`new_*` constructor: the rule
/// `phase1_scope.allocates` applies to plan entries (`\b(?:create|new)_\w*\s*\(`).
pub fn allocating(text: &str) -> bool {
    let bytes = text.as_bytes();
    let word = |at: usize| bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_';
    ["create_", "new_"].iter().any(|prefix| {
        text.match_indices(prefix).any(|(at, _)| {
            if at > 0 && word(at - 1) {
                return false;
            }
            let mut end = at + prefix.len();
            while end < bytes.len() && word(end) {
                end += 1;
            }
            while end < bytes.len() && bytes[end].is_ascii_whitespace() {
                end += 1;
            }
            bytes.get(end) == Some(&b'(')
        })
    })
}

/// The operators chosen for one site, or why there are none.
#[derive(Debug, PartialEq, Eq)]
pub struct Choice {
    pub category: String,
    pub operators: Vec<Operator>,
    /// Operators deliberately withheld, with the reason.
    pub notes: Vec<String>,
}

/// What a site's surroundings allow.
#[derive(Clone, Copy, Debug, Default)]
pub struct Scope {
    /// A `Parser` method taking `&mut self` in `tsr_parser`: missing-node, flag
    /// and token operators are available, and node and parameter results are
    /// result wrappers.
    pub parser: bool,
    /// A `Binder` method: the unreachable-flow operator is available.
    pub binder: bool,
    /// In the missing-node call closure: missing-node operators are withheld.
    pub chain: bool,
    /// Subtree facts: integers mutate to `0` and `!0`.
    pub facts: bool,
}

impl Scope {
    pub fn of(file: &str, function: &FnInfo, chain: bool) -> Self {
        let head = type_head(&function.self_ty);
        Self {
            parser: head == "Parser"
                && function.receiver == Receiver::RefMut
                && file.starts_with("crates/tsr_parser/src/"),
            binder: head == "Binder" && function.receiver != Receiver::None,
            chain,
            facts: file.ends_with("/subtree_facts.rs"),
        }
    }
}

/// Everything operator choice reads about a site.
#[derive(Clone, Copy)]
pub struct Site<'a> {
    pub scope: Scope,
    pub function: &'a FnInfo,
    pub types: &'a TypeIndex,
    /// The directory name of the site's crate under `crates/`.
    pub krate: &'a str,
}

fn same_type(left: &str, right: &str) -> bool {
    left == right
        || !left.contains('<') && !right.contains('<') && type_head(left) == type_head(right)
}

/// One candidate replacement value.
#[derive(Clone, Debug)]
struct Candidate {
    name: String,
    value: String,
    /// A result wrapper (after the body) rather than a guard (at entry).
    after: bool,
    capture: Option<String>,
    /// Ordered after the parameter mutants.
    weak: bool,
    /// A constant the body's own tail constant can make equivalent.
    constant: bool,
}

impl Candidate {
    /// A constant value: a guard, or a wrapper when `after`.
    fn constant(value: &str, after: bool) -> Self {
        let name = if after {
            format!("wrap_result:{}", squash(value))
        } else {
            format!("return:{}", squash(value))
        };
        Self {
            name,
            value: value.to_owned(),
            after,
            capture: None,
            weak: false,
            constant: true,
        }
    }

    /// A value computed from the body's result: always a wrapper.
    fn derived(name: &str, value: String) -> Self {
        Self {
            name: name.to_owned(),
            value,
            after: true,
            capture: None,
            weak: false,
            constant: false,
        }
    }

    /// A value computed from a parameter (`param` + `suffix`): a guard reads the
    /// parameter; a wrapper reads its entry value, copied before the body runs.
    fn from_param(param: &str, suffix: &str, after: bool) -> Self {
        if after {
            Self {
                name: format!("wrap_param:{param}{}", squash(suffix)),
                value: format!("{PARAM}{suffix}"),
                after: true,
                capture: Some(param.to_owned()),
                weak: false,
                constant: false,
            }
        } else {
            Self {
                name: format!("return:{param}{}", squash(suffix)),
                value: format!("{param}{suffix}"),
                after: false,
                capture: None,
                weak: false,
                constant: false,
            }
        }
    }

    fn weak(mut self) -> Self {
        self.weak = true;
        self
    }

    /// The same candidate for `Result<T, E>`: `Ok(value)`, or the value mapped
    /// over the real result.
    fn in_ok(self) -> Self {
        if self.value.contains(RESULT) {
            let inner = self.value.replace(RESULT, "__phase1_ok");
            Self {
                name: self.name.replacen("wrap_", "wrap_ok_", 1),
                value: format!("{RESULT}.map(|__phase1_ok| {inner})"),
                ..self
            }
        } else {
            let value = format!("Ok({})", self.value);
            let name = if self.constant {
                Self::constant(&value, self.after).name
            } else {
                self.name.replacen(':', ":Ok(", 1) + ")"
            };
            Self {
                name,
                value,
                ..self
            }
        }
    }
}

struct Values {
    category: String,
    candidates: Vec<Candidate>,
    notes: Vec<String>,
}

impl Values {
    fn new(category: &str) -> Self {
        Self {
            category: category.to_owned(),
            candidates: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn constants(category: &str, values: &[&str], after: bool) -> Self {
        let mut result = Self::new(category);
        result.candidates = values
            .iter()
            .map(|value| Candidate::constant(value, after))
            .collect();
        result
    }
}

const CHAIN_NOTE: &str = "missing-node operator withheld: the function is in the call closure of \
create_missing_identifier/create_missing_list, so it would recurse";

/// The missing-node constructor for a node or list type inside the parser.
fn missing_node(ty: &str, site: Site<'_>, notes: &mut Vec<String>) -> Option<&'static str> {
    if ty.contains('<') || !site.scope.parser {
        return None;
    }
    let call = match type_head(ty) {
        "NodeId" => "self.create_missing_identifier()",
        "NodeListId" => "self.create_missing_list()",
        _ => return None,
    };
    if site.scope.chain {
        notes.push(CHAIN_NOTE.to_owned());
        return None;
    }
    Some(call)
}

/// A shared slice or string parameter: a `usize` position over it can move to
/// its end.
fn slice_param(params: &[Param]) -> Option<&Param> {
    params.iter().find(|param| {
        let Some(rest) = param.ty.strip_prefix('&') else {
            return false;
        };
        let rest = match rest.strip_prefix('\'') {
            Some(lifetime) => {
                lifetime.trim_start_matches(|c: char| c.is_alphanumeric() || c == '_')
            }
            None => rest,
        };
        !param.name.starts_with('_') && (rest.starts_with('[') || rest == "str")
    })
}

fn integer_values(ty: &str, site: Site<'_>, params: &[Param]) -> Values {
    let mut result = if site.scope.facts {
        Values::constants("integer", &["0", "!0"], false)
    } else {
        Values::constants("integer", &["0", "1"], false)
    };
    if site.types.resolve(ty) == "usize" {
        if let Some(param) = slice_param(params) {
            result.candidates.insert(
                0,
                Candidate::from_param(&param.name, ".len()", site.scope.parser),
            );
        }
    }
    result
}

/// `!x`, `None` or `x ^ 1` for one tuple component, with the operator's name.
fn component_op(ty: &str, at: &str, types: &TypeIndex) -> Option<(String, &'static str)> {
    if ty == "bool" {
        Some((format!("!{at}"), "!"))
    } else if first_arg(ty, "Option").is_some() {
        Some(("None".to_owned(), "None"))
    } else if types.is_integer(ty) {
        Some((format!("{at} ^ 1"), "^1"))
    } else {
        None
    }
}

fn tuple_values(ty: &str, site: Site<'_>) -> Values {
    let mut result = Values::new("tuple");
    let parts = top_level_args(&ty[1..ty.len() - 1]);
    for (index, part) in parts.iter().enumerate() {
        let at = format!("__phase1_tuple.{index}");
        if let Some((op, name)) = component_op(part, &at, site.types) {
            result.candidates.push(Candidate::derived(
                &format!("wrap_tuple:{index}:{name}"),
                format!("{{ let mut __phase1_tuple = {RESULT}; {at} = {op}; __phase1_tuple }}"),
            ));
        }
    }
    result
}

/// The next (or previous) unit variant, cyclically; other variants map to the
/// first unit variant.
fn variant_shift(ty: &str, variants: &[String], exhaustive: bool, forward: bool) -> String {
    let count = variants.len();
    let mut arms: Vec<String> = (0..count)
        .map(|index| {
            let to = if forward {
                (index + 1) % count
            } else {
                (index + count - 1) % count
            };
            format!("{ty}::{} => {ty}::{}", variants[index], variants[to])
        })
        .collect();
    if !exhaustive {
        arms.push(format!("_ => {ty}::{}", variants[0]));
    }
    format!("match {RESULT} {{ {} }}", arms.join(", "))
}

/// Values of a type named by path (neither a primitive nor a tuple nor a
/// reference): the workspace's enum, newtype and same-typed parameters.
fn named_values(ty: &str, head: &str, site: Site<'_>, params: &[Param]) -> Values {
    let mut result = Values::new(if head.ends_with("Id") { "id" } else { "other" });
    if let Some(definition) = site.types.enum_of(ty) {
        "enum".clone_into(&mut result.category);
        let (variants, exhaustive) = (&definition.unit_variants, definition.exhaustive_units);
        result.candidates.push(Candidate::derived(
            "wrap_variant:next",
            variant_shift(ty, variants, exhaustive, true),
        ));
        if variants.len() >= 3 {
            result.candidates.push(Candidate::derived(
                "wrap_variant:previous",
                variant_shift(ty, variants, exhaustive, false),
            ));
        }
    } else if site.types.integer_newtype(ty, site.krate) {
        "newtype".clone_into(&mut result.category);
        result.candidates.push(Candidate::derived(
            "wrap_newtype:^1",
            format!("{{ let mut __phase1_newtype = {RESULT}; __phase1_newtype.0 ^= 1; __phase1_newtype }}"),
        ));
    }
    parameter_values(ty, site, params, &mut result);
    result
}

/// The last same-typed parameter, then the first `Option` of the type unwrapped.
fn parameter_values(ty: &str, site: Site<'_>, params: &[Param], result: &mut Values) {
    let after = site.scope.parser;
    if let Some(param) = params.iter().rev().find(|param| same_type(&param.ty, ty)) {
        result
            .candidates
            .push(Candidate::from_param(&param.name, "", after));
    }
    if let Some(param) = params
        .iter()
        .find(|param| first_arg(&param.ty, "Option").is_some_and(|inner| same_type(inner, ty)))
    {
        result
            .candidates
            .push(Candidate::from_param(&param.name, ".unwrap()", after));
    }
}

fn node_values(ty: &str, head: &str, site: Site<'_>, params: &[Param]) -> Values {
    let mut result = Values::new(if head == "NodeId" {
        "node_id"
    } else {
        "node_list_id"
    });
    if let Some(call) = missing_node(ty, site, &mut result.notes) {
        result.candidates.push(Candidate::constant(call, true));
    }
    parameter_values(ty, site, params, &mut result);
    if site.scope.parser {
        if head == "NodeId" {
            result.candidates.push(
                Candidate::derived(
                    &format!("wrap_flags:{ERROR_FLAG}"),
                    format!(
                        "{{ let __phase1_flags = ::tsr_ast::Factory::node(&self.factory, {RESULT}).flags(); \
                         ::tsr_ast::Factory::set_node_flags(&mut self.factory, {RESULT}, \
                         __phase1_flags ^ ::tsr_ast::node_flags::{ERROR_FLAG}); {RESULT} }}"
                    ),
                )
                .weak(),
            );
        } else if !site
            .function
            .method_calls
            .iter()
            .any(|name| name == "mark_list_missing")
        {
            result.candidates.push(
                Candidate::derived(
                    "wrap_list_missing",
                    format!(
                        "{{ crate::ParserFactory::mark_list_missing(&mut self.factory, {RESULT}); {RESULT} }}"
                    ),
                )
                .weak(),
            );
        }
    }
    result
}

fn syntax_kind_values(ty: &str, site: Site<'_>) -> Values {
    let mut result = Values::new("syntax_kind");
    let unknown = format!("{ty}::Unknown");
    if site.scope.parser && site.function.assigns_self_token {
        result.candidates.push(Candidate::derived(
            &format!("wrap_token:{unknown}"),
            format!(
                "if {RESULT} == {ty}::EndOfFile {{ {RESULT} }} else {{ self.token = {unknown}; {unknown} }}"
            ),
        ));
        result
            .candidates
            .push(Candidate::constant(&unknown, true).weak());
    } else {
        result.candidates.push(Candidate::constant(&unknown, true));
    }
    result
}

/// Candidate values of type `ty`, strongest first. `params` supplies
/// parameter-derived values; arms pass none, because a pattern binding may
/// shadow a parameter there.
fn values(ty: &str, site: Site<'_>, params: &[Param]) -> Values {
    // Plain constants are guards; see the module comment.
    let after = false;
    if ty == "bool" {
        return Values::constants("bool", &["true", "false"], after);
    }
    if site.types.is_integer(ty) || type_head(ty) == "SubtreeFacts" && !ty.contains('<') {
        return integer_values(ty, site, params);
    }
    if let Some(inner) = first_arg(ty, "Option") {
        let node = site.scope.parser
            && !inner.contains('<')
            && matches!(type_head(inner), "NodeId" | "NodeListId");
        let mut result = Values::constants("option", &["None"], node);
        if let Some(call) = missing_node(inner, site, &mut result.notes) {
            result.candidates.push(Candidate::derived(
                &format!("wrap_present:{call}"),
                format!("{RESULT}.map(|_| {call})"),
            ));
        }
        if let Some(param) = params.iter().rev().find(|param| param.ty == ty) {
            result
                .candidates
                .push(Candidate::from_param(&param.name, "", site.scope.parser));
        }
        return result;
    }
    if let Some(inner) = first_arg(ty, "Result") {
        if inner == "()" {
            return Values::constants("result_unit", &["Ok(())"], after);
        }
        let inner = values(inner, site, params);
        return Values {
            category: format!("result_{}", inner.category),
            candidates: inner.candidates.into_iter().map(Candidate::in_ok).collect(),
            notes: inner.notes,
        };
    }
    if let Some(rest) = ty.strip_prefix('&') {
        let rest = match rest.strip_prefix('\'') {
            Some(lifetime) => {
                lifetime.trim_start_matches(|c: char| c.is_alphanumeric() || c == '_')
            }
            None => rest,
        };
        return if rest.starts_with('[') {
            Values::constants("slice", &["&[]"], after)
        } else if rest == "str" {
            Values::constants("str", &["\"\""], after)
        } else {
            Values::new("reference")
        };
    }
    if ty.starts_with('(') && ty.ends_with(')') {
        return tuple_values(ty, site);
    }
    let head = type_head(ty);
    match head {
        "NodeId" | "NodeListId" if !ty.contains('<') => node_values(ty, head, site, params),
        "JsString" | "String" if !ty.contains('<') => {
            Values::constants("string", &["Default::default()"], after)
        }
        "Vec" => Values::constants("vec", &["Vec::new()"], after),
        "Cow" if ty.contains('[') => {
            Values::constants("cow", &["::std::borrow::Cow::Borrowed(&[])"], after)
        }
        "Cow" if ty.ends_with(",str>") => {
            Values::constants("cow", &["::std::borrow::Cow::Borrowed(\"\")"], after)
        }
        _ if EMPTY_CONSTRUCTORS.contains(&head) && !ty.contains('<') => {
            let mut result = Values::new("empty");
            result
                .candidates
                .push(Candidate::constant(&format!("{ty}::empty()"), after));
            result
        }
        "SyntaxKind" if !ty.contains('<') => syntax_kind_values(ty, site),
        "Tristate" if !ty.contains('<') => {
            let mut result = Values::new("tristate");
            for value in ["TRUE", "FALSE"] {
                result
                    .candidates
                    .push(Candidate::constant(&format!("{ty}::{value}"), after));
            }
            result
        }
        "ControlFlow" if ty.ends_with("ControlFlow<()>") || ty.ends_with("ControlFlow<(),()>") => {
            Values::constants(
                "control_flow",
                &[
                    "::std::ops::ControlFlow::Continue(())",
                    "::std::ops::ControlFlow::Break(())",
                ],
                after,
            )
        }
        "BindingFlow" if site.scope.binder => Values::constants(
            "binding_flow",
            &["crate::need(self.unreachable_flow)"],
            after,
        ),
        "TextRange" if !ty.contains('<') => {
            let mut result = Values::new("text_range");
            result.candidates.push(Candidate::derived(
                "wrap_range:empty",
                format!(
                    "{{ let __phase1_range = {RESULT}; {ty}::new(__phase1_range.pos(), __phase1_range.pos()) }}"
                ),
            ));
            result
        }
        "Self" => Values::new("self"),
        _ if head.len() == 1 && head.chars().all(|c| c.is_ascii_uppercase()) => {
            Values::new("generic")
        }
        _ => named_values(ty, head, site, params),
    }
}

fn is_zero(constant: &str) -> bool {
    matches!(constant, "0" | "NONE" | "Ok(0)" | "Ok(NONE)")
}

/// Drops a constant equal to the constant the body already produces: that
/// mutant would be equivalent by construction.
fn keep_candidate(candidate: &Candidate, constant: Option<&str>) -> bool {
    let Some(constant) = constant else {
        return true;
    };
    if !candidate.constant {
        return true;
    }
    let value = squash(&candidate.value);
    value != constant && !(is_zero(&value) && is_zero(constant))
}

fn param_operators(params: &[Param], types: &TypeIndex) -> Vec<Operator> {
    params
        .iter()
        .filter(|param| !param.name.starts_with('_'))
        .filter_map(|param| {
            let alt = if param.ty == "bool" {
                format!("!{}", param.name)
            } else if first_arg(&param.ty, "Option").is_some() {
                "None".to_owned()
            } else if types.is_integer(&param.ty) {
                format!("{} ^ 1", param.name)
            } else {
                return None;
            };
            Some(Operator {
                name: format!("param:{}:{}", param.name, squash(&alt)),
                shape: Shape::Param {
                    name: param.name.clone(),
                    mutable: param.mutable,
                    alt,
                },
                allocates: false,
            })
        })
        .collect()
}

fn operator_of(candidate: Candidate) -> Operator {
    let allocates = allocating(&candidate.value);
    let shape = if candidate.after {
        Shape::Wrap {
            value: candidate.value,
            capture: candidate.capture,
        }
    } else {
        Shape::Guard(Some(candidate.value))
    };
    Operator {
        name: candidate.name,
        shape,
        allocates,
    }
}

/// Keeps the first allocating operator of a site and drops later ones, so a
/// site needs at most one control.
fn one_allocating(operators: &mut Vec<Operator>) {
    let mut seen = false;
    operators.retain(|operator| {
        let keep = !(operator.allocates && seen);
        seen |= operator.allocates;
        keep
    });
}

pub fn for_function(site: Site<'_>) -> Choice {
    let function = site.function;
    if function.is_const {
        return Choice {
            category: "const_fn".to_owned(),
            operators: Vec::new(),
            notes: vec!["a const fn cannot call the runtime switch".to_owned()],
        };
    }
    let mut notes = Vec::new();
    let (category, strong, weak) = match &function.ret {
        None => (
            "unit".to_owned(),
            vec![Operator {
                name: "skip_body".to_owned(),
                shape: Shape::Guard(None),
                allocates: false,
            }],
            Vec::new(),
        ),
        Some(ret) => {
            let found = values(ret, site, &function.params);
            notes.extend(found.notes);
            let (weak, strong): (Vec<Candidate>, Vec<Candidate>) = found
                .candidates
                .into_iter()
                .filter(|candidate| keep_candidate(candidate, function.tail_const.as_deref()))
                // A macro body's types are metavariables: only guards fit.
                .filter(|candidate| !(function.macro_body && candidate.after))
                .partition(|candidate| candidate.weak);
            (
                found.category,
                strong.into_iter().map(operator_of).collect(),
                weak.into_iter().map(operator_of).collect::<Vec<_>>(),
            )
        }
    };
    let mut operators = strong;
    if !function.macro_body {
        operators.extend(param_operators(&function.params, site.types));
    }
    operators.extend(weak);
    one_allocating(&mut operators);
    operators.truncate(MAX_OPERATORS);
    Choice {
        category,
        operators,
        notes,
    }
}

pub fn for_arm(arm: &ArmInfo, site: Site<'_>) -> Choice {
    let Some(ty) = &arm.value_ty else {
        return Choice {
            category: "unknown".to_owned(),
            operators: Vec::new(),
            notes: vec![
                "the arm's value type is not derivable: its match is not in the function's tail"
                    .to_owned(),
            ],
        };
    };
    let found = values(ty, site, &[]);
    let mut operators: Vec<Operator> = found
        .candidates
        .into_iter()
        // An arm replaces its value outright: only constants fit.
        .filter(|candidate| candidate.constant && candidate.capture.is_none())
        .filter(|candidate| keep_candidate(candidate, arm.body_const.as_deref()))
        .map(|candidate| Operator {
            name: format!("arm:{}", squash(&candidate.value)),
            allocates: allocating(&candidate.value),
            shape: Shape::Arm(candidate.value),
        })
        .collect();
    one_allocating(&mut operators);
    operators.truncate(MAX_OPERATORS);
    Choice {
        category: found.category,
        operators,
        notes: found.notes,
    }
}

pub fn for_stmt(stmt: &StmtInfo) -> Choice {
    match &stmt.kind {
        StmtKind::If { .. } | StmtKind::LetIf { .. } => Choice {
            category: "condition".to_owned(),
            operators: vec![Operator {
                name: "negate_condition".to_owned(),
                shape: Shape::Negate,
                allocates: false,
            }],
            notes: Vec::new(),
        },
        StmtKind::Other(kind) => Choice {
            category: (*kind).to_owned(),
            operators: Vec::new(),
            notes: vec![format!("a {kind} statement has no automatic operator")],
        },
    }
}

#[cfg(test)]
#[path = "operators_tests.rs"]
mod tests;
