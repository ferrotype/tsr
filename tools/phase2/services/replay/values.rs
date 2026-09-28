//! Recorder values against Rust handles: the per-checker token maps, the node
//! lookup, argument resolution and the structural comparison of results.
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::ControlFlow;
use std::rc::Rc;
use std::sync::Arc;
use tsr_arena::{ArenaId, NodeId, SymbolId};
use tsr_ast::{AstView, ChildVisitor, JsDocProvider, NodeListId, NodeSlice};
use tsr_checker::{
    IndexInfoRef, LiteralView, Operation, SignatureRef, SymbolRef, TypePredicateRef, TypeRef,
    TypeViewData,
};
use tsr_compiler::Program;

/// Object flags that record how a type was built (the recorder's
/// `phase2StableObjectFlags`); the rest cache answers of earlier queries.
const STABLE_OBJECT_FLAGS: u32 = {
    use tsr_checker::object_flags as of;
    of::CLASS
        | of::INTERFACE
        | of::REFERENCE
        | of::TUPLE
        | of::ANONYMOUS
        | of::MAPPED
        | of::INSTANTIATED
        | of::OBJECT_LITERAL
        | of::EVOLVING_ARRAY
        | of::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES
        | of::REVERSE_MAPPED
        | of::JSX_ATTRIBUTES
        | of::JS_LITERAL
        | of::FRESH_LITERAL
        | of::ARRAY_LITERAL
        | of::PRIMITIVE_UNION
        | of::CONTAINS_WIDENING_TYPE
        | of::CONTAINS_OBJECT_OR_ARRAY_LITERAL
        | of::NON_INFERRABLE_TYPE
        | of::CONTAINS_SPREAD
        | of::OBJECT_REST_TYPE
        | of::INSTANTIATION_EXPRESSION_TYPE
        | of::SINGLE_SIGNATURE_TYPE
};

/// The outcome of one replayed call.
#[derive(Debug)]
pub enum Outcome {
    Match,
    /// Rust answered differently.
    Mismatch(String),
    /// An argument refers to a value the replay never mapped (it came from a
    /// call that did not match, or from outside the checker).
    Unmapped(String),
    /// The replay cannot make this call yet.
    Unsupported(String),
    /// Rust returned an error or panicked.
    Failed(String),
    /// The checker cannot run: its program did not load or it panicked earlier.
    Skipped(String),
    /// The program is one this replay does not rebuild by design (a later
    /// phase's program kind); the reason names it.
    Excluded(String),
}

impl Outcome {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Mismatch(_) => "mismatch",
            Self::Unmapped(_) => "unmapped",
            Self::Unsupported(_) => "unsupported",
            Self::Failed(_) => "failed",
            Self::Skipped(_) => "skipped",
            Self::Excluded(_) => "excluded",
        }
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Match => None,
            Self::Mismatch(reason)
            | Self::Unmapped(reason)
            | Self::Unsupported(reason)
            | Self::Failed(reason)
            | Self::Skipped(reason)
            | Self::Excluded(reason) => Some(reason),
        }
    }
}

impl From<tsr_checker::Error> for Outcome {
    fn from(error: tsr_checker::Error) -> Self {
        Self::Failed(format!("{error:?}"))
    }
}

/// The Go recorder writes a name without the process-wide symbol id that
/// private names, unique symbols and pattern ambient modules carry
/// (`phase2Name`), and `encoding/json` writes the 0xFE marker as U+FFFD.
pub fn normalize_name(name: &[u8]) -> String {
    let mut name = name;
    let mut owned;
    if name.starts_with(b"\xfe#") {
        let digits = name[2..].iter().take_while(|b| b.is_ascii_digit()).count();
        if digits > 0 && name.get(2 + digits) == Some(&b'@') {
            owned = b"\xfe#".to_vec();
            owned.extend_from_slice(&name[2 + digits..]);
            return String::from_utf8_lossy(&owned).into_owned();
        }
    }
    if name.first() == Some(&0xfe) {
        if let Some(at) = name.iter().rposition(|&b| b == b'@') {
            if at > 1 && at + 1 < name.len() && name[at + 1..].iter().all(u8::is_ascii_digit) {
                owned = name[..=at].to_vec();
                name = &owned;
                return String::from_utf8_lossy(name).into_owned();
            }
        }
    }
    String::from_utf8_lossy(name).into_owned()
}

/// A result as Rust produced it, before it is compared with the record.
pub enum Actual {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Number(String),
    Type(TypeRef),
    Symbol(SymbolRef),
    Signature(SignatureRef),
    Node(NodeId),
    Index(IndexInfoRef),
    Predicate(TypePredicateRef),
    /// Builder output, already described as the recorder's synthetic tree.
    Tree(Value),
    List(Vec<Actual>),
    /// A symbol list the recorder orders by name and declaration.
    Unordered(Vec<SymbolRef>),
    /// A recorded object token (a node builder, an emit resolver).
    Object,
    /// An already described value compared by equality.
    Json(Value),
    Fields(Vec<(&'static str, Actual)>),
}

pub fn opt<T>(value: Option<T>, f: impl FnOnce(T) -> Actual) -> Actual {
    value.map_or(Actual::Null, f)
}

pub fn types(list: Vec<TypeRef>) -> Actual {
    Actual::List(list.into_iter().map(Actual::Type).collect())
}

pub fn symbols(list: Vec<SymbolRef>) -> Actual {
    Actual::List(list.into_iter().map(Actual::Symbol).collect())
}

pub fn signatures(list: Vec<SignatureRef>) -> Actual {
    Actual::List(list.into_iter().map(Actual::Signature).collect())
}

pub fn string(text: &[u8]) -> Actual {
    Actual::Str(normalize_name(text))
}

/// A file's nodes by their `(pos, end)`.
type NodeIndex = Rc<HashMap<(i32, i32), Vec<NodeId>>>;

/// One source file of a program: where its nodes are, found by position.
struct FileEntry {
    index: usize,
    name: String,
    nodes: RefCell<Option<NodeIndex>>,
    jsdoc: RefCell<Option<NodeIndex>>,
}

/// A loaded program and the index the replay finds its nodes by.
pub struct ProgramState {
    pub program: Arc<Program>,
    files: Vec<FileEntry>,
    by_name: HashMap<String, usize>,
    by_arena: HashMap<ArenaId, usize>,
    missing: Vec<String>,
    extra: Vec<String>,
    ordered: bool,
}

struct Children<'a> {
    view: AstView<'a>,
    nodes: Vec<NodeId>,
}
impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, id: NodeId) -> ControlFlow<()> {
        self.nodes.push(id);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, id: NodeListId) -> ControlFlow<()> {
        match self.view.list(id) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(_) => ControlFlow::Break(()),
        }
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        match self.view.node_slice(nodes) {
            Ok(nodes) => {
                self.nodes.extend(nodes.iter().flatten());
                ControlFlow::Continue(())
            }
            Err(_) => ControlFlow::Break(()),
        }
    }
}

/// The children of `id` in `ForEachChild` order.
pub fn children(view: AstView<'_>, id: NodeId) -> Vec<NodeId> {
    let mut visitor = Children {
        view,
        nodes: Vec::new(),
    };
    if let Ok(node) = view.node(id) {
        let _ = node.for_each_child(&mut visitor);
    }
    visitor.nodes
}

impl ProgramState {
    pub fn new(
        program: Arc<Program>,
        expected: &[String],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut files = Vec::new();
        let mut by_name = HashMap::new();
        let mut by_arena = HashMap::new();
        for (index, file) in program.files().iter().enumerate() {
            let view = file.bound().view();
            let name =
                String::from_utf8_lossy(view.source_file()?.parse_options().file_name.as_bytes())
                    .into_owned();
            by_name.insert(name.clone(), files.len());
            by_arena.insert(file.source().arena(), files.len());
            files.push(FileEntry {
                index,
                name,
                nodes: RefCell::new(None),
                jsdoc: RefCell::new(None),
            });
        }
        let actual: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        let missing = expected
            .iter()
            .filter(|name| !by_name.contains_key(name.as_str()))
            .cloned()
            .collect();
        let extra = actual
            .iter()
            .filter(|name| !expected.iter().any(|e| e == *name))
            .map(|name| (*name).to_string())
            .collect();
        let ordered = actual == expected.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(Self {
            program,
            files,
            by_name,
            by_arena,
            missing,
            extra,
            ordered,
        })
    }

    pub fn status(&self) -> Value {
        json!({
            "files": self.files.len(),
            "same_files": self.missing.is_empty() && self.extra.is_empty(),
            "same_order": self.ordered,
            "missing": self.missing,
            "extra": self.extra,
        })
    }

    fn view(&self, file: usize) -> AstView<'_> {
        self.program.files()[self.files[file].index]
            .bound()
            .view()
            .ast()
    }

    pub fn source(&self, name: &str) -> Option<NodeId> {
        self.by_name
            .get(name)
            .map(|&file| self.program.files()[self.files[file].index].source())
    }

    fn index(&self, file: usize) -> NodeIndex {
        if let Some(index) = self.files[file].nodes.borrow().as_ref() {
            return index.clone();
        }
        let view = self.view(file);
        let root = self.program.files()[self.files[file].index].source();
        let mut index: HashMap<(i32, i32), Vec<NodeId>> = HashMap::new();
        let mut work = vec![root];
        while let Some(id) = work.pop() {
            if let Ok(node) = view.node(id) {
                index.entry((node.pos(), node.end())).or_default().push(id);
            }
            let mut next = children(view, id);
            next.reverse();
            work.extend(next);
        }
        let index = Rc::new(index);
        *self.files[file].nodes.borrow_mut() = Some(index.clone());
        index
    }

    /// Every JSDoc node of the file, by position, built the first time a
    /// lookup misses the main tree.
    fn jsdoc_index(&self, file: usize) -> NodeIndex {
        if let Some(index) = self.files[file].jsdoc.borrow().as_ref() {
            return index.clone();
        }
        let view = self.view(file);
        let source = self.program.files()[self.files[file].index].source();
        let mut index: HashMap<(i32, i32), Vec<NodeId>> = HashMap::new();
        let hosts: Vec<NodeId> = self.index(file).values().flatten().copied().collect();
        let mut provider = tsr_parser::ParserJsDocProvider::default();
        for host in hosts {
            let Ok(roots) = provider.jsdoc(view, source, host) else {
                continue;
            };
            let mut work: Vec<NodeId> = roots.iter().copied().collect();
            while let Some(id) = work.pop() {
                let Ok(owner) = view.for_node_owner(id) else {
                    continue;
                };
                if let Ok(node) = owner.node(id) {
                    index.entry((node.pos(), node.end())).or_default().push(id);
                }
                work.extend(children(owner, id));
            }
        }
        let index = Rc::new(index);
        *self.files[file].jsdoc.borrow_mut() = Some(index.clone());
        index
    }

    /// The node the recorder describes as `{file, pos, end, kind}`.
    pub fn lookup(&self, reference: &Value) -> Result<NodeId, String> {
        let name = reference["file"].as_str().ok_or("node without file")?;
        let &file = self
            .by_name
            .get(name)
            .ok_or_else(|| format!("file {name} is not in the program"))?;
        let pos = reference["pos"].as_i64().ok_or("node without pos")? as i32;
        let end = reference["end"].as_i64().ok_or("node without end")? as i32;
        let kind = reference["kind"].as_str().ok_or("node without kind")?;
        let view = self.view(file);
        let pick = |candidates: Option<&Vec<NodeId>>| -> Option<NodeId> {
            candidates?.iter().copied().find(|&id| {
                view.for_node_owner(id)
                    .and_then(|owner| owner.node(id))
                    .is_ok_and(|node| node.kind().to_string() == kind)
            })
        };
        if let Some(id) = pick(self.index(file).get(&(pos, end))) {
            return Ok(id);
        }
        if let Some(id) = pick(self.jsdoc_index(file).get(&(pos, end))) {
            return Ok(id);
        }
        // A token the language service made on demand (`GetTokenAtPosition`
        // and friends): the file's token cache gives the same node back.
        if let Some(id) = self.token(file, pos, end, kind) {
            return Ok(id);
        }
        Err(format!("no {kind} at {name}:{pos}-{end}"))
    }

    fn token(&self, file: usize, pos: i32, end: i32, kind: &str) -> Option<NodeId> {
        let view = self.view(file);
        let source = self.program.files()[self.files[file].index].source();
        let mut provider = tsr_parser::ParserJsDocProvider::default();
        let mut navigator = tsr_astnav::Navigator::new(view, source, &mut provider);
        let mut candidates = Vec::new();
        for position in [i64::from(end) - 1, i64::from(pos), i64::from(end)] {
            if position < 0 {
                continue;
            }
            candidates.extend(navigator.get_token_at_position(position).ok());
            candidates.extend(navigator.get_touching_token(position).ok());
            candidates.extend(navigator.find_preceding_token(position).ok().flatten());
        }
        candidates.into_iter().find(|&id| {
            view.for_node_owner(id)
                .and_then(|owner| owner.node(id))
                .is_ok_and(|node| {
                    node.pos() == pos && node.end() == end && node.kind().to_string() == kind
                })
        })
    }

    /// The source file node of a program node.
    pub fn source_of(&self, id: NodeId) -> Option<NodeId> {
        let file = self.by_arena.get(&id.arena()).copied().or_else(|| {
            (0..self.files.len()).find(|&file| self.view(file).for_node_owner(id).is_ok())
        })?;
        Some(self.program.files()[self.files[file].index].source())
    }

    /// A source node as the recorder describes it.
    pub fn describe(&self, id: NodeId) -> Value {
        let file = self.by_arena.get(&id.arena()).copied().or_else(|| {
            (0..self.files.len()).find(|&file| self.view(file).for_node_owner(id).is_ok())
        });
        let Some(file) = file else {
            return json!({"unknown_node": format!("{id:?}")});
        };
        let view = self.view(file);
        match view.for_node_owner(id).and_then(|owner| owner.node(id)) {
            Ok(node) => json!({"node": {
                "file": self.files[file].name,
                "pos": node.pos(),
                "end": node.end(),
                "kind": node.kind().to_string(),
            }}),
            Err(error) => json!({"unknown_node": format!("{id:?}: {error:?}")}),
        }
    }
}

/// The recorder's tree for a node the builder made (`phase2Tree`).
pub fn tree(view: AstView<'_>, id: NodeId) -> Value {
    use tsr_ast::SyntaxKind as K;
    let Ok(node) = view.node(id) else {
        return json!({"unreadable": format!("{id:?}")});
    };
    let kind = node.kind();
    let mut tree = Map::new();
    tree.insert("kind".into(), json!(kind.to_string()));
    if matches!(
        kind.known(),
        Some(
            K::Identifier
                | K::PrivateIdentifier
                | K::StringLiteral
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::TemplateHead
                | K::TemplateMiddle
                | K::TemplateTail
                | K::RegularExpressionLiteral
        )
    ) {
        if let Ok(text) = view.node_text(id) {
            tree.insert(
                "text".into(),
                json!(String::from_utf8_lossy(text.as_bytes())),
            );
        }
    }
    let list: Vec<Value> = children(view, id)
        .into_iter()
        .map(|child| tree_of(view, child))
        .collect();
    if !list.is_empty() {
        tree.insert("children".into(), Value::Array(list));
    }
    Value::Object(tree)
}

fn tree_of(view: AstView<'_>, id: NodeId) -> Value {
    match view.for_node_owner(id) {
        Ok(owner) => tree(owner, id),
        Err(_) => tree(view, id),
    }
}

#[derive(Clone, Copy)]
enum Binding {
    Type(u32),
    Symbol(SymbolId),
    Signature(u32),
}

/// One recorded checker's replay state: which Rust value each recorder token
/// stands for, and the definitions the recorder wrote for every token.
pub struct Replay {
    pub program: Rc<ProgramState>,
    types: HashMap<String, TypeRef>,
    type_tokens: HashMap<u32, String>,
    symbols: HashMap<String, SymbolRef>,
    symbol_tokens: HashMap<SymbolId, String>,
    signatures: HashMap<String, SignatureRef>,
    signature_tokens: HashMap<u32, String>,
    defs: HashMap<String, Value>,
    /// Index infos and predicates seen in results, by their recorded form,
    /// so a later call can pass them back.
    index_infos: Vec<(Value, IndexInfoRef)>,
    predicates: Vec<(Value, TypePredicateRef)>,
    trail: Vec<(String, Binding)>,
}

type Compared = Result<(), String>;

impl Replay {
    pub fn new(program: Rc<ProgramState>) -> Self {
        Self {
            program,
            types: HashMap::new(),
            type_tokens: HashMap::new(),
            symbols: HashMap::new(),
            symbol_tokens: HashMap::new(),
            signatures: HashMap::new(),
            signature_tokens: HashMap::new(),
            defs: HashMap::new(),
            index_infos: Vec::new(),
            predicates: Vec::new(),
            trail: Vec::new(),
        }
    }

    pub fn add_defs(&mut self, defs: &Map<String, Value>) {
        for (token, def) in defs {
            self.defs
                .entry(token.clone())
                .or_insert_with(|| def.clone());
        }
    }

    fn bind(&mut self, token: &str, binding: Binding) {
        match binding {
            Binding::Type(id) => {
                self.type_tokens.insert(id, token.to_string());
            }
            Binding::Symbol(id) => {
                self.symbol_tokens.insert(id, token.to_string());
            }
            Binding::Signature(id) => {
                self.signature_tokens.insert(id, token.to_string());
            }
        }
        self.trail.push((token.to_string(), binding));
    }

    fn rollback(&mut self, mark: usize) {
        while self.trail.len() > mark {
            let (token, binding) = self.trail.pop().expect("trail entry");
            match binding {
                Binding::Type(id) => {
                    self.type_tokens.remove(&id);
                    self.types.remove(&token);
                }
                Binding::Symbol(id) => {
                    self.symbol_tokens.remove(&id);
                    self.symbols.remove(&token);
                }
                Binding::Signature(id) => {
                    self.signature_tokens.remove(&id);
                    self.signatures.remove(&token);
                }
            }
        }
    }

    fn def(&self, token: &str) -> Result<&Value, String> {
        self.defs
            .get(token)
            .ok_or_else(|| format!("{token} has no recorded definition"))
    }

    fn node_eq(&self, recorded: &Value, actual: Option<NodeId>, path: &str) -> Compared {
        let described = actual.map_or(Value::Null, |id| self.program.describe(id));
        if &described == recorded {
            Ok(())
        } else {
            Err(format!("{path}: go {recorded} rust {described}"))
        }
    }

    fn type_list(
        &mut self,
        op: &Operation<'_>,
        recorded: &Value,
        actual: &[TypeRef],
        path: &str,
    ) -> Compared {
        let empty = Vec::new();
        let recorded = match recorded {
            Value::Null => &empty,
            Value::Array(list) => list,
            other => return Err(format!("{path}: go {other} rust a type list")),
        };
        if recorded.len() != actual.len() {
            return Err(format!(
                "{path}: go {} types rust {}",
                recorded.len(),
                actual.len()
            ));
        }
        for (i, (recorded, &actual)) in recorded.iter().zip(actual).enumerate() {
            self.type_value(op, recorded, actual, &format!("{path}[{i}]"))?;
        }
        Ok(())
    }

    fn type_value(
        &mut self,
        op: &Operation<'_>,
        recorded: &Value,
        actual: TypeRef,
        path: &str,
    ) -> Compared {
        let token = recorded["type"]
            .as_str()
            .ok_or_else(|| format!("{path}: go {recorded} rust a type"))?;
        self.match_type(op, token, actual, path)
    }

    fn opt_type(
        &mut self,
        op: &Operation<'_>,
        recorded: &Value,
        actual: Option<TypeRef>,
        path: &str,
    ) -> Compared {
        match (recorded, actual) {
            (Value::Null, None) => Ok(()),
            (Value::Null, Some(_)) => Err(format!("{path}: go nil rust a type")),
            (_, None) => Err(format!("{path}: go {recorded} rust nil")),
            (_, Some(actual)) => self.type_value(op, recorded, actual, path),
        }
    }

    fn opt_symbol(
        &mut self,
        op: &Operation<'_>,
        recorded: &Value,
        actual: Option<SymbolRef>,
        path: &str,
    ) -> Compared {
        match (recorded, actual) {
            (Value::Null, None) => Ok(()),
            (Value::Null, Some(_)) => Err(format!("{path}: go nil rust a symbol")),
            (_, None) => Err(format!("{path}: go {recorded} rust nil")),
            (_, Some(actual)) => {
                let token = recorded["symbol"]
                    .as_str()
                    .ok_or_else(|| format!("{path}: go {recorded} rust a symbol"))?;
                self.match_symbol(op, token, actual, path)
            }
        }
    }

    pub fn match_type(
        &mut self,
        op: &Operation<'_>,
        token: &str,
        actual: TypeRef,
        path: &str,
    ) -> Compared {
        if let Some(bound) = self.types.get(token) {
            return if *bound == actual {
                Ok(())
            } else {
                Err(format!("{path}: {token} is already another rust type"))
            };
        }
        if let Some(other) = self.type_tokens.get(&actual.id()) {
            return Err(format!(
                "{path}: rust type is already {other}, go has {token}"
            ));
        }
        let def = self.def(token)?.clone();
        let path = format!("{path}={token}");
        self.types.insert(token.to_string(), actual);
        self.bind(token, Binding::Type(actual.id()));
        let view = op
            .replay_type_view(actual)
            .map_err(|e| format!("{path}: {e:?}"))?;
        let flags = def["flags"].as_u64().unwrap_or(0) as u32;
        if flags != view.flags {
            return Err(format!(
                "{path}.flags: go {flags:#x} rust {:#x}",
                view.flags
            ));
        }
        let object_flags = def["object_flags"].as_u64().unwrap_or(0) as u32;
        if object_flags != view.object_flags & STABLE_OBJECT_FLAGS {
            return Err(format!(
                "{path}.object_flags: go {object_flags:#x} rust {:#x}",
                view.object_flags & STABLE_OBJECT_FLAGS
            ));
        }
        if let Some(kind) = def["kind"].as_str() {
            if kind != view.kind {
                return Err(format!("{path}.kind: go {kind} rust {}", view.kind));
            }
        }
        self.opt_symbol(op, &def["symbol"], view.symbol, &format!("{path}.symbol"))?;
        match (&def["alias"], &view.alias) {
            (Value::Null, None) => {}
            (Value::Null, Some(_)) => return Err(format!("{path}.alias: go none rust an alias")),
            (_, None) => return Err(format!("{path}.alias: go an alias rust none")),
            (recorded, Some((symbol, arguments))) => {
                self.opt_symbol(
                    op,
                    &recorded["symbol"],
                    Some(*symbol),
                    &format!("{path}.alias.symbol"),
                )?;
                self.type_list(
                    op,
                    &recorded["arguments"],
                    arguments,
                    &format!("{path}.alias.arguments"),
                )?;
            }
        }
        match &view.data {
            TypeViewData::None => {}
            TypeViewData::Intrinsic(name) => {
                let name = normalize_name(name.as_bytes());
                if def["intrinsic"].as_str() != Some(name.as_str()) {
                    return Err(format!(
                        "{path}.intrinsic: go {} rust {name}",
                        def["intrinsic"]
                    ));
                }
            }
            TypeViewData::Literal(value) => {
                let described = match value {
                    LiteralView::String(text) => json!({"string": normalize_name(text.as_bytes())}),
                    LiteralView::Number(number) => json!({"number": number.to_string()}),
                    LiteralView::Boolean(value) => json!({"boolean": value}),
                    LiteralView::BigInt(value) => json!({
                        "bigint": String::from_utf8_lossy(&value.base10_value),
                        "negative": value.negative,
                    }),
                    LiteralView::Computed => Value::Null,
                };
                if def["value"] != described {
                    return Err(format!(
                        "{path}.value: go {} rust {described}",
                        def["value"]
                    ));
                }
            }
            TypeViewData::Unique(name) => {
                let name = normalize_name(name.as_bytes());
                if def["unique"].as_str() != Some(name.as_str()) {
                    return Err(format!("{path}.unique: go {} rust {name}", def["unique"]));
                }
            }
            TypeViewData::Types(list) => {
                self.type_list(op, &def["types"], list, &format!("{path}.types"))?;
            }
            TypeViewData::TypeParameter {
                is_this_type,
                target,
            } => {
                if def["is_this_type"].as_bool() != Some(*is_this_type) {
                    return Err(format!(
                        "{path}.is_this_type: go {} rust {is_this_type}",
                        def["is_this_type"]
                    ));
                }
                self.opt_type(op, &def["target"], *target, &format!("{path}.target"))?;
            }
            TypeViewData::Index { target, flags } => {
                if def["index_flags"].as_u64() != Some(u64::from(*flags)) {
                    return Err(format!(
                        "{path}.index_flags: go {} rust {flags}",
                        def["index_flags"]
                    ));
                }
                self.type_value(op, &def["target"], *target, &format!("{path}.target"))?;
            }
            TypeViewData::IndexedAccess { object, index } => {
                self.type_value(op, &def["object"], *object, &format!("{path}.object"))?;
                self.type_value(op, &def["index"], *index, &format!("{path}.index"))?;
            }
            TypeViewData::Conditional {
                root,
                check,
                extends,
            } => {
                self.node_eq(&def["root"], Some(*root), &format!("{path}.root"))?;
                self.type_value(op, &def["check"], *check, &format!("{path}.check"))?;
                self.type_value(op, &def["extends"], *extends, &format!("{path}.extends"))?;
            }
            TypeViewData::Substitution { base, constraint } => {
                self.type_value(op, &def["base"], *base, &format!("{path}.base"))?;
                self.type_value(
                    op,
                    &def["constraint"],
                    *constraint,
                    &format!("{path}.constraint"),
                )?;
            }
            TypeViewData::StringMapping { target } => {
                self.type_value(op, &def["target"], *target, &format!("{path}.target"))?;
            }
            TypeViewData::Template { texts, types } => {
                let texts: Vec<String> =
                    texts.iter().map(|t| normalize_name(t.as_bytes())).collect();
                if def["texts"] != json!(texts) {
                    return Err(format!("{path}.texts: go {} rust {texts:?}", def["texts"]));
                }
                self.type_list(op, &def["types"], types, &format!("{path}.types"))?;
            }
            TypeViewData::Mapped { declaration } => {
                self.node_eq(
                    &def["declaration"],
                    *declaration,
                    &format!("{path}.declaration"),
                )?;
            }
            TypeViewData::Tuple {
                element_flags,
                readonly,
            } => {
                if def["element_flags"] != json!(element_flags)
                    || def["readonly"].as_bool() != Some(*readonly)
                {
                    return Err(format!(
                        "{path}.tuple: go {} {} rust {element_flags:?} {readonly}",
                        def["element_flags"], def["readonly"]
                    ));
                }
            }
            TypeViewData::Reference {
                target,
                arguments,
                node,
            } => {
                self.opt_type(op, &def["target"], *target, &format!("{path}.target"))?;
                // Type arguments resolve lazily; compare them only when both
                // checkers have resolved them.
                if let (Some(recorded), Some(arguments)) = (def.get("arguments"), arguments) {
                    self.type_list(op, recorded, arguments, &format!("{path}.arguments"))?;
                }
                let recorded = def.get("node").unwrap_or(&Value::Null);
                self.node_eq(recorded, *node, &format!("{path}.node"))?;
            }
        }
        Ok(())
    }

    pub fn match_symbol(
        &mut self,
        op: &Operation<'_>,
        token: &str,
        actual: SymbolRef,
        path: &str,
    ) -> Compared {
        if let Some(bound) = self.symbols.get(token) {
            return if *bound == actual {
                Ok(())
            } else {
                Err(format!("{path}: {token} is already another rust symbol"))
            };
        }
        if let Some(other) = self.symbol_tokens.get(&actual.id()) {
            return Err(format!(
                "{path}: rust symbol is already {other}, go has {token}"
            ));
        }
        let def = self.def(token)?.clone();
        let path = format!("{path}={token}");
        self.symbols.insert(token.to_string(), actual);
        self.bind(token, Binding::Symbol(actual.id()));
        if let Some(builtin) = def["builtin"].as_str() {
            let rust = op
                .replay_builtin_name(actual)
                .map_err(|e| format!("{path}: {e:?}"))?;
            if rust != Some(builtin) {
                return Err(format!("{path}.builtin: go {builtin} rust {rust:?}"));
            }
        }
        let (name, flags, check_flags, value_declaration, parent) = {
            let symbol = op.symbol(actual).map_err(|e| format!("{path}: {e:?}"))?;
            (
                normalize_name(symbol.name_bytes()),
                symbol.flags(),
                symbol.check_flags(),
                symbol.value_declaration(),
                symbol.parent(),
            )
        };
        if def["name"].as_str() != Some(name.as_str()) {
            return Err(format!("{path}.name: go {} rust {name:?}", def["name"]));
        }
        if def["flags"].as_u64() != Some(u64::from(flags)) {
            return Err(format!("{path}.flags: go {} rust {flags:#x}", def["flags"]));
        }
        if def["check_flags"].as_u64() != Some(u64::from(check_flags)) {
            return Err(format!(
                "{path}.check_flags: go {} rust {check_flags:#x}",
                def["check_flags"]
            ));
        }
        let declarations: Vec<Value> = op
            .symbol_declarations(actual)
            .map_err(|e| format!("{path}: {e:?}"))?
            .iter()
            .map(|id| id.map_or(Value::Null, |id| self.program.describe(id)))
            .collect();
        let recorded = def["declarations"].as_array().cloned().unwrap_or_default();
        if recorded != declarations {
            return Err(format!(
                "{path}.declarations: go {recorded:?} rust {declarations:?}"
            ));
        }
        self.node_eq(
            &def["value_declaration"],
            value_declaration,
            &format!("{path}.value_declaration"),
        )?;
        let parent = parent
            .map(|id| op.symbol_ref(id))
            .transpose()
            .map_err(|e| format!("{path}: {e:?}"))?;
        let recorded_parent = def.get("parent").cloned().unwrap_or(Value::Null);
        self.opt_symbol(op, &recorded_parent, parent, &format!("{path}.parent"))
    }

    pub fn match_signature(
        &mut self,
        op: &Operation<'_>,
        token: &str,
        actual: SignatureRef,
        path: &str,
    ) -> Compared {
        if let Some(bound) = self.signatures.get(token) {
            return if *bound == actual {
                Ok(())
            } else {
                Err(format!("{path}: {token} is already another rust signature"))
            };
        }
        if let Some(other) = self.signature_tokens.get(&actual.id()) {
            return Err(format!(
                "{path}: rust signature is already {other}, go has {token}"
            ));
        }
        let def = self.def(token)?.clone();
        let path = format!("{path}={token}");
        self.signatures.insert(token.to_string(), actual);
        self.bind(token, Binding::Signature(actual.id()));
        let view = op
            .replay_signature_view(actual)
            .map_err(|e| format!("{path}: {e:?}"))?;
        if def["flags"].as_u64() != Some(u64::from(view.flags)) {
            return Err(format!(
                "{path}.flags: go {} rust {:#x}",
                def["flags"], view.flags
            ));
        }
        if def["min_argument_count"].as_i64() != Some(i64::from(view.min_argument_count)) {
            return Err(format!(
                "{path}.min_argument_count: go {} rust {}",
                def["min_argument_count"], view.min_argument_count
            ));
        }
        if def["composite"].as_bool() != Some(view.composite) {
            return Err(format!(
                "{path}.composite: go {} rust {}",
                def["composite"], view.composite
            ));
        }
        self.node_eq(
            &def["declaration"],
            view.declaration,
            &format!("{path}.declaration"),
        )?;
        // Go does not distinguish a nil list from an empty one here.
        let type_parameters = view.type_parameters.clone().unwrap_or_default();
        self.type_list(
            op,
            &def["type_parameters"],
            &type_parameters,
            &format!("{path}.type_parameters"),
        )?;
        let parameters = view.parameters.clone().unwrap_or_default();
        let recorded = match &def["parameters"] {
            Value::Array(list) => list.clone(),
            _ => Vec::new(),
        };
        if recorded.len() != parameters.len() {
            return Err(format!(
                "{path}.parameters: go {} rust {}",
                recorded.len(),
                parameters.len()
            ));
        }
        for (i, (recorded, parameter)) in recorded.iter().zip(parameters).enumerate() {
            self.opt_symbol(
                op,
                recorded,
                Some(parameter),
                &format!("{path}.parameters[{i}]"),
            )?;
        }
        self.opt_symbol(
            op,
            &def["this_parameter"],
            view.this_parameter,
            &format!("{path}.this_parameter"),
        )?;
        match (&def["target"], view.target) {
            (Value::Null, None) => Ok(()),
            (recorded, Some(target)) => {
                let token = recorded["signature"]
                    .as_str()
                    .ok_or_else(|| format!("{path}.target: go {recorded} rust a signature"))?;
                self.match_signature(op, token, target, &format!("{path}.target"))
            }
            (recorded, None) => Err(format!("{path}.target: go {recorded} rust nil")),
        }
    }

    /// Compares one result, rolling back every binding it made if it differs.
    pub fn compare(
        &mut self,
        op: &Operation<'_>,
        recorded: &Value,
        actual: &Actual,
        path: &str,
    ) -> Compared {
        let mark = self.trail.len();
        let result = self.compare_inner(op, recorded, actual, path);
        if result.is_err() {
            self.rollback(mark);
        }
        result
    }

    fn compare_inner(
        &mut self,
        op: &Operation<'_>,
        recorded: &Value,
        actual: &Actual,
        path: &str,
    ) -> Compared {
        let differ =
            |rust: &dyn std::fmt::Debug| Err(format!("{path}: go {recorded} rust {rust:?}"));
        match actual {
            Actual::Null => match recorded {
                Value::Null => Ok(()),
                Value::Array(list) if list.is_empty() => Ok(()),
                _ => differ(&"nil"),
            },
            Actual::Bool(value) => {
                if recorded.as_bool() == Some(*value) {
                    Ok(())
                } else {
                    differ(value)
                }
            }
            Actual::Int(value) => {
                if recorded.as_i64() == Some(*value) {
                    Ok(())
                } else {
                    differ(value)
                }
            }
            Actual::Str(value) => {
                if recorded.as_str() == Some(value.as_str()) {
                    Ok(())
                } else {
                    differ(value)
                }
            }
            Actual::Number(value) => {
                if recorded["number"].as_str() == Some(value.as_str()) {
                    Ok(())
                } else {
                    differ(value)
                }
            }
            Actual::Type(ty) => self.type_value(op, recorded, *ty, path),
            Actual::Symbol(symbol) => self.opt_symbol(op, recorded, Some(*symbol), path),
            Actual::Signature(signature) => {
                let token = recorded["signature"]
                    .as_str()
                    .ok_or_else(|| format!("{path}: go {recorded} rust a signature"))?;
                self.match_signature(op, token, *signature, path)
            }
            Actual::Node(id) => self.node_eq(recorded, Some(*id), path),
            Actual::Index(info) => {
                let parts = op
                    .index_info_parts(*info)
                    .map_err(|e| format!("{path}: {e:?}"))?;
                let recorded_info = &recorded["index"];
                self.type_value(
                    op,
                    &recorded_info["key"],
                    parts.key_type,
                    &format!("{path}.key"),
                )?;
                self.type_value(
                    op,
                    &recorded_info["value"],
                    parts.value_type,
                    &format!("{path}.value"),
                )?;
                if recorded_info["readonly"].as_bool() != Some(parts.is_readonly) {
                    return Err(format!(
                        "{path}.readonly: go {} rust {}",
                        recorded_info["readonly"], parts.is_readonly
                    ));
                }
                self.node_eq(
                    &recorded_info["declaration"],
                    parts.declaration,
                    &format!("{path}.declaration"),
                )?;
                self.index_infos.push((recorded.clone(), *info));
                Ok(())
            }
            Actual::Predicate(predicate) => {
                let parts = op
                    .type_predicate_parts(*predicate)
                    .map_err(|e| format!("{path}: {e:?}"))?;
                let recorded_predicate = &recorded["predicate"];
                let kind = parts.kind as i64;
                if recorded_predicate["kind"].as_i64() != Some(kind)
                    || recorded_predicate["index"].as_i64()
                        != Some(i64::from(parts.parameter_index))
                    || recorded_predicate["name"].as_str()
                        != Some(normalize_name(parts.parameter_name.as_bytes()).as_str())
                {
                    return Err(format!(
                        "{path}: go {recorded_predicate} rust kind {kind} index {} name {:?}",
                        parts.parameter_index,
                        normalize_name(parts.parameter_name.as_bytes())
                    ));
                }
                self.opt_type(
                    op,
                    &recorded_predicate["type"],
                    parts.r#type,
                    &format!("{path}.type"),
                )?;
                self.predicates.push((recorded.clone(), *predicate));
                Ok(())
            }
            Actual::Tree(tree) => {
                let recorded_tree = strip_sources(&recorded["synthetic"]);
                if recorded_tree == *tree {
                    Ok(())
                } else {
                    Err(format!("{path}: go {recorded_tree} rust {tree}"))
                }
            }
            Actual::List(items) => {
                let empty = Vec::new();
                let list = match recorded {
                    Value::Null => &empty,
                    Value::Array(list) => list,
                    _ => return differ(&"a list"),
                };
                if list.len() != items.len() {
                    let described: Vec<&Value> = items
                        .iter()
                        .filter_map(|item| match item {
                            Actual::Json(value) => Some(value),
                            _ => None,
                        })
                        .collect();
                    let rust = if described.is_empty() {
                        String::new()
                    } else {
                        format!(
                            ": {}",
                            Value::Array(described.into_iter().cloned().collect())
                        )
                    };
                    return Err(format!(
                        "{path}: go {} items rust {}{rust}",
                        list.len(),
                        items.len()
                    ));
                }
                for (i, (recorded, actual)) in list.iter().zip(items).enumerate() {
                    self.compare_inner(op, recorded, actual, &format!("{path}[{i}]"))?;
                }
                Ok(())
            }
            Actual::Unordered(items) => {
                let sorted = self.sort_symbols(op, items)?;
                let actual = Actual::List(sorted.into_iter().map(Actual::Symbol).collect());
                self.compare_inner(op, recorded, &actual, path)
            }
            Actual::Object => {
                if recorded.is_object() {
                    Ok(())
                } else {
                    differ(&"an object")
                }
            }
            Actual::Json(value) => {
                if recorded == value {
                    Ok(())
                } else {
                    differ(value)
                }
            }
            Actual::Fields(fields) => {
                for (name, actual) in fields {
                    let recorded = recorded.get(*name).unwrap_or(&Value::Null);
                    self.compare_inner(op, recorded, actual, &format!("{path}.{name}"))?;
                }
                Ok(())
            }
        }
    }

    /// The recorder's order for a map-ordered symbol list: name, then the
    /// first declaration's file and position, then the flags.
    fn sort_symbols(
        &self,
        op: &Operation<'_>,
        items: &[SymbolRef],
    ) -> Result<Vec<SymbolRef>, String> {
        let mut keyed = Vec::with_capacity(items.len());
        for &symbol in items {
            let (name, flags, check_flags) = {
                let read = op.symbol(symbol).map_err(|e| format!("{e:?}"))?;
                (read.name_bytes().to_vec(), read.flags(), read.check_flags())
            };
            let position = op
                .symbol_declarations(symbol)
                .map_err(|e| format!("{e:?}"))?
                .iter()
                .next()
                .flatten()
                .map(|id| {
                    let described = self.program.describe(id);
                    let node = &described["node"];
                    format!(
                        "{}:{:09}:{:09}",
                        node["file"].as_str().unwrap_or_default(),
                        node["pos"].as_i64().unwrap_or_default(),
                        node["end"].as_i64().unwrap_or_default()
                    )
                })
                .unwrap_or_default();
            let mut key = name;
            key.push(0);
            key.extend_from_slice(position.as_bytes());
            key.push(0);
            key.extend_from_slice(format!("{flags:08x}\0{check_flags:08x}").as_bytes());
            keyed.push((key, symbol));
        }
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(keyed.into_iter().map(|(_, symbol)| symbol).collect())
    }

    // Arguments.

    pub fn type_arg(&mut self, op: &mut Operation<'_>, value: &Value) -> Result<TypeRef, Outcome> {
        let token = value["type"]
            .as_str()
            .ok_or_else(|| Outcome::Unsupported(format!("expected a type, got {value}")))?;
        if let Some(ty) = self.types.get(token) {
            return Ok(*ty);
        }
        self.resolve_type(op, token)
    }

    /// A type the language service reached without a checker call: an
    /// intrinsic, or the declared type of a class, interface or type
    /// parameter it read off a declaration.
    fn resolve_type(&mut self, op: &mut Operation<'_>, token: &str) -> Result<TypeRef, Outcome> {
        let def = self
            .defs
            .get(token)
            .cloned()
            .ok_or_else(|| Outcome::Unmapped(format!("type {token} has no definition")))?;
        let mut candidates = Vec::new();
        match def["kind"].as_str() {
            Some("IntrinsicType") => {
                for name in tsr_checker::BUILTIN_TYPE_NAMES {
                    candidates.extend(op.builtin_type(name));
                }
            }
            Some("TypeParameter" | "InterfaceType") if def["symbol"].is_object() => {
                let symbol = self.symbol_arg(op, &def["symbol"])?;
                // The pin had already created this declared type.
                candidates.extend(op.get_declared_type_of_symbol(symbol).ok());
            }
            _ => {}
        }
        let mut reasons = Vec::new();
        for candidate in candidates {
            match self.compare(op, &json!({"type": token}), &Actual::Type(candidate), "arg") {
                Ok(()) => return Ok(candidate),
                Err(reason) => reasons.push(reason),
            }
        }
        Err(Outcome::Unmapped(format!(
            "type {token} ({}): {}",
            def["kind"],
            if reasons.is_empty() {
                "no candidate".to_string()
            } else {
                reasons.join("; ").chars().take(300).collect()
            }
        )))
    }

    pub fn opt_type_arg(
        &mut self,
        op: &mut Operation<'_>,
        value: &Value,
    ) -> Result<Option<TypeRef>, Outcome> {
        if value.is_null() {
            Ok(None)
        } else {
            self.type_arg(op, value).map(Some)
        }
    }

    pub fn types_arg(
        &mut self,
        op: &mut Operation<'_>,
        value: &Value,
    ) -> Result<Vec<TypeRef>, Outcome> {
        match value {
            Value::Null => Ok(Vec::new()),
            Value::Array(list) => list.iter().map(|item| self.type_arg(op, item)).collect(),
            other => Err(Outcome::Unsupported(format!("expected types, got {other}"))),
        }
    }

    pub fn signature_arg(&mut self, value: &Value) -> Result<SignatureRef, Outcome> {
        let token = value["signature"]
            .as_str()
            .ok_or_else(|| Outcome::Unsupported(format!("expected a signature, got {value}")))?;
        self.signatures
            .get(token)
            .copied()
            .ok_or_else(|| Outcome::Unmapped(format!("signature {token}")))
    }

    pub fn symbol_arg(&mut self, op: &Operation<'_>, value: &Value) -> Result<SymbolRef, Outcome> {
        let token = value["symbol"]
            .as_str()
            .ok_or_else(|| Outcome::Unsupported(format!("expected a symbol, got {value}")))?;
        if let Some(symbol) = self.symbols.get(token) {
            return Ok(*symbol);
        }
        self.resolve_symbol(op, token)
    }

    pub fn opt_symbol_arg(
        &mut self,
        op: &Operation<'_>,
        value: &Value,
    ) -> Result<Option<SymbolRef>, Outcome> {
        if value.is_null() {
            Ok(None)
        } else {
            self.symbol_arg(op, value).map(Some)
        }
    }

    /// A symbol the language service reached without a checker call (a
    /// node's symbol, a table member): the Rust symbol its first declaration
    /// binds, or its merged or export symbol, whichever matches the record.
    fn resolve_symbol(&mut self, op: &Operation<'_>, token: &str) -> Result<SymbolRef, Outcome> {
        let def = self
            .defs
            .get(token)
            .cloned()
            .ok_or_else(|| Outcome::Unmapped(format!("symbol {token} has no definition")))?;
        let mut candidates = Vec::new();
        if let Some(builtin) = def["builtin"].as_str() {
            if let Some(symbol) = op.replay_builtin_symbol(builtin).map_err(Outcome::from)? {
                candidates.push(symbol);
            }
        }
        if let Some(first) = def["declarations"].as_array().and_then(|list| list.first()) {
            let reference = &first["node"];
            if reference.is_null() {
                return Err(Outcome::Unmapped(format!(
                    "symbol {token} declared by synthetic syntax"
                )));
            }
            let node = self
                .program
                .lookup(reference)
                .map_err(|reason| Outcome::Unmapped(format!("symbol {token}: {reason}")))?;
            let mut bound = Vec::new();
            for file in self.program.program.files() {
                if let Ok(Some(binding)) = file.bound().view().node_binding(node) {
                    bound.extend(binding.symbol);
                    bound.extend(binding.local_symbol);
                    break;
                }
            }
            for id in bound {
                let Ok(symbol) = op.symbol_ref(id) else {
                    continue;
                };
                candidates.push(symbol);
                if let Ok(merged) = op.get_merged_symbol(symbol) {
                    candidates.push(merged);
                }
                if let Ok(export) = op.get_export_symbol_of_symbol(symbol) {
                    candidates.push(export);
                }
                if let Ok(read) = op.symbol(symbol) {
                    if let Some(export) = read.export_symbol() {
                        if let Ok(export) = op.symbol_ref(export) {
                            candidates.push(export);
                        }
                    }
                }
            }
        }
        let mut reasons = Vec::new();
        for candidate in candidates {
            match self.compare(
                op,
                &json!({"symbol": token}),
                &Actual::Symbol(candidate),
                "arg",
            ) {
                Ok(()) => return Ok(candidate),
                Err(reason) => reasons.push(reason),
            }
        }
        Err(Outcome::Unmapped(format!(
            "symbol {token} ({}): {}",
            def["name"],
            if reasons.is_empty() {
                "no candidate".to_string()
            } else {
                reasons.join("; ")
            }
        )))
    }

    pub fn node_arg(&self, value: &Value) -> Result<NodeId, Outcome> {
        if let Some(reference) = value.get("node") {
            // The program's own synthesized nodes (the import helpers and JSX
            // runtime specifiers) have no position; the Rust program has none.
            if reference["pos"].as_i64() == Some(-1) && reference["end"].as_i64() == Some(-1) {
                return Err(Outcome::Unsupported(
                    "synthesized import specifier argument".to_string(),
                ));
            }
            return self.program.lookup(reference).map_err(Outcome::Unmapped);
        }
        if let Some(name) = value.get("source_file").and_then(Value::as_str) {
            return self
                .program
                .source(name)
                .ok_or_else(|| Outcome::Unmapped(format!("source file {name}")));
        }
        if value.get("synthetic").is_some() {
            return Err(Outcome::Unsupported("synthetic node argument".to_string()));
        }
        Err(Outcome::Unsupported(format!(
            "expected a node, got {value}"
        )))
    }

    pub fn opt_node_arg(&self, value: &Value) -> Result<Option<NodeId>, Outcome> {
        if value.is_null() {
            Ok(None)
        } else {
            self.node_arg(value).map(Some)
        }
    }

    pub fn index_info_arg(&self, value: &Value) -> Result<IndexInfoRef, Outcome> {
        self.index_infos
            .iter()
            .find(|(recorded, _)| recorded == value)
            .map(|(_, info)| *info)
            .ok_or_else(|| Outcome::Unmapped("index info".to_string()))
    }

    pub fn predicate_arg(&self, value: &Value) -> Result<TypePredicateRef, Outcome> {
        self.predicates
            .iter()
            .find(|(recorded, _)| recorded == value)
            .map(|(_, predicate)| *predicate)
            .ok_or_else(|| Outcome::Unmapped("type predicate".to_string()))
    }
}

/// The recorder writes where a reused source node came from; the replay
/// compares the tree's structure only.
fn strip_sources(tree: &Value) -> Value {
    match tree {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "source")
                .map(|(key, value)| (key.clone(), strip_sources(value)))
                .collect(),
        ),
        Value::Array(list) => Value::Array(list.iter().map(strip_sources).collect()),
        other => other.clone(),
    }
}

pub fn str_arg(value: &Value) -> Result<&str, Outcome> {
    value
        .as_str()
        .ok_or_else(|| Outcome::Unsupported(format!("expected a string, got {value}")))
}

pub fn int_arg(value: &Value) -> Result<i64, Outcome> {
    value
        .as_i64()
        .ok_or_else(|| Outcome::Unsupported(format!("expected an integer, got {value}")))
}

pub fn bool_arg(value: &Value) -> Result<bool, Outcome> {
    value
        .as_bool()
        .ok_or_else(|| Outcome::Unsupported(format!("expected a boolean, got {value}")))
}
