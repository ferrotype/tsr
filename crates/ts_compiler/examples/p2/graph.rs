//! Portable graph identities only; all graph reads go through public owners.
use super::{file_name, hex, node_json, source, utf8, view, Error, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::ControlFlow;
use ts_arena::{NodeId, SymbolId};
use ts_ast::{AstView, ChildVisitor, NodeListId, NodeSlice, SymbolTableId};
use ts_checker::Operation;
use ts_compiler::Program;

pub fn children(program: &Program, node: NodeId) -> Result<Vec<NodeId>> {
    struct Visitor<'a> {
        view: AstView<'a>,
        nodes: Vec<NodeId>,
        error: Option<ts_arena::Error>,
    }
    impl ChildVisitor for Visitor<'_> {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            self.nodes.push(node);
            ControlFlow::Continue(())
        }
        fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
            match self.view.list(list) {
                Ok(list) => self.visit_node_slice(list.nodes()),
                Err(error) => {
                    self.error = Some(error);
                    ControlFlow::Break(())
                }
            }
        }
        fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
            match self.view.node_slice(nodes) {
                Ok(nodes) => {
                    self.nodes.extend(nodes.iter().flatten());
                    ControlFlow::Continue(())
                }
                Err(error) => {
                    self.error = Some(error);
                    ControlFlow::Break(())
                }
            }
        }
    }
    let ast = view(program, node)?;
    let mut visitor = Visitor {
        view: ast,
        nodes: Vec::new(),
        error: None,
    };
    let _ = ast.node(node)?.for_each_child(&mut visitor);
    if let Some(error) = visitor.error {
        return Err(error.into());
    }
    Ok(visitor.nodes)
}
pub fn walk(program: &Program, node: NodeId) -> Result<Vec<NodeId>> {
    let mut out = vec![node];
    for child in children(program, node)? {
        out.extend(walk(program, child)?);
    }
    Ok(out)
}
fn source_symbol(program: &Program, id: SymbolId) -> Result<ts_ast::SymbolRef<'_>> {
    for file in program.files() {
        let view = file.bound().view();
        if view.result().symbols().id() == id.arena() {
            return Ok(ts_ast::SymbolRef::Stored(view.symbol(id)?));
        }
    }
    Err(Error::Ast(ts_arena::Error::WrongOwner))
}
fn symbol<'a>(
    program: &'a Program,
    op: Option<&'a Operation<'_>>,
    id: SymbolId,
) -> Result<ts_ast::SymbolRef<'a>> {
    match op {
        Some(op) => Ok(op.symbol(op.symbol_ref(id)?)?),
        None => source_symbol(program, id),
    }
}
// Match the oracle's lexical class identity, not either binder's runtime ID.
fn portable_name(
    program: &Program,
    op: Option<&Operation<'_>>,
    name: &[u8],
    id: Option<SymbolId>,
) -> Result<Vec<u8>> {
    if name.starts_with(b"\xfe@") {
        let suffix = name
            .iter()
            .rposition(|b| *b == b'@')
            .expect("prefix contains @");
        // Well-known names without a numeric suffix are already portable.
        if suffix > 1 {
            if suffix + 1 == name.len() || !name[suffix + 1..].iter().all(u8::is_ascii_digit) {
                return Err(Error::Protocol(
                    "unique name has invalid runtime identity".into(),
                ));
            }
            let op = op.ok_or_else(|| Error::Protocol("unique name has no checker".into()))?;
            let id = id.ok_or_else(|| Error::Protocol("unique name has no symbol".into()))?;
            let ty = op.symbol_name_type(op.symbol_ref(id)?)?.ok_or_else(|| {
                Error::Protocol("unique name has no existing nameType identity".into())
            })?;
            if op.type_flags(ty)? & ts_checker::type_flags::UNIQUE_ES_SYMBOL == 0 {
                return Err(Error::Protocol(
                    "unique nameType is not unique symbol".into(),
                ));
            }
            let key = op
                .type_symbol(ty)?
                .ok_or_else(|| Error::Protocol("unique nameType has no symbol".into()))?;
            let symbol = symbol(program, Some(op), key)?;
            let declaration = symbol
                .value_declaration()
                .or(declarations(program, Some(op), key)?
                    .into_iter()
                    .flatten()
                    .next())
                .ok_or_else(|| {
                    Error::Protocol("unique symbol has no defining declaration".into())
                })?;
            let read = view(program, declaration)?.node(declaration)?;
            let mut result = name[..suffix].to_vec();
            result.extend_from_slice(
                format!(
                    "@symbol:{}:{}:{}:{}",
                    hex(file_name(program, declaration)?),
                    read.kind().raw(),
                    read.pos(),
                    read.end()
                )
                .as_bytes(),
            );
            return Ok(result);
        }
    }
    if !name.starts_with(b"\xfe#") {
        return Ok(name.to_vec());
    }
    let digits = name[2..].iter().take_while(|b| b.is_ascii_digit()).count();
    let suffix = 2 + digits;
    let id = id.ok_or_else(|| Error::Protocol("private name has no symbol".into()))?;
    if digits == 0 || name.get(suffix) != Some(&b'@') {
        return Err(Error::Protocol(
            "private name has no runtime class identity".into(),
        ));
    }
    let s = symbol(program, op, id)?;
    let mut declaration = s.value_declaration();
    if declaration.is_none() {
        declaration = declarations(program, op, id)?.into_iter().flatten().next();
    }
    while let Some(node) = declaration {
        let read = view(program, node)?.node(node)?;
        if matches!(
            read.kind().known(),
            Some(ts_ast::SyntaxKind::ClassDeclaration | ts_ast::SyntaxKind::ClassExpression)
        ) {
            let mut result = vec![0xfe];
            result.extend_from_slice(
                format!(
                    "#class:{}:{}:{}:{}",
                    hex(file_name(program, node)?),
                    read.kind().raw(),
                    read.pos(),
                    read.end()
                )
                .as_bytes(),
            );
            result.extend_from_slice(&name[suffix..]);
            return Ok(result);
        }
        declaration = read.parent();
    }
    Err(Error::Protocol(
        "private name has no declaring class".into(),
    ))
}
fn table(
    program: &Program,
    op: Option<&Operation<'_>>,
    id: SymbolTableId,
) -> Result<Vec<(Vec<u8>, Option<SymbolId>)>> {
    let read = if let Some(op) = op {
        op.symbol_table(id)?
    } else {
        program
            .files()
            .iter()
            .find(|f| f.bound().view().result().tables().id() == id.arena())
            .ok_or(Error::Ast(ts_arena::Error::WrongOwner))?
            .bound()
            .view()
            .result()
            .tables()
            .get(id)?
    };
    let mut entries = read
        .iter()
        .map(|(name, id)| Ok((portable_name(program, op, name, id)?, id)))
        .collect::<Result<Vec<_>>>()?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}
fn declarations(
    program: &Program,
    op: Option<&Operation<'_>>,
    id: SymbolId,
) -> Result<Vec<Option<NodeId>>> {
    if let Some(op) = op {
        return Ok(op.symbol_declarations(op.symbol_ref(id)?)?.iter().collect());
    }
    let file = program
        .files()
        .iter()
        .find(|f| f.bound().view().result().symbols().id() == id.arena())
        .ok_or(Error::Ast(ts_arena::Error::WrongOwner))?;
    let view = file.bound().view();
    Ok(view
        .result()
        .declarations()
        .get(view.symbol(id)?.declarations())?
        .iter()
        .collect())
}
pub struct Graph {
    ids: HashMap<SymbolId, String>,
    owner: String,
    next: usize,
}
impl Graph {
    pub fn new(program: &Program, owner: &str, only: Option<NodeId>) -> Result<Self> {
        let mut graph = Self {
            ids: HashMap::new(),
            owner: owner.into(),
            next: 0,
        };
        let mut files: Vec<_> = program
            .files()
            .iter()
            .filter(|f| only.is_none_or(|id| id == f.source()))
            .collect();
        files.sort_by_key(|f| file_name(program, f.source()).expect("retained file"));
        for file in files {
            let mut next = 0;
            let label = utf8(file_name(program, file.source())?)?;
            for node in walk(program, file.source())? {
                let binding = file.bound().view().node_binding(node)?.unwrap_or_default();
                graph.seed(program, binding.symbol, label, &mut next)?;
                graph.seed(program, binding.local_symbol, label, &mut next)?;
                if let Some(locals) = binding.locals {
                    for (_, symbol) in table(program, None, locals)? {
                        graph.seed(program, symbol, label, &mut next)?;
                    }
                }
            }
        }
        Ok(graph)
    }
    fn seed(
        &mut self,
        program: &Program,
        id: Option<SymbolId>,
        file: &str,
        next: &mut usize,
    ) -> Result<()> {
        let Some(id) = id else { return Ok(()) };
        if self.ids.contains_key(&id) {
            return Ok(());
        }
        self.ids.insert(id, format!("source:{file}:{next}"));
        *next += 1;
        let s = source_symbol(program, id)?;
        for t in [s.members(), s.exports()].into_iter().flatten() {
            for (_, value) in table(program, None, t)? {
                self.seed(program, value, file, next)?;
            }
        }
        self.seed(program, s.parent(), file, next)?;
        self.seed(program, s.export_symbol(), file, next)
    }
    fn id(&mut self, id: Option<SymbolId>) -> Value {
        let Some(id) = id else { return Value::Null };
        let label = self.ids.entry(id).or_insert_with(|| {
            let label = format!("checker:{}:{}", self.owner, self.next);
            self.next += 1;
            label
        });
        json!(label)
    }
    fn table_json(
        &mut self,
        program: &Program,
        op: Option<&Operation<'_>>,
        id: Option<SymbolTableId>,
        queue: &mut VecDeque<SymbolId>,
    ) -> Result<Value> {
        let Some(id) = id else { return Ok(Value::Null) };
        let mut out = Vec::new();
        for (name, value) in table(program, op, id)? {
            if let Some(value) = value {
                queue.push_back(value);
            }
            out.push(json!({"name_hex":hex(&name),"symbol":self.id(value)}));
        }
        Ok(json!(out))
    }
    pub fn snapshot(
        &mut self,
        program: &Program,
        op: Option<&Operation<'_>>,
        root: Option<SymbolId>,
    ) -> Result<Value> {
        let Some(root) = root else {
            return Ok(Value::Null);
        };
        let root_id = self.id(Some(root));
        let mut queue = VecDeque::from([root]);
        let mut seen = HashSet::new();
        let mut records = Vec::new();
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id) {
                continue;
            }
            let s = symbol(program, op, id)?;
            let ds = if s.declarations().is_nil() {
                Value::Null
            } else {
                json!(declarations(program, op, id)?
                    .into_iter()
                    .map(|id| node_json(program, id))
                    .collect::<Result<Vec<_>>>()?)
            };
            let members = self.table_json(program, op, s.members(), &mut queue)?;
            let exports = self.table_json(program, op, s.exports(), &mut queue)?;
            let parent = self.id(s.parent());
            if let Some(id) = s.parent() {
                queue.push_back(id);
            }
            let export_symbol = self.id(s.export_symbol());
            if let Some(id) = s.export_symbol() {
                queue.push_back(id);
            }
            records.push(json!({"id":self.id(Some(id)),"name_hex":hex(&portable_name(program,op,s.name_bytes(),Some(id))?),"flags":s.flags(),"check_flags":s.check_flags(),"declarations":ds,"value_declaration":node_json(program,s.value_declaration())?,"members":members,"exports":exports,"parent":parent,"export_symbol":export_symbol}));
        }
        Ok(json!({"root":root_id,"records":records}))
    }
}
pub fn bound_snapshot(program: &Program, root: NodeId) -> Result<Value> {
    let file = source(program, root)?;
    let mut graph = Graph::new(program, "bound", Some(root))?;
    let mut nodes = Vec::new();
    for id in walk(program, root)? {
        let binding = file.view().node_binding(id)?.unwrap_or_default();
        let children = children(program, id)?
            .into_iter()
            .map(|id| node_json(program, Some(id)))
            .collect::<Result<Vec<_>>>()?;
        nodes.push(json!({"node":node_json(program,Some(id))?,"flags":file.view().node(id)?.flags(),"symbol":graph.snapshot(program,None,binding.symbol)?,"local_symbol":graph.snapshot(program,None,binding.local_symbol)?,"children":children}));
    }
    let mut locals = Vec::new();
    if let Some(id) = file.view().node_binding(root)?.and_then(|b| b.locals) {
        for (name, symbol) in table(program, None, id)? {
            locals
                .push(json!({"name_hex":hex(&name),"symbol":graph.snapshot(program,None,symbol)?}));
        }
    }
    Ok(
        json!({"file":utf8(file_name(program,root)?)?,"text_hex":hex(file.view().source_file()?.text().as_bytes()),"nodes":nodes,"locals":locals}),
    )
}
pub fn bound_global(program: &Program, file: &str, name: &str) -> Result<SymbolId> {
    let file = program
        .file(file.as_bytes())
        .ok_or(Error::Ast(ts_arena::Error::WrongOwner))?;
    let locals = file
        .bound()
        .view()
        .node_binding(file.source())?
        .and_then(|b| b.locals)
        .ok_or(Error::Ast(ts_arena::Error::InvalidGraph))?;
    file.bound()
        .view()
        .result()
        .tables()
        .get(locals)?
        .get(name.as_bytes())
        .flatten()
        .ok_or(Error::Ast(ts_arena::Error::InvalidGraph))
}
