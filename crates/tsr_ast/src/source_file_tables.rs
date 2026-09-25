//! Source-file tables: name tables, declaration maps and per-file data.
//!
//! Ports of `tsc/internal/ast/ast.go`, witnessed by the `containers` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::bind_result::BoundView;
use crate::{modifier_flags, AstView, JsDocProvider, NodeId, SyntaxKind as K};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tsr_arena::Error;

/// Go's `sourceFileDataKeyCounter`.
static SOURCE_FILE_DATA_KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Go's `SourceFileDataKey[T]`: an identity for one kind of per-file value.
/// Go's type parameter checks the value type statically; here the cell's
/// downcast checks it, and a key keeps one value type. Zero is Go's zero key,
/// which `getSourceFileDataCell` refuses.
pub type SourceFileDataKey = u64;

/// port: tsc/internal/ast/ast.go:NewSourceFileDataKey
pub fn new_source_file_data_key() -> SourceFileDataKey {
    SOURCE_FILE_DATA_KEY_COUNTER.fetch_add(1, Ordering::Relaxed) + 1
}

type Cell = Arc<dyn Any + Send + Sync>;

/// Go's `SourceFile.data` and `dataMu`.
#[derive(Default)]
pub struct SourceFileData {
    cells: Mutex<HashMap<u64, Cell>>,
}

impl std::fmt::Debug for SourceFileData {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SourceFileData")
            .finish_non_exhaustive()
    }
}

fn typed_cell<T: Send + Sync + 'static>(cell: Cell) -> Arc<OnceLock<T>> {
    cell.downcast::<OnceLock<T>>()
        .expect("interface conversion: a SourceFileDataKey keeps one value type")
}

/// Go's `getSourceFileDataCell`. The port marker is on the existing-cell
/// test, a site the mutation splicer can negate (a cell has no replacement
/// value).
fn get_source_file_data_cell<T: Send + Sync + 'static>(
    data: &SourceFileData,
    key: Option<SourceFileDataKey>,
) -> Arc<OnceLock<T>> {
    let key = match key {
        Some(key) if key != 0 => key,
        _ => panic!("invalid SourceFileDataKey; use NewSourceFileDataKey"),
    };
    let mut cells = data
        .cells
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let existing = cells.get(&key).cloned();
    // port: tsc/internal/ast/ast.go:getSourceFileDataCell
    if existing.is_some() {
        if let Some(cell) = existing {
            return typed_cell(cell);
        }
    }
    let cell: Cell = Arc::new(OnceLock::<T>::new());
    cells.insert(key, cell.clone());
    typed_cell(cell)
}

/// Go's `GetOrComputeSourceFileData`; `data` is the file's
/// `SourceFileState::data`. The port marker is on the once test, a site the
/// mutation splicer can negate (a generic result has no replacement value);
/// the fallback after it runs only under that mutant, since an initialized
/// cell stays initialized.
pub fn get_or_compute_source_file_data<T: Clone + Send + Sync + 'static>(
    data: &SourceFileData,
    key: Option<SourceFileDataKey>,
    compute: impl FnOnce() -> T,
) -> T {
    let cell = get_source_file_data_cell::<T>(data, key);
    let mut compute = Some(compute);
    // port: tsc/internal/ast/ast.go:GetOrComputeSourceFileData
    if cell.get().is_none() {
        let compute = compute.take().expect("the value is computed once");
        let _ = cell.get_or_init(compute);
    }
    match cell.get() {
        Some(value) => value.clone(),
        None => compute.take().expect("the value is computed once")(),
    }
}

/// Go's `GetDeclarationName`: the name a declaration map files it under,
/// empty for none.
/// port: tsc/internal/ast/ast.go:GetDeclarationName
pub fn get_declaration_name(view: AstView<'_>, declaration: NodeId) -> Result<Vec<u8>, Error> {
    if let Some(name) =
        crate::binder_helpers::get_non_assigned_name_of_declaration(view, declaration)?
    {
        let read = view.node(name)?;
        if read.kind() == K::ComputedPropertyName {
            let expression = read
                .expression()
                .expect("runtime error: invalid memory address or nil pointer dereference");
            let expression_read = view.node(expression)?;
            if crate::utilities::is_string_or_numeric_literal_like(&expression_read) {
                return Ok(view.node_text(expression)?.as_bytes().to_vec());
            }
            if expression_read.kind() == K::PropertyAccessExpression {
                let member = expression_read
                    .name()
                    .expect("runtime error: invalid memory address or nil pointer dereference");
                return Ok(view.node_text(member)?.as_bytes().to_vec());
            }
        } else if crate::utilities::is_property_name(&read) {
            return Ok(view.node_text(name)?.as_bytes().to_vec());
        }
    }
    Ok(Vec::new())
}

/// Go's `SourceFile.GetDeclarationMap`, computed on each call: Go caches it
/// on the file, which no caller can observe.
/// port: tsc/internal/ast/ast.go:SourceFile.GetDeclarationMap
pub fn get_declaration_map(
    view: AstView<'_>,
    bound: Option<BoundView<'_>>,
    file: NodeId,
) -> Result<HashMap<Vec<u8>, Vec<NodeId>>, Error> {
    compute_declaration_map(view, bound, file)
}

/// A node's children in `ForEachChild` order.
pub(crate) fn children(view: AstView<'_>, node: NodeId) -> Result<Vec<NodeId>, Error> {
    struct Collect<'v> {
        view: AstView<'v>,
        nodes: Vec<NodeId>,
        error: Option<Error>,
    }
    impl crate::ChildVisitor for Collect<'_> {
        fn visit_node(&mut self, node: NodeId) -> std::ops::ControlFlow<()> {
            self.nodes.push(node);
            std::ops::ControlFlow::Continue(())
        }
        fn visit_list(&mut self, list: crate::NodeListId) -> std::ops::ControlFlow<()> {
            match self.view.list(list) {
                Ok(list) => self.visit_node_slice(list.nodes()),
                Err(error) => {
                    self.error = Some(error);
                    std::ops::ControlFlow::Break(())
                }
            }
        }
        fn visit_node_slice(&mut self, nodes: crate::NodeSlice) -> std::ops::ControlFlow<()> {
            match self.view.node_slice(nodes) {
                Ok(nodes) => {
                    self.nodes.extend(nodes.iter().flatten());
                    std::ops::ControlFlow::Continue(())
                }
                Err(error) => {
                    self.error = Some(error);
                    std::ops::ControlFlow::Break(())
                }
            }
        }
    }
    let mut collect = Collect {
        view,
        nodes: Vec::new(),
        error: None,
    };
    let _ = view.node(node)?.for_each_child(&mut collect);
    match collect.error {
        Some(error) => Err(error),
        None => Ok(collect.nodes),
    }
}

fn symbol_of(bound: Option<BoundView<'_>>, node: NodeId) -> Result<Option<crate::SymbolId>, Error> {
    let Some(bound) = bound else {
        return Ok(None);
    };
    Ok(bound.node_binding(node)?.and_then(|binding| binding.symbol))
}

/// port: tsc/internal/ast/ast.go:SourceFile.computeDeclarationMap
fn compute_declaration_map(
    view: AstView<'_>,
    bound: Option<BoundView<'_>>,
    file: NodeId,
) -> Result<HashMap<Vec<u8>, Vec<NodeId>>, Error> {
    struct Map<'v, 'b> {
        view: AstView<'v>,
        bound: Option<BoundView<'b>>,
        result: HashMap<Vec<u8>, Vec<NodeId>>,
    }
    impl Map<'_, '_> {
        fn add_declaration(&mut self, declaration: NodeId) -> Result<(), Error> {
            let name = get_declaration_name(self.view, declaration)?;
            if !name.is_empty() {
                self.result.entry(name).or_default().push(declaration);
            }
            Ok(())
        }
        fn visit_children(&mut self, node: NodeId) -> Result<(), Error> {
            for child in children(self.view, node)? {
                self.visit(child)?;
            }
            Ok(())
        }
        fn visit(&mut self, node: NodeId) -> Result<(), Error> {
            stacker::maybe_grow(64 * 1024, 1024 * 1024, || self.visit_worker(node))
        }
        fn visit_worker(&mut self, node: NodeId) -> Result<(), Error> {
            let view = self.view;
            let read = view.node(node)?;
            match read.kind().known() {
                Some(
                    K::FunctionDeclaration
                    | K::FunctionExpression
                    | K::MethodDeclaration
                    | K::MethodSignature,
                ) => {
                    let name = get_declaration_name(view, node)?;
                    if !name.is_empty() {
                        let last = self
                            .result
                            .get(&name)
                            .and_then(|declarations| declarations.last().copied());
                        let same_group = match last {
                            Some(last) => {
                                read.parent() == view.node(last)?.parent()
                                    && symbol_of(self.bound, node)? == symbol_of(self.bound, last)?
                            }
                            None => false,
                        };
                        if same_group {
                            let last = last.expect("same group has a last declaration");
                            if read.body().is_some() && view.node(last)?.body().is_none() {
                                *self
                                    .result
                                    .get_mut(&name)
                                    .and_then(|d| d.last_mut())
                                    .expect("last") = node;
                            }
                        } else {
                            self.result.entry(name).or_default().push(node);
                        }
                    }
                    self.visit_children(node)
                }
                Some(
                    K::ClassDeclaration
                    | K::ClassExpression
                    | K::InterfaceDeclaration
                    | K::TypeAliasDeclaration
                    | K::EnumDeclaration
                    | K::ModuleDeclaration
                    | K::ImportEqualsDeclaration
                    | K::ImportClause
                    | K::NamespaceImport
                    | K::GetAccessor
                    | K::SetAccessor
                    | K::TypeLiteral,
                ) => {
                    self.add_declaration(node)?;
                    self.visit_children(node)
                }
                Some(K::ImportSpecifier | K::ExportSpecifier) => {
                    if read.property_name().is_some() {
                        self.add_declaration(node)?;
                    }
                    Ok(())
                }
                Some(K::Parameter)
                    if !crate::utilities::has_syntactic_modifier(
                        view,
                        node,
                        modifier_flags::PARAMETER_PROPERTY_MODIFIER,
                    )? =>
                {
                    Ok(())
                }
                Some(K::Parameter | K::VariableDeclaration | K::BindingElement) => {
                    if let Some(name) = read.name() {
                        if crate::utilities::is_binding_pattern(&view.node(name)?) {
                            self.visit_children(name)?;
                        } else {
                            if let Some(initializer) = read.initializer() {
                                self.visit(initializer)?;
                            }
                            self.add_declaration(node)?;
                        }
                    }
                    Ok(())
                }
                Some(K::EnumMember | K::PropertyDeclaration | K::PropertySignature) => {
                    self.add_declaration(node)
                }
                Some(K::ExportDeclaration) => {
                    let clause = read
                        .data_source()
                        .as_export_declaration()
                        .ok_or(Error::InvalidGraph)?
                        .export_clause();
                    if let Some(clause) = clause {
                        let clause_read = view.node(clause)?;
                        if clause_read.kind() == K::NamedExports {
                            for element in view
                                .node_slice(clause_read.elements(view)?)?
                                .iter()
                                .flatten()
                            {
                                self.visit(element)?;
                            }
                        } else {
                            self.visit(clause_read.name().expect("namespace export name"))?;
                        }
                    }
                    Ok(())
                }
                Some(K::ImportDeclaration) => {
                    if let Some(clause) = read.import_clause() {
                        let clause_read = view.node(clause)?;
                        if let Some(name) = clause_read.name() {
                            self.add_declaration(name)?;
                        }
                        let bindings = clause_read
                            .data_source()
                            .as_import_clause()
                            .ok_or(Error::InvalidGraph)?
                            .named_bindings();
                        if let Some(bindings) = bindings {
                            let bindings_read = view.node(bindings)?;
                            if bindings_read.kind() == K::NamespaceImport {
                                self.add_declaration(bindings)?;
                            } else {
                                for element in view
                                    .node_slice(bindings_read.elements(view)?)?
                                    .iter()
                                    .flatten()
                                {
                                    self.visit(element)?;
                                }
                            }
                        }
                    }
                    Ok(())
                }
                Some(K::BinaryExpression) => {
                    use crate::binder_helpers::JSDeclarationKind as J;
                    if matches!(
                        crate::binder_helpers::get_assignment_declaration_kind(view, node)?,
                        J::ExportsProperty | J::ThisProperty | J::Property
                    ) {
                        self.add_declaration(node)?;
                    }
                    self.visit_children(node)
                }
                _ => self.visit_children(node),
            }
        }
    }
    let mut map = Map {
        view,
        bound,
        result: HashMap::new(),
    };
    map.visit_children(file)?;
    Ok(map.result)
}

/// Go's `SourceFile.GetNameTable`, computed on each call (Go caches it on the
/// file). `jsdoc` parses the file's lazy JSDoc as `Node.JSDoc(file)` does.
pub fn get_name_table(
    view: AstView<'_>,
    jsdoc: &mut dyn JsDocProvider,
    file: NodeId,
) -> Result<HashMap<Vec<u8>, i32>, Error> {
    fn walk(
        view: AstView<'_>,
        jsdoc: &mut dyn JsDocProvider,
        file: NodeId,
        node: NodeId,
        table: &mut HashMap<Vec<u8>, i32>,
    ) -> Result<(), Error> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            let read = view.node(node)?;
            let named = match read.kind().known() {
                Some(K::Identifier) => {
                    !crate::utilities_tail::is_tag_name(view, node)?
                        && !view.node_text(node)?.as_bytes().is_empty()
                }
                Some(K::PrivateIdentifier) => true,
                _ => {
                    crate::utilities::is_string_or_numeric_literal_like(&read)
                        && literal_is_name(view, node)?
                }
            };
            if named {
                let text = view.node_text(node)?.as_bytes().to_vec();
                let mut position = read.pos();
                // A repeated name has no single position. The port marker
                // is on this test, a site the mutation splicer can negate (a
                // table has no replacement value).
                // port: tsc/internal/ast/ast.go:SourceFile.GetNameTable
                if table.contains_key(&text) {
                    position = -1;
                }
                table.insert(text, position);
            }
            for child in children(view, node)? {
                walk(view, jsdoc, file, child, table)?;
            }
            for doc in jsdoc.jsdoc(view, file, node)?.to_vec() {
                for child in children(view, doc)? {
                    walk(view, jsdoc, file, child, table)?;
                }
            }
            Ok(())
        })
    }
    let mut table = HashMap::new();
    for child in children(view, file)? {
        walk(view, jsdoc, file, child, &mut table)?;
    }
    Ok(table)
}

/// Go's `SourceFile.HasIdentifier`, collecting the identifiers on each call
/// (Go collects them once per file).
/// port: tsc/internal/ast/ast.go:SourceFile.HasIdentifier
pub fn has_identifier(view: AstView<'_>, file: NodeId, name: &[u8]) -> Result<bool, Error> {
    let identifiers: HashSet<Vec<u8>> = collect_identifiers_for_source_file(view, file)?
        .into_iter()
        .collect();
    Ok(identifiers.contains(name))
}

/// Go's set, as the texts in walk order (a caller only asks membership).
/// port: tsc/internal/ast/ast.go:collectIdentifiersForSourceFile
fn collect_identifiers_for_source_file(
    view: AstView<'_>,
    file: NodeId,
) -> Result<Vec<Vec<u8>>, Error> {
    fn collect(
        view: AstView<'_>,
        node: NodeId,
        identifiers: &mut Vec<Vec<u8>>,
    ) -> Result<(), Error> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            if matches!(
                view.node(node)?.kind().known(),
                Some(
                    K::Identifier
                        | K::PrivateIdentifier
                        | K::StringLiteral
                        | K::NumericLiteral
                        | K::BigIntLiteral
                        | K::NoSubstitutionTemplateLiteral
                )
            ) {
                identifiers.push(view.node_text(node)?.as_bytes().to_vec());
            }
            for child in children(view, node)? {
                collect(view, child, identifiers)?;
            }
            Ok(())
        })
    }
    let mut identifiers = Vec::new();
    collect(view, file, &mut identifiers)?;
    Ok(identifiers)
}

/// port: tsc/internal/ast/utilities.go:literalIsName
fn literal_is_name(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let parent = view
        .node(node)?
        .parent()
        .expect("runtime error: invalid memory address or nil pointer dereference");
    Ok(crate::utilities_positions::is_declaration_name(view, node)?
        || view.node(parent)?.kind() == K::ExternalModuleReference
        || crate::utilities_tail::is_argument_of_element_access_expression(view, Some(node))?
        || crate::utilities_positions::is_literal_computed_property_declaration_name(view, node)?)
}
