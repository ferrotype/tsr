//! The P1 storage-families trace: a frozen sequence of constructor actions run
//! on a checker prepared by the type-creating prefix of `NewChecker`, in Go and
//! in Rust, with every result observed and the live storage counted. This is the
//! production storage candidate's measurement, not the subset census (which needs
//! the complete semantic slice) and not an E5 result.

use crate::{
    element_flags, CheckerOptions, CheckerState, ElementFlags, Error, LiteralValue, SignatureId,
    TupleElementInfo, TypeAlias, TypeId, TypeKind, UnionReduction, BUILTIN_TYPE_NAMES,
};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use ts_arena::{CheckerIdentity, Counters, Generation, NodeId, SymbolId};
use ts_ast::{check_flags, symbol_flags, JsString, SymbolFlags, SyntaxKind};
use ts_jsnum::{Number, PseudoBigInt};

#[derive(Clone, Copy)]
enum Root {
    Type(TypeId),
    Symbol(SymbolId),
    Signature(SignatureId),
    Node(NodeId),
}

struct Member {
    name: JsString,
    r#type: usize,
    optional: bool,
    readonly: bool,
}

enum Action {
    Builtin(String),
    String(JsString),
    Number(Number),
    BigInt(PseudoBigInt),
    Fresh(usize),
    Regular(usize),
    Union(Vec<usize>, UnionReduction),
    UnionAlias(Vec<usize>, usize, Vec<usize>),
    Symbol(SymbolFlags, JsString, u32),
    TypeParameter(Option<usize>),
    TupleTarget(Vec<ElementFlags>, bool),
    Tuple(Vec<usize>),
    Reference(usize, Vec<usize>),
    Anonymous(Option<usize>, Vec<Member>),
    Template(Vec<JsString>, Vec<usize>),
    CallSignature(Vec<usize>, usize),
    SyntheticExpression(usize),
}

/// Decoded requests; decoding and text backing precede the measured interval.
pub struct Prepared {
    options: CheckerOptions,
    actions: Vec<Action>,
}

/// The checker, its results and the counters, all live at the checkpoint.
pub struct Live {
    _counters: Counters,
    _generation: Generation,
    _identity: Arc<CheckerIdentity>,
    state: CheckerState,
    roots: Vec<Root>,
    /// Creation counters after `NewChecker`'s prefix, before the trace.
    prefix_counts: (usize, u32, usize),
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("write to string");
    }
    output
}

fn bytes(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err("invalid byte hex".into());
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let digit = |byte: u8| char::from(byte).to_digit(16).ok_or("invalid hex digit");
            Ok((digit(chunk[0])? * 16 + digit(chunk[1])?) as u8)
        })
        .collect()
}

fn field<'a>(row: &'a Map<String, Value>, name: &str) -> Result<&'a Value, String> {
    row.get(name).ok_or_else(|| format!("missing field {name}"))
}

fn text_field(row: &Map<String, Value>, name: &str) -> Result<JsString, String> {
    Ok(JsString::from_bytes(bytes(
        field(row, name)?
            .as_str()
            .ok_or("hex field must be a string")?,
    )?))
}

fn u32_field(row: &Map<String, Value>, name: &str) -> Result<u32, String> {
    field(row, name)?
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("invalid u32 field {name}"))
}

fn root_index(value: &Value, limit: usize) -> Result<usize, String> {
    let index = value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or("invalid root")?;
    if index >= limit {
        return Err("root is not yet available".into());
    }
    Ok(index)
}

fn root_list(row: &Map<String, Value>, name: &str, limit: usize) -> Result<Vec<usize>, String> {
    field(row, name)?
        .as_array()
        .ok_or("root list must be an array")?
        .iter()
        .map(|value| root_index(value, limit))
        .collect()
}

fn expect_fields(row: &Map<String, Value>, expected: &[&str]) -> Result<(), String> {
    if row.len() != expected.len() || expected.iter().any(|key| !row.contains_key(*key)) {
        return Err(format!("missing or unexpected action field in {row:?}"));
    }
    Ok(())
}

impl Prepared {
    pub fn new(request: &Value) -> Result<Self, String> {
        let request = request.as_object().ok_or("request must be an object")?;
        expect_fields(request, &["version", "options", "actions"])?;
        if field(request, "version")?.as_u64() != Some(1) {
            return Err("unsupported trace version".into());
        }
        let options = field(request, "options")?
            .as_object()
            .ok_or("options must be an object")?;
        expect_fields(
            options,
            &["strict_null_checks", "exact_optional_property_types"],
        )?;
        let options = CheckerOptions {
            strict_null_checks: field(options, "strict_null_checks")?
                .as_bool()
                .ok_or("option must be boolean")?,
            exact_optional_property_types: field(options, "exact_optional_property_types")?
                .as_bool()
                .ok_or("option must be boolean")?,
        };
        let rows = field(request, "actions")?
            .as_array()
            .ok_or("actions must be an array")?;
        if rows.is_empty() {
            return Err("empty trace".into());
        }
        let mut actions = Vec::with_capacity(rows.len());
        for (index, row) in rows.iter().enumerate() {
            let row = row.as_object().ok_or("action must be an object")?;
            let op = field(row, "op")?.as_str().ok_or("op must be a string")?;
            let action = match op {
                "builtin" => {
                    expect_fields(row, &["op", "name"])?;
                    let name = field(row, "name")?
                        .as_str()
                        .ok_or("name must be a string")?;
                    if !BUILTIN_TYPE_NAMES.contains(&name) {
                        return Err(format!("unknown builtin {name}"));
                    }
                    Action::Builtin(name.to_string())
                }
                "string" => {
                    expect_fields(row, &["op", "text_hex"])?;
                    Action::String(text_field(row, "text_hex")?)
                }
                "number" => {
                    expect_fields(row, &["op", "bits_hex"])?;
                    let text = field(row, "bits_hex")?.as_str().ok_or("bits must be hex")?;
                    let bits = u64::from_str_radix(text, 16).map_err(|_| "invalid bits")?;
                    if text.len() != 16 {
                        return Err("bits_hex must be 16 digits".into());
                    }
                    Action::Number(Number::new(f64::from_bits(bits)))
                }
                "bigint" => {
                    expect_fields(row, &["op", "negative", "digits"])?;
                    let digits = field(row, "digits")?
                        .as_str()
                        .ok_or("digits must be text")?;
                    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                        return Err("digits must be decimal".into());
                    }
                    Action::BigInt(PseudoBigInt::new(
                        digits.as_bytes(),
                        field(row, "negative")?
                            .as_bool()
                            .ok_or("negative must be boolean")?,
                    ))
                }
                "fresh" => {
                    expect_fields(row, &["op", "root"])?;
                    Action::Fresh(root_index(field(row, "root")?, index)?)
                }
                "regular" => {
                    expect_fields(row, &["op", "root"])?;
                    Action::Regular(root_index(field(row, "root")?, index)?)
                }
                "union" => {
                    expect_fields(row, &["op", "types", "reduction"])?;
                    let reduction = match field(row, "reduction")?.as_str() {
                        Some("literal") => UnionReduction::Literal,
                        Some("none") => UnionReduction::None,
                        _ => return Err("unsupported union reduction".into()),
                    };
                    Action::Union(root_list(row, "types", index)?, reduction)
                }
                "union_alias" => {
                    expect_fields(row, &["op", "types", "alias_symbol", "alias_args"])?;
                    Action::UnionAlias(
                        root_list(row, "types", index)?,
                        root_index(field(row, "alias_symbol")?, index)?,
                        root_list(row, "alias_args", index)?,
                    )
                }
                "symbol" => {
                    expect_fields(row, &["op", "flags", "text_hex", "check_flags"])?;
                    Action::Symbol(
                        u32_field(row, "flags")?,
                        text_field(row, "text_hex")?,
                        u32_field(row, "check_flags")?,
                    )
                }
                "type_parameter" => {
                    expect_fields(row, &["op", "symbol"])?;
                    let symbol = field(row, "symbol")?;
                    Action::TypeParameter(if symbol.is_null() {
                        None
                    } else {
                        Some(root_index(symbol, index)?)
                    })
                }
                "tuple_target" => {
                    expect_fields(row, &["op", "elements", "readonly"])?;
                    let elements = field(row, "elements")?
                        .as_array()
                        .ok_or("elements must be an array")?
                        .iter()
                        .map(|value| match value.as_str() {
                            Some("required") => Ok(element_flags::REQUIRED),
                            Some("optional") => Ok(element_flags::OPTIONAL),
                            Some("rest") => Ok(element_flags::REST),
                            Some("variadic") => Ok(element_flags::VARIADIC),
                            _ => Err("unknown element kind".to_string()),
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Action::TupleTarget(
                        elements,
                        field(row, "readonly")?
                            .as_bool()
                            .ok_or("readonly must be boolean")?,
                    )
                }
                "tuple" => {
                    expect_fields(row, &["op", "types"])?;
                    Action::Tuple(root_list(row, "types", index)?)
                }
                "reference" => {
                    expect_fields(row, &["op", "target", "args"])?;
                    Action::Reference(
                        root_index(field(row, "target")?, index)?,
                        root_list(row, "args", index)?,
                    )
                }
                "anonymous" => {
                    expect_fields(row, &["op", "symbol", "members"])?;
                    let symbol = field(row, "symbol")?;
                    let symbol = if symbol.is_null() {
                        None
                    } else {
                        Some(root_index(symbol, index)?)
                    };
                    let members = field(row, "members")?
                        .as_array()
                        .ok_or("members must be an array")?
                        .iter()
                        .map(|member| {
                            let member = member.as_object().ok_or("member must be an object")?;
                            expect_fields(member, &["text_hex", "type", "optional", "readonly"])?;
                            Ok(Member {
                                name: text_field(member, "text_hex")?,
                                r#type: root_index(field(member, "type")?, index)?,
                                optional: field(member, "optional")?
                                    .as_bool()
                                    .ok_or("optional must be boolean")?,
                                readonly: field(member, "readonly")?
                                    .as_bool()
                                    .ok_or("readonly must be boolean")?,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    Action::Anonymous(symbol, members)
                }
                "template" => {
                    expect_fields(row, &["op", "texts", "types"])?;
                    let texts = field(row, "texts")?
                        .as_array()
                        .ok_or("texts must be an array")?
                        .iter()
                        .map(|text| {
                            Ok(JsString::from_bytes(bytes(
                                text.as_str().ok_or("text must be hex")?,
                            )?))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    let types = root_list(row, "types", index)?;
                    if texts.len() != types.len() + 1 {
                        return Err("template texts must be one longer than types".into());
                    }
                    Action::Template(texts, types)
                }
                "call_signature" => {
                    expect_fields(row, &["op", "parameters", "return"])?;
                    Action::CallSignature(
                        root_list(row, "parameters", index)?,
                        root_index(field(row, "return")?, index)?,
                    )
                }
                "synthetic_expression" => {
                    expect_fields(row, &["op", "type"])?;
                    Action::SyntheticExpression(root_index(field(row, "type")?, index)?)
                }
                _ => return Err(format!("unknown action {op}")),
            };
            actions.push(action);
        }
        Ok(Self { options, actions })
    }

    /// Runs `NewChecker`'s type prefix and then every action in one step.
    pub fn execute(&self) -> Result<Live, Error> {
        self.start()?.run()
    }

    /// Runs `NewChecker`'s type prefix only, so a caller can sample its
    /// allocator between the prefix and the trace.
    pub fn start(&self) -> Result<Started<'_>, Error> {
        let counters = Counters::new();
        let generation = Generation::new(&counters);
        let identity = CheckerIdentity::new(generation.clone(), &counters);
        let state = CheckerState::new(&identity, &counters, self.options)?;
        Ok(Started {
            prepared: self,
            counters,
            generation,
            identity,
            state,
        })
    }
}

/// A checker after `NewChecker`'s prefix, before the trace.
pub struct Started<'a> {
    prepared: &'a Prepared,
    counters: Counters,
    generation: Generation,
    identity: Arc<CheckerIdentity>,
    state: CheckerState,
}

impl Started<'_> {
    /// Runs every action; all allocations in here are the trace's interval.
    pub fn run(self) -> Result<Live, Error> {
        let Self {
            prepared,
            counters,
            generation,
            identity,
            mut state,
        } = self;
        let prefix_counts = (
            state.types.len(),
            state.symbol_count,
            state.signatures.len(),
        );
        let mut roots: Vec<Root> = Vec::with_capacity(prepared.actions.len());
        let type_root = |roots: &[Root], index: usize| -> Result<TypeId, Error> {
            match roots[index] {
                Root::Type(id) => Ok(id),
                _ => Err(Error::Unsupported("trace root is not a type")),
            }
        };
        let symbol_root = |roots: &[Root], index: usize| -> Result<SymbolId, Error> {
            match roots[index] {
                Root::Symbol(id) => Ok(id),
                _ => Err(Error::Unsupported("trace root is not a symbol")),
            }
        };
        let type_roots = |roots: &[Root], indexes: &[usize]| -> Result<Vec<TypeId>, Error> {
            indexes
                .iter()
                .map(|index| type_root(roots, *index))
                .collect()
        };
        for action in &prepared.actions {
            let root = match action {
                Action::Builtin(name) => Root::Type(
                    state
                        .builtins
                        .type_by_name(name)
                        .ok_or(Error::Unsupported("unknown builtin"))?,
                ),
                Action::String(value) => Root::Type(state.get_string_literal_type(value.clone())?),
                Action::Number(value) => Root::Type(state.get_number_literal_type(*value)?),
                Action::BigInt(value) => Root::Type(state.get_big_int_literal_type(value.clone())?),
                Action::Fresh(index) => {
                    Root::Type(state.get_fresh_type_of_literal_type(type_root(&roots, *index)?)?)
                }
                Action::Regular(index) => {
                    Root::Type(state.get_regular_type_of_literal_type(type_root(&roots, *index)?)?)
                }
                Action::Union(indexes, reduction) => {
                    let types = type_roots(&roots, indexes)?;
                    Root::Type(state.get_union_type_ex(&types, *reduction, None, None)?)
                }
                Action::UnionAlias(indexes, symbol, args) => {
                    let types = type_roots(&roots, indexes)?;
                    let alias = state.types.push_alias(TypeAlias {
                        symbol: symbol_root(&roots, *symbol)?,
                        type_arguments: Arc::from(type_roots(&roots, args)?),
                    })?;
                    Root::Type(state.get_union_type_ex(
                        &types,
                        UnionReduction::Literal,
                        Some(alias),
                        None,
                    )?)
                }
                Action::Symbol(flags, name, check_flags) => {
                    Root::Symbol(state.new_symbol_ex(*flags, name.clone(), *check_flags)?)
                }
                Action::TypeParameter(symbol) => {
                    let symbol = symbol.map(|index| symbol_root(&roots, index)).transpose()?;
                    Root::Type(state.new_type_parameter(symbol)?)
                }
                Action::TupleTarget(elements, readonly) => {
                    let infos: Vec<TupleElementInfo> = elements
                        .iter()
                        .map(|flags| TupleElementInfo {
                            flags: *flags,
                            labeled_declaration: None,
                        })
                        .collect();
                    Root::Type(state.get_tuple_target_type(&infos, *readonly)?)
                }
                Action::Tuple(indexes) => {
                    let types = type_roots(&roots, indexes)?;
                    Root::Type(state.create_tuple_type(&types)?)
                }
                Action::Reference(target, args) => {
                    let target = type_root(&roots, *target)?;
                    let args = type_roots(&roots, args)?;
                    Root::Type(state.create_type_reference(target, &args)?)
                }
                Action::Anonymous(symbol, members) => {
                    let symbol = symbol.map(|index| symbol_root(&roots, index)).transpose()?;
                    let mut table = ts_ast::SymbolTable::with_capacity(members.len());
                    for member in members {
                        let t = type_root(&roots, member.r#type)?;
                        let flags = symbol_flags::PROPERTY
                            | if member.optional {
                                symbol_flags::OPTIONAL
                            } else {
                                0
                            };
                        let property = state.new_symbol_ex(
                            flags,
                            member.name.clone(),
                            if member.readonly {
                                check_flags::READONLY
                            } else {
                                0
                            },
                        )?;
                        state
                            .value_symbol_links
                            .get_or_default(property)
                            .resolved_type = Some(t);
                        table.insert(member.name.clone(), Some(property));
                    }
                    let members = if table.is_empty() {
                        None
                    } else {
                        Some(state.alloc_symbol_table(table))
                    };
                    Root::Type(state.new_anonymous_type(symbol, members, &[], &[], &[])?)
                }
                Action::Template(texts, indexes) => {
                    let types = type_roots(&roots, indexes)?;
                    Root::Type(state.get_template_literal_type(texts, &types)?)
                }
                Action::CallSignature(parameters, return_type) => {
                    let parameters: Vec<SymbolId> = parameters
                        .iter()
                        .map(|index| symbol_root(&roots, *index))
                        .collect::<Result<_, _>>()?;
                    let parameters = if parameters.is_empty() {
                        None
                    } else {
                        Some(Arc::from(parameters))
                    };
                    let return_type = type_root(&roots, *return_type)?;
                    Root::Signature(state.new_call_signature(
                        None,
                        None,
                        parameters,
                        return_type,
                    )?)
                }
                Action::SyntheticExpression(index) => {
                    let t = type_root(&roots, *index)?;
                    Root::Node(state.new_synthetic_expression(t, false, None)?)
                }
            };
            roots.push(root);
        }
        Ok(Live {
            _counters: counters,
            _generation: generation,
            _identity: identity,
            state,
            roots,
            prefix_counts,
        })
    }
}

impl Live {
    /// Roots, named types, counts and the census. Called after the allocator is
    /// sampled so JSON construction stays outside the interval.
    pub fn observation(&self) -> Result<Value, Error> {
        let roots = self
            .roots
            .iter()
            .map(|root| self.observe_root(*root))
            .collect::<Result<Vec<_>, _>>()?;
        let mut named = Map::new();
        for name in BUILTIN_TYPE_NAMES {
            let id = self
                .state
                .builtins
                .type_by_name(name)
                .ok_or(Error::Unsupported("unknown builtin"))?;
            let record = self.state.types.get(id)?;
            named.insert(
                name.to_string(),
                json!({"id": id.get(), "flags": record.flags, "object_flags": record.object_flags}),
            );
        }
        let type_roots: Vec<TypeId> = self
            .roots
            .iter()
            .filter_map(|root| match root {
                Root::Type(id) => Some(*id),
                _ => None,
            })
            .collect();
        Ok(json!({
            "roots": roots,
            "named": named,
            "counts": {
                "types": self.state.types.len(),
                "symbols": self.state.symbol_count,
                "signatures": self.state.signatures.len(),
            },
            "prefix_counts": {
                "types": self.prefix_counts.0,
                "symbols": self.prefix_counts.1,
                "signatures": self.prefix_counts.2,
            },
            "census": self.state.census(&type_roots)?,
        }))
    }

    fn observe_root(&self, root: Root) -> Result<Value, Error> {
        let state = &self.state;
        Ok(match root {
            Root::Symbol(id) => {
                let symbol = state.symbol(id)?;
                json!({"kind": "symbol", "flags": symbol.flags(), "check_flags": symbol.check_flags(),
                    "name_hex": hex(symbol.name_bytes())})
            }
            Root::Signature(id) => {
                let signature = state.signatures.get(id)?;
                let declaration_is_function_type = match signature.declaration {
                    Some(node) => {
                        state.factory.view().node(node)?.kind().known()
                            == Some(SyntaxKind::FunctionType)
                    }
                    None => false,
                };
                json!({"kind": "signature", "id": id.get(),
                    "return": signature.resolved_return_type.map_or(0, TypeId::get),
                    "min_argument_count": signature.min_argument_count,
                    "parameter_count": signature.parameters.as_ref().map_or(0, |list| list.len()),
                    "declaration_is_function_type": declaration_is_function_type})
            }
            Root::Node(id) => {
                let kind = state.factory.view().node(id)?.kind().known();
                json!({"kind": "node",
                    "is_synthetic_expression": kind == Some(SyntaxKind::SyntheticExpression),
                    "type": state.synthetic_expression_types.get(&id).map_or(0, |t| t.get())})
            }
            Root::Type(id) => self.observe_type(id)?,
        })
    }

    fn observe_type(&self, id: TypeId) -> Result<Value, Error> {
        let state = &self.state;
        let record = *state.types.get(id)?;
        let mut row = Map::new();
        row.insert("kind".into(), json!("type"));
        row.insert("id".into(), json!(id.get()));
        row.insert("flags".into(), json!(record.flags));
        row.insert("object_flags".into(), json!(record.object_flags));
        row.insert(
            "symbol_name_hex".into(),
            match record.symbol {
                Some(symbol) => json!(hex(state.symbol(symbol)?.name_bytes())),
                None => Value::Null,
            },
        );
        if let Some(alias) = state.types.alias_of(id)? {
            row.insert(
                "alias_symbol_name_hex".into(),
                json!(hex(state.symbol(alias.symbol)?.name_bytes())),
            );
            row.insert("alias_args".into(), json!(ids(&alias.type_arguments)));
        } else {
            row.insert("alias_symbol_name_hex".into(), Value::Null);
            row.insert("alias_args".into(), json!([]));
        }
        let payload = match record.kind {
            TypeKind::Intrinsic => {
                row.insert(
                    "name_hex".into(),
                    json!(hex(state.types.intrinsic(id)?.name.as_bytes())),
                );
                "intrinsic"
            }
            TypeKind::Literal => {
                let data = state.types.literal(id)?;
                let value = match &data.value {
                    LiteralValue::String(text) => format!("string:{}", hex(text.as_bytes())),
                    LiteralValue::Number(number) => {
                        format!("number:{:016x}", number.value().to_bits())
                    }
                    LiteralValue::Boolean(value) => format!("boolean:{value}"),
                    LiteralValue::BigInt(value) => {
                        format!("bigint:{}", String::from_utf8_lossy(&value.to_text()))
                    }
                    LiteralValue::ComputedEnum => "enum:computed".to_string(),
                };
                row.insert("value".into(), json!(value));
                row.insert("fresh".into(), json!(data.fresh.map_or(0, TypeId::get)));
                row.insert("regular".into(), json!(data.regular.get()));
                "literal"
            }
            TypeKind::Union => {
                let data = state.types.union(id)?;
                row.insert("types".into(), json!(ids(&data.types)));
                row.insert("origin".into(), json!(data.origin.map_or(0, TypeId::get)));
                "union"
            }
            TypeKind::Anonymous | TypeKind::Reference | TypeKind::Interface | TypeKind::Tuple => {
                let object = state.types.object(id)?;
                row.insert("target".into(), json!(object.target.map_or(0, TypeId::get)));
                row.insert(
                    "properties".into(),
                    json!(self.properties(object.structured.properties.as_deref())?),
                );
                row.insert(
                    "type_arguments".into(),
                    match state.types.type_reference(id) {
                        Ok(reference) => json!(reference
                            .resolved_type_arguments
                            .as_deref()
                            .map_or_else(Vec::new, ids)),
                        Err(_) => json!([]),
                    },
                );
                if let Ok(interface) = state.types.interface(id) {
                    row.insert(
                        "this_type".into(),
                        json!(interface.this_type.map_or(0, TypeId::get)),
                    );
                    row.insert(
                        "type_parameters".into(),
                        json!(ids(interface.type_parameters())),
                    );
                    let mut members = Vec::new();
                    if let Some(table) = interface.declared_members {
                        for (name, symbol) in state.tables.get(table)? {
                            let resolved = symbol
                                .and_then(|s| state.value_symbol_links.try_get(s))
                                .and_then(|links| links.resolved_type)
                                .map_or(0, TypeId::get);
                            members.push((hex(name), resolved));
                        }
                    }
                    members.sort();
                    row.insert("declared_members".into(), json!(members));
                }
                if let Ok(tuple) = state.types.tuple(id) {
                    row.insert(
                        "element_flags".into(),
                        json!(tuple
                            .element_infos
                            .iter()
                            .map(|e| e.flags)
                            .collect::<Vec<_>>()),
                    );
                    row.insert("min_length".into(), json!(tuple.min_length));
                    row.insert("fixed_length".into(), json!(tuple.fixed_length));
                    row.insert("combined_flags".into(), json!(tuple.combined_flags));
                    row.insert("readonly".into(), json!(tuple.readonly));
                }
                match record.kind {
                    TypeKind::Anonymous => "anonymous",
                    TypeKind::Reference => "reference",
                    TypeKind::Interface => "interface",
                    _ => "tuple",
                }
            }
            TypeKind::TypeParameter => {
                let data = state.types.type_parameter(id)?;
                row.insert(
                    "constraint".into(),
                    json!(data.constraint.map_or(0, TypeId::get)),
                );
                row.insert("is_this_type".into(), json!(data.is_this_type));
                "type_parameter"
            }
            TypeKind::TemplateLiteral => {
                let data = state.types.template_literal(id)?;
                row.insert(
                    "texts".into(),
                    json!(data
                        .texts
                        .iter()
                        .map(|t| hex(t.as_bytes()))
                        .collect::<Vec<_>>()),
                );
                row.insert("types".into(), json!(ids(&data.types)));
                "template_literal"
            }
            kind => {
                return Err(Error::UnexpectedType {
                    context: "storage trace observation",
                    kind,
                })
            }
        };
        row.insert("payload".into(), json!(payload));
        Ok(Value::Object(row))
    }

    fn properties(&self, properties: Option<&[SymbolId]>) -> Result<Vec<(String, u32)>, Error> {
        let Some(properties) = properties else {
            return Ok(Vec::new());
        };
        properties
            .iter()
            .map(|symbol| {
                let name = hex(self.state.symbol(*symbol)?.name_bytes());
                let resolved = self
                    .state
                    .value_symbol_links
                    .try_get(*symbol)
                    .and_then(|links| links.resolved_type)
                    .map_or(0, TypeId::get);
                Ok((name, resolved))
            })
            .collect()
    }
}

fn ids(types: &[TypeId]) -> Vec<u32> {
    types.iter().map(|t| t.get()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(actions: &Value) -> Value {
        json!({"version": 1, "options": {"strict_null_checks": true, "exact_optional_property_types": false},
            "actions": actions})
    }

    #[test]
    fn malformed_or_forward_referencing_requests_cannot_produce_observations() {
        for actions in [
            json!([]),
            json!([{"op": "string", "text_hex": "f"}]),
            json!([{"op": "fresh", "root": 0}]),
            json!([{"op": "builtin", "name": "nope"}]),
            json!([{"op": "number", "bits_hex": "1"}]),
            json!([{"op": "template", "texts": [""], "types": []}, {"op": "template", "texts": [""], "types": [0]}]),
            json!([{"op": "union", "types": [], "reduction": "subtype"}]),
            json!([{"op": "string", "text_hex": "00", "unexpected": true}]),
        ] {
            assert!(
                Prepared::new(&request(&actions)).is_err(),
                "accepted {actions}"
            );
        }
        assert!(Prepared::new(&json!({"version": 2, "options": {}, "actions": []})).is_err());
    }

    #[test]
    fn a_small_trace_executes_and_observes_every_root() {
        let prepared = Prepared::new(&request(&json!([
            {"op": "builtin", "name": "stringType"},
            {"op": "builtin", "name": "numberType"},
            {"op": "union", "types": [1, 0], "reduction": "literal"},
            {"op": "string", "text_hex": "61"},
            {"op": "fresh", "root": 3},
            {"op": "symbol", "flags": 4, "text_hex": "78", "check_flags": 0},
            {"op": "anonymous", "symbol": null, "members": [{"text_hex": "62", "type": 3, "optional": false, "readonly": true}]},
            {"op": "tuple_target", "elements": ["required", "optional"], "readonly": false},
            {"op": "tuple", "types": [0, 1]},
            {"op": "call_signature", "parameters": [5], "return": 0},
            {"op": "synthetic_expression", "type": 2}
        ])))
        .unwrap();
        let live = prepared.execute().unwrap();
        let observation = live.observation().unwrap();
        let roots = observation["roots"].as_array().unwrap();
        assert_eq!(roots.len(), 11);
        assert_eq!(roots[2]["payload"], "union");
        assert_eq!(
            roots[2]["id"], observation["named"]["stringOrNumberType"]["id"],
            "string | number interns to the builtin union whatever the order"
        );
        assert_eq!(roots[6]["properties"][0][0], "62");
        assert_eq!(roots[7]["payload"], "tuple");
        assert_eq!(roots[7]["element_flags"], json!([1, 2]));
        assert_eq!(roots[8]["payload"], "reference");
        assert_eq!(roots[9]["declaration_is_function_type"], true);
        assert_eq!(roots[10]["is_synthetic_expression"], true);
        assert_eq!(roots[10]["type"], roots[2]["id"]);
        let census = &observation["census"];
        assert!(census["type_storage_bytes"].as_u64().unwrap() > 0);
        assert_eq!(
            census["unavailable"],
            json!(["checker_ast", "display_ast", "display_emit"])
        );
        assert!(
            census["types"]["reachable"].as_u64().unwrap()
                <= census["types"]["created"].as_u64().unwrap()
        );
    }
}
