//! S08 relater comparison child (`data/s08/relater-fixtures.json`).
//!
//! `p7_relater REQUESTS OUTPUT --implementation id|reference` runs one ordered
//! pass of every fixture and mode group in a fresh process. Each group has a
//! setup interval (checker creation, A/B declared-type lookup and, for the
//! reference, the graph translation and construction) and a relation interval
//! (the four frozen actions with their lazy resolution). The `s08-allocation`
//! feature adds the S07 allocator wrapper for requested and live bytes.
//!
//! The ID implementation is the production checker's relater through the
//! relation probe. The reference implementation is
//! `tools/s08/relater-prototype`, constructed from a description of the
//! production checker's resolved types: construction, not delegation.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;
use ts_arena::{CheckerIdentity, Counters, Generation};
use ts_checker::{CheckerOwner, LiteralShape, Operation, RelationKind, TypeKind, TypeRef};
use ts_compiler::{Program, ProgramCheckerHost};

use s08_relater_prototype as proto;

mod p3;
use p3::{array, load, text, Result};

#[cfg(feature = "s08-allocation")]
#[global_allocator]
static ALLOCATOR: cap::Cap<mimalloc::MiMalloc> = cap::Cap::new(mimalloc::MiMalloc, usize::MAX);
#[cfg(not(feature = "s08-allocation"))]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

const MODE: &str = if cfg!(feature = "s08-allocation") {
    "alloc"
} else {
    "normal"
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Implementation {
    Id,
    Reference,
}

/// Wall time and allocator endpoints of one interval.
#[derive(Default)]
struct Interval {
    ns: u64,
    requested_bytes: u64,
}

fn allocator_total() -> u64 {
    #[cfg(feature = "s08-allocation")]
    {
        ALLOCATOR.total_allocated() as u64
    }
    #[cfg(not(feature = "s08-allocation"))]
    {
        0
    }
}

fn allocator_live() -> u64 {
    #[cfg(feature = "s08-allocation")]
    {
        ALLOCATOR.allocated() as u64
    }
    #[cfg(not(feature = "s08-allocation"))]
    {
        0
    }
}

fn measure<T>(work: impl FnOnce() -> T) -> (T, Interval) {
    let total = allocator_total();
    let started = Instant::now();
    let value = work();
    let ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    (
        value,
        Interval {
            ns,
            requested_bytes: allocator_total() - total,
        },
    )
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut text, "{byte:02x}").expect("formatting into String is infallible");
    }
    text
}

fn mode_of(value: &Value) -> Result<(RelationKind, proto::Mode)> {
    Ok(match text(value)? {
        "identity" => (RelationKind::Identity, proto::Mode::Identity),
        "assignable" => (RelationKind::Assignable, proto::Mode::Assignable),
        "subtype" => (RelationKind::Subtype, proto::Mode::Subtype),
        "strict_subtype" => (RelationKind::StrictSubtype, proto::Mode::StrictSubtype),
        "comparable" => (RelationKind::Comparable, proto::Mode::Comparable),
        _ => return Err("unknown relation".into()),
    })
}

fn diagnostic_payload(program: &Program, d: &ts_ast::Diagnostic) -> Result<Value> {
    let file = if let Some(file) = d.file {
        let file = program
            .files()
            .iter()
            .find(|entry| entry.source() == file)
            .ok_or("diagnostic file missing")?;
        Some(String::from_utf8(
            file.bound().view().source_file()?.file_name().to_vec(),
        )?)
    } else {
        None
    };
    Ok(
        json!({"file":file,"pos":d.loc.pos(),"end":d.loc.end(),"code":d.code,"category":d.category,"key_hex":hex(d.message_key.as_bytes()),"text_hex":hex(d.message_text.as_bytes()),
        "args":if d.message_args.is_empty() { Value::Null } else { json!(d.message_args.iter().map(|v| String::from_utf8(v.as_bytes().to_vec())).collect::<std::result::Result<Vec<_>,_>>()?) },
        "chain":d.message_chain.iter().map(|d|diagnostic_payload(program,d)).collect::<Result<Vec<_>>>()?,"related":d.related_information.iter().map(|d|diagnostic_payload(program,d)).collect::<Result<Vec<_>>>()?}),
    )
}

/// A/B declaration nodes and declared types of one fixture on one checker.
struct Lookup {
    declarations: BTreeMap<Vec<u8>, ts_arena::NodeId>,
    types: BTreeMap<String, TypeRef>,
}

fn lookup(program: &Arc<Program>, op: &mut Operation<'_>, actions: &[Value]) -> Result<Lookup> {
    let file = program.file(b"/fixture.ts").ok_or("fixture missing")?;
    let ast = file.bound().view().ast();
    let statements = ast
        .node(file.source())?
        .statement_list()
        .ok_or("statements missing")?;
    let mut declarations = BTreeMap::new();
    for node in ast
        .node_slice(ast.list(statements)?.nodes())?
        .iter()
        .flatten()
    {
        if let Some(name) = ast.node(node)?.name() {
            declarations.insert(ast.node_text(name)?.as_bytes().to_vec(), node);
        }
    }
    let mut types = BTreeMap::new();
    for action in actions {
        for key in ["source", "target"] {
            let name = text(&action[key])?;
            if !types.contains_key(name) {
                let node = *declarations
                    .get(name.as_bytes())
                    .ok_or("declaration missing")?;
                let symbol = op
                    .get_symbol_at_location(
                        ast.node(node)?.name().ok_or("declaration name missing")?,
                    )?
                    .ok_or("symbol missing")?;
                types.insert(name.to_owned(), op.get_declared_type_of_symbol(symbol)?);
            }
        }
    }
    Ok(Lookup {
        declarations,
        types,
    })
}

// ---------------------------------------------------------------------------
// Translation of production types into a prototype description.

struct Translator<'a, 'op> {
    op: &'a mut Operation<'op>,
    indices: BTreeMap<u32, usize>,
    description: proto::Description,
}

impl Translator<'_, '_> {
    fn translate(&mut self, t: TypeRef) -> Result<usize> {
        if let Some(index) = self.indices.get(&t.id()) {
            return Ok(*index);
        }
        let index = self.description.types.len();
        self.indices.insert(t.id(), index);
        let flags = self.op.type_flags(t)?;
        let object_flags = self.op.type_object_flags(t)?;
        let symbol_id = self.op.type_symbol(t)?;
        // Names feed the reference's diagnostic text only; the production
        // printer would serialize whole nested structures here.
        let name = if flags & ts_checker::type_flags::INTRINSIC != 0 {
            String::from_utf8_lossy(self.op.intrinsic_type_name(t)?.as_bytes()).into_owned()
        } else if let Some(alias) = self.op.alias_symbol(t)? {
            String::from_utf8_lossy(self.op.symbol(self.op.symbol_ref(alias)?)?.name_bytes())
                .into_owned()
        } else if let Some(symbol) = symbol_id {
            String::from_utf8_lossy(self.op.symbol(self.op.symbol_ref(symbol)?)?.name_bytes())
                .into_owned()
        } else {
            format!("#{}", t.id())
        };
        let symbol = symbol_id.map(symbol_identity);
        let alias = self.op.alias_symbol(t)?.map(symbol_identity);
        // Reserve the slot before recursing so cycles resolve to this index.
        self.description.types.push(proto::TypeDesc {
            name: name.clone(),
            flags,
            object_flags,
            symbol,
            alias,
            kind: proto::KindDesc::Unsupported("translating".into()),
        });
        let kind = match self.op.type_kind(t)? {
            TypeKind::Intrinsic => proto::KindDesc::Intrinsic,
            TypeKind::Literal => {
                let (value, fresh) = self.op.literal_shape(t)?;
                let value = match value {
                    LiteralShape::String(bytes) => proto::LiteralValue::String(bytes),
                    LiteralShape::Number(value) => proto::LiteralValue::Number(value.to_bits()),
                    LiteralShape::Boolean(value) => proto::LiteralValue::Boolean(value),
                    LiteralShape::BigInt { negative, digits } => {
                        proto::LiteralValue::BigInt { negative, digits }
                    }
                    LiteralShape::Unknown => {
                        return Ok(self.unsupported(index, "computed enum literal"))
                    }
                };
                let alternate = if fresh {
                    self.op.regular_type_of_literal_type(t)?
                } else {
                    self.op.fresh_type_of_literal_type(t)?
                };
                let alternate = if alternate == t {
                    None
                } else {
                    Some(self.translate(alternate)?)
                };
                proto::KindDesc::Literal {
                    value,
                    fresh,
                    alternate,
                }
            }
            TypeKind::Anonymous | TypeKind::Interface => self.object(t, symbol_id)?,
            TypeKind::Union => {
                let members = self.op.constituents(t)?;
                let mut indices = Vec::with_capacity(members.len());
                for member in members {
                    indices.push(self.translate(member)?);
                }
                proto::KindDesc::Union(indices)
            }
            TypeKind::Intersection => {
                let members = self.op.constituents(t)?;
                let mut indices = Vec::with_capacity(members.len());
                for member in members {
                    indices.push(self.translate(member)?);
                }
                proto::KindDesc::Intersection(indices)
            }
            other => return Ok(self.unsupported(index, &format!("{other:?} type"))),
        };
        self.description.types[index].kind = kind;
        Ok(index)
    }

    fn unsupported(&mut self, index: usize, reason: &str) -> usize {
        self.description.types[index].kind = proto::KindDesc::Unsupported(reason.to_string());
        index
    }

    fn object(
        &mut self,
        t: TypeRef,
        symbol: Option<ts_arena::SymbolId>,
    ) -> Result<proto::KindDesc> {
        use ts_ast::symbol_flags as sf;
        let mut members = Vec::new();
        for property in self.op.properties_of_type(t)? {
            let (name, flags) = {
                let read = self.op.symbol(property)?;
                (
                    String::from_utf8_lossy(read.name_bytes()).into_owned(),
                    read.flags(),
                )
            };
            let ty = self.op.get_type_of_symbol(property)?;
            let readonly = self.op.is_readonly_symbol(property)?;
            members.push(proto::MemberDesc {
                name,
                optional: flags & sf::OPTIONAL != 0,
                readonly,
                class_member: flags & sf::CLASS_MEMBER != 0,
                r#type: self.translate(ty)?,
            });
        }
        let mut index_infos = Vec::new();
        for (key, value, readonly) in self.op.index_infos(t)? {
            index_infos.push(proto::IndexDesc {
                key: self.translate(key)?,
                value: self.translate(value)?,
                readonly,
            });
        }
        let mut call_signatures = Vec::new();
        let mut construct_signatures = Vec::new();
        for (construct, list) in [
            (false, &mut call_signatures),
            (true, &mut construct_signatures),
        ] {
            for signature in self.op.signatures_of_type(t, construct)? {
                list.push(self.signature(signature)?);
            }
        }
        let inferable_index = match symbol {
            Some(id) => {
                let flags = self.op.symbol(self.op.symbol_ref(id)?)?.flags();
                flags & (sf::OBJECT_LITERAL | sf::TYPE_LITERAL | sf::ENUM | sf::VALUE_MODULE) != 0
                    && flags & sf::CLASS == 0
            }
            None => false,
        };
        Ok(proto::KindDesc::Object {
            members,
            index_infos,
            call_signatures,
            construct_signatures,
            inferable_index,
        })
    }

    fn signature(&mut self, signature: ts_checker::SignatureRef) -> Result<proto::SignatureDesc> {
        use ts_ast::SyntaxKind as K;
        let shape = self.op.signature_shape(signature)?;
        let mut parameters = Vec::with_capacity(shape.parameters.len());
        for parameter in shape.parameters {
            parameters.push(self.translate(parameter)?);
        }
        let this_type = match shape.this_type {
            Some(t) => Some(self.translate(t)?),
            None => None,
        };
        let bivariant = matches!(
            shape.declaration_kind,
            Some(kind) if kind == K::MethodDeclaration as i16 || kind == K::MethodSignature as i16 || kind == K::Constructor as i16
        );
        Ok(proto::SignatureDesc {
            parameters,
            min_argument_count: shape.min_argument_count,
            has_rest_parameter: shape.has_rest_parameter,
            type_parameters: shape.type_parameters,
            this_type,
            return_type: self.translate(shape.return_type)?,
            bivariant_parameters: bivariant,
            is_abstract: shape.is_abstract,
        })
    }
}

/// A stable per-checker identity for a symbol id (recursion identities).
fn symbol_identity(id: ts_arena::SymbolId) -> u64 {
    id.bits()
}

/// The intrinsic cells the prototype algorithm names, added to every description.
fn add_intrinsics(translator: &mut Translator<'_, '_>) -> Result<()> {
    for name in ["stringType", "numberType", "bigintType"] {
        let t = translator.op.builtin_type(name).ok_or("builtin type")?;
        translator.translate(t)?;
    }
    Ok(())
}

fn cache_state(checker: &proto::Checker, graph_len: usize) -> Value {
    let mut caches = serde_json::Map::new();
    for (mode, name) in proto::MODES.iter().zip([
        "identity",
        "assignable",
        "subtype",
        "strict_subtype",
        "comparable",
    ]) {
        let relation = checker.relation(*mode);
        caches.insert(
            name.to_string(),
            json!({"entries": relation.entries(), "result_flags": relation.result_flags()}),
        );
    }
    json!({"caches": caches, "types_created": graph_len, "signatures_created": 0, "instantiations": 0,
        "lazy_records": checker.graph.lazy_records()})
}

/// One fixture/mode group on a fresh checker.
fn group(
    program: &Arc<Program>,
    actions: &[Value],
    generation: &Generation,
    counters: &Counters,
    implementation: Implementation,
) -> Result<Value> {
    let live_before = allocator_live();
    let (relation_kind, proto_mode) = mode_of(&actions[0]["mode"])?;
    let (setup, setup_interval) = measure(|| -> Result<_> {
        let owner = Arc::new(CheckerOwner::for_program(
            CheckerIdentity::new(generation.clone(), counters),
            counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )?);
        Ok(owner)
    });
    let owner = setup?;
    let mut op = owner.operation()?;
    let before_lookup = op.relation_state();
    let (looked_up, lookup_interval) = measure(|| lookup(program, &mut op, actions));
    let looked_up = looked_up?;
    let after_lookup = op.relation_state();
    let checker_ns = setup_interval.ns;
    let lookup_ns = lookup_interval.ns;
    let mut translate_ns = 0u64;
    let mut setup_ns = setup_interval.ns + lookup_interval.ns;
    let mut setup_bytes = setup_interval.requested_bytes + lookup_interval.requested_bytes;
    let mut observations = Vec::new();
    let mut unsupported: Option<String> = None;
    let relation_interval;
    let live_after;
    match implementation {
        Implementation::Id => {
            let (result, interval) = measure(|| -> Result<Vec<Value>> {
                let mut rows = Vec::new();
                for action in actions {
                    let before = op.relation_state();
                    let node = if action["report_errors"] == true {
                        Some(
                            *looked_up
                                .declarations
                                .get(text(&action["source"])?.as_bytes())
                                .ok_or("declaration missing")?,
                        )
                    } else {
                        None
                    };
                    let (result, calls, diagnostic) = op.observe_type_relation(
                        looked_up.types[text(&action["source"])?],
                        looked_up.types[text(&action["target"])?],
                        relation_kind,
                        node,
                    )?;
                    let diagnostics = diagnostic
                        .iter()
                        .map(|d| diagnostic_payload(program, d))
                        .collect::<Result<Vec<_>>>()?;
                    rows.push(json!({"action":action,"result":result,"ternary_calls":calls,"before":before,"after":op.relation_state(),"diagnostics":diagnostics}));
                }
                Ok(rows)
            });
            observations = result?;
            relation_interval = interval;
            live_after = allocator_live();
        }
        Implementation::Reference => {
            // Setup continues: translate the reachable production types into a
            // description and construct the prototype graph (lazy members).
            let (constructed, translate_interval) = measure(|| -> Result<_> {
                let mut translator = Translator {
                    op: &mut op,
                    indices: BTreeMap::new(),
                    description: proto::Description::default(),
                };
                add_intrinsics(&mut translator)?;
                let mut roots = BTreeMap::new();
                for (name, t) in &looked_up.types {
                    roots.insert(name.clone(), translator.translate(*t)?);
                }
                let description = translator.description;
                let checker = proto::Checker::new();
                let constructed = checker.graph.construct(&description)?;
                checker.register_intrinsics(&constructed.cells);
                Ok((checker, constructed, roots, description.types.len()))
            });
            let (checker, constructed, roots, described) = translate_interval_result(constructed)?;
            setup_ns += translate_interval.ns;
            translate_ns = translate_interval.ns;
            setup_bytes += translate_interval.requested_bytes;
            let (result, interval) = measure(|| -> Result<Vec<Value>> {
                let mut rows = Vec::new();
                for action in actions {
                    let before = cache_state(&checker, checker.graph.len());
                    let source = &constructed.cells[roots[text(&action["source"])?]];
                    let target = &constructed.cells[roots[text(&action["target"])?]];
                    let diagnostics_before = checker.diagnostics().len();
                    let outcome = checker.check_type_related_to(
                        source,
                        target,
                        proto_mode,
                        action["report_errors"] == true,
                    );
                    let calls = checker.take_observed();
                    match outcome {
                        Ok((ternary, related)) => {
                            let diagnostics: Vec<String> =
                                checker.diagnostics()[diagnostics_before..].to_vec();
                            rows.push(json!({"action":action,"result":related,"top_ternary":ternary,"ternary_calls":calls,
                                "before":before,"after":cache_state(&checker, checker.graph.len()),
                                "diagnostics":diagnostics.iter().map(|d| json!({"text":d})).collect::<Vec<_>>()}));
                        }
                        Err(error) => {
                            return Err(format!("{error:?}").into());
                        }
                    }
                }
                Ok(rows)
            });
            relation_interval = interval;
            match result {
                Ok(rows) => observations = rows,
                Err(error) => unsupported = Some(error.to_string()),
            }
            let _ = described;
            live_after = allocator_live();
            drop(constructed);
            drop(checker);
        }
    }
    let live_after_release = {
        drop(op);
        drop(owner);
        allocator_live()
    };
    Ok(json!({
        "mode": actions[0]["mode"],
        "state": if unsupported.is_some() { "unsupported" } else { "executed" },
        "reason": unsupported,
        "before_lookup": before_lookup,
        "after_lookup": after_lookup,
        "actions": observations,
        "setup_ns": setup_ns, "relation_ns": relation_interval.ns,
        "setup_parts_ns": {"checker": checker_ns, "lookup": lookup_ns, "translate": translate_ns},
        "allocation": {"setup_requested_bytes": setup_bytes, "relation_requested_bytes": relation_interval.requested_bytes,
            "live_before": live_before, "live_after": live_after, "live_after_release": live_after_release},
    }))
}

type Constructed = (
    proto::Checker,
    proto::Constructed,
    BTreeMap<String, usize>,
    usize,
);
fn translate_interval_result(value: Result<Constructed>) -> Result<Constructed> {
    value
}

fn observe(request_bytes: &[u8], implementation: Implementation) -> Result<Value> {
    let request_sha256 = format!("{:x}", Sha256::digest(request_bytes));
    let requests: Value = serde_json::from_slice(request_bytes)?;
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let mut rows = Vec::new();
    let mut totals = json!({"setup_ns": 0u64, "relation_ns": 0u64, "setup_requested_bytes": 0u64, "relation_requested_bytes": 0u64,
        "retained_bytes": 0u64, "groups_executed": 0u64, "groups_unsupported": 0u64});
    let started = Instant::now();
    for request in array(&requests)? {
        let (program, load_interval) = measure(|| load(request, &counters));
        let program = program?;
        let mut groups = Vec::new();
        let actions = array(&request["actions"])?;
        let mut start = 0;
        while start < actions.len() {
            let mut end = start + 1;
            while end < actions.len() && actions[end]["mode"] == actions[start]["mode"] {
                end += 1;
            }
            let value = group(
                &program,
                &actions[start..end],
                &generation,
                &counters,
                implementation,
            )?;
            let add = |totals: &mut Value, key: &str, delta: u64| {
                totals[key] = json!(totals[key].as_u64().unwrap_or(0) + delta);
            };
            add(
                &mut totals,
                "setup_ns",
                value["setup_ns"].as_u64().unwrap_or(0),
            );
            add(
                &mut totals,
                "relation_ns",
                value["relation_ns"].as_u64().unwrap_or(0),
            );
            add(
                &mut totals,
                "setup_requested_bytes",
                value["allocation"]["setup_requested_bytes"]
                    .as_u64()
                    .unwrap_or(0),
            );
            add(
                &mut totals,
                "relation_requested_bytes",
                value["allocation"]["relation_requested_bytes"]
                    .as_u64()
                    .unwrap_or(0),
            );
            let live_before = i128::from(value["allocation"]["live_before"].as_u64().unwrap_or(0));
            let live_after = i128::from(value["allocation"]["live_after"].as_u64().unwrap_or(0));
            let retained = i128::from(totals["retained_bytes"].as_i64().unwrap_or(0));
            totals["retained_bytes"] = json!(i64::try_from(retained + live_after - live_before)?);
            if value["state"] == "executed" {
                add(&mut totals, "groups_executed", 1);
            } else {
                add(&mut totals, "groups_unsupported", 1);
            }
            groups.push(value);
            start = end;
        }
        rows.push(json!({"id":request["id"],"groups":groups,"state":"executed","load_ns":load_interval.ns}));
    }
    totals["process_ns"] = json!(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
    Ok(
        json!({"version":1,"implementation":match implementation { Implementation::Id => "id", Implementation::Reference => "reference" },
        "mode":MODE,"request_sha256":request_sha256,"stack_bytes":256usize << 20,"rows":rows,"totals":totals}),
    )
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 || args[3] != "--implementation" {
        return Err(
            "usage: p7_relater requests.json output.json --implementation id|reference".into(),
        );
    }
    let implementation = match args[4].as_str() {
        "id" => Implementation::Id,
        "reference" => Implementation::Reference,
        _ => return Err("implementation must be id or reference".into()),
    };
    let request_bytes = std::fs::read(&args[1])?;
    // The frozen deep fixtures nest 4,096 levels; both implementations recurse
    // on one generously sized worker stack.
    let observed = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || observe(&request_bytes, implementation).map_err(|e| e.to_string()))?
        .join()
        .map_err(|_| "relater worker panicked")??;
    std::fs::write(&args[2], serde_json::to_vec(&observed)?)?;
    println!("{}", serde_json::to_string(&observed["totals"])?);
    Ok(())
}
