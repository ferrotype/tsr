//! The pin's harness order for the error baseline: a second fresh program on
//! which `Program.Emit` runs before its diagnostics are collected
//! (`harnessutil.compileFilesWithHost`). No output is transformed or written;
//! the driver makes the emit resolver calls of the script transforms that
//! reach the checker before semantic checking — import elision, the runtime
//! syntax transform's enum member values and constant-enum inlining — with the
//! emitter's option guards, over the files `emitter.emitJSFile` transforms.
//! Declaration emit and the other transforms' resolver calls are not executed,
//! and the schedule says so. One effect of the downleveler that runs between
//! them is modeled because it decides which nodes constant-enum inlining asks
//! about: below ES2022 the class-fields transform replaces an access to a
//! private name an enclosing class declares with a helper call.
use crate::executor;
use serde_json::{json, Value};
use std::ops::ControlFlow;
use std::sync::Arc;
use tsr_arena::{Error as ArenaError, NodeId};
use tsr_ast::{
    modifier_flags as mf, AstView, ChildVisitor, NodeListId, NodeSlice, SyntaxKind as K,
};
use tsr_checker::{CheckerOwner, Operation};
use tsr_compiler::{FileCache, Program, ProgramFile};

pub const TRANSFORMS: [&str; 3] = [
    "import_elision",
    "runtime_syntax_enum_members",
    "const_enum_inlining",
];
pub const NOT_EXECUTED: [&str; 2] = ["declaration_emit", "other_script_transforms"];

/// The post-emit diagnostics of one request.
pub enum Post {
    /// No script transform runs (`noEmit`, or no file with JavaScript
    /// output), so the post-emit program repeats the pre-emit program's
    /// schedule and its diagnostics are the pre-emit ones.
    Pre { schedule: Value },
    Fresh {
        program: Arc<Program>,
        _checkers: Checkers,
        diagnostics: Vec<tsr_ast::Diagnostic>,
        schedule: Value,
    },
}

/// What keeps the post-emit program's checkers alive with its diagnostics;
/// held, never read.
#[allow(dead_code)]
pub enum Checkers {
    Owner(Arc<CheckerOwner>),
    Pool(tsr_compiler::CheckedProgram),
}

type Result<T> = std::result::Result<T, Value>;

fn arena(error: ArenaError) -> Value {
    executor::failure(format!("{error:?}"), "emit_schedule")
}

fn checker(error: tsr_checker::Error) -> Value {
    executor::checker_failure(error)
}

fn compiler(error: tsr_compiler::Error) -> Value {
    executor::compiler_failure(error)
}

fn identity(program: &str, reason: Option<&str>, files: &[Value], gate: &Value) -> Value {
    json!({"state":"executed","program":program,"reason":reason,"transforms":TRANSFORMS,
        "not_executed":NOT_EXECUTED,"no_emit_on_error":gate,"files":files})
}

pub fn run(request: &Value, cache: &mut FileCache, pre: &Program) -> Result<Post> {
    let options = pre.options();
    if options.no_emit.is_true() {
        return Ok(Post::Pre {
            schedule: identity("pre", Some("noEmit"), &[], &Value::Null),
        });
    }
    if !options.no_emit_on_error.is_true()
        && pre.javascript_emit_files().map_err(compiler)?.is_empty()
    {
        return Ok(Post::Pre {
            schedule: identity("pre", Some("no JavaScript output"), &[], &Value::Null),
        });
    }
    if request["mode"].is_string() {
        return run_checked(request, cache);
    }
    let fresh = executor::load_fresh(request, cache)?;
    let program = fresh.program.clone();
    let (diagnostics, schedule) = {
        let mut op = fresh.owner.operation().map_err(checker)?;
        let mut gate = Value::Null;
        let mut files = Vec::new();
        let mut skipped = false;
        if program.options().no_emit_on_error.is_true() {
            let found = executor::any_program_diagnostics(&program, &mut op)?;
            skipped = !found.is_empty();
            gate = json!({"diagnostics":found.len(),"emit_skipped":skipped});
        }
        if !skipped {
            for file in program.javascript_emit_files().map_err(compiler)? {
                files.push(schedule_file(&program, &mut op, file)?);
            }
        }
        let values =
            executor::harness_diagnostics(&program, &mut op, &request["diagnostic_phases"])?;
        let sorted = program
            .sort_and_deduplicate_diagnostics(&values)
            .map_err(compiler)?;
        (sorted, identity("fresh", None, &files, &gate))
    };
    Ok(Post::Fresh {
        program,
        _checkers: Checkers::Owner(fresh.owner),
        diagnostics,
        schedule,
    })
}

/// `run` through the post-emit program's checker pool: the `noEmitOnError`
/// gate with the pool's diagnostics, then each emitted file scheduled with
/// its own checker in the program's work group (`Program.Emit`), which a
/// single-threaded program runs last file first.
fn run_checked(request: &Value, cache: &mut FileCache) -> Result<Post> {
    let checked = executor::load_fresh_checked(request, cache)?;
    let program = checked.program().clone();
    let mut gate = Value::Null;
    let mut skipped = false;
    if program.options().no_emit_on_error.is_true() {
        let found = executor::any_program_diagnostics_checked(&checked)?;
        skipped = !found.is_empty();
        gate = json!({"diagnostics":found.len(),"emit_skipped":skipped});
    }
    let mut files = Vec::new();
    if !skipped {
        let emitted = program.javascript_emit_files().map_err(compiler)?;
        let results: Vec<std::sync::Mutex<Option<Result<Value>>>> = emitted
            .iter()
            .map(|_| std::sync::Mutex::new(None))
            .collect();
        let group = tsr_core::workgroup::WorkGroup::new(program.single_threaded());
        for (file, slot) in emitted.iter().zip(&results) {
            let (checked, program) = (&checked, &program);
            group.queue(move || {
                let mut result = None;
                let request = tsr_checker::CheckerRequest::default();
                let served =
                    checked.with_type_checker_for_file(&request, file.source(), &mut |op| {
                        result = Some(schedule_file(program, op, file));
                        Ok(())
                    });
                *slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(
                    served
                        .map_err(checker)
                        .and_then(|()| result.expect("the task ran")),
                );
            });
        }
        group.run_and_wait();
        drop(group);
        for slot in results {
            files.push(
                slot.into_inner()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .expect("every queued schedule ran")?,
            );
        }
    }
    let values = executor::harness_diagnostics_checked(&checked, &request["diagnostic_phases"])?;
    let diagnostics = program
        .sort_and_deduplicate_diagnostics(&values)
        .map_err(compiler)?;
    Ok(Post::Fresh {
        program,
        _checkers: Checkers::Pool(checked),
        diagnostics,
        schedule: identity("fresh", None, &files, &gate),
    })
}

#[derive(Default)]
struct Calls {
    mark_linked_references_recursively: usize,
    is_referenced_alias_declaration: usize,
    is_value_alias_declaration: usize,
    is_top_level_value_import_equals_with_entity_name: usize,
    enum_member_value: usize,
    constant_value: usize,
}

struct Schedule<'a, 'op> {
    view: AstView<'a>,
    op: &'a mut Operation<'op>,
    preserve_const_enums: bool,
    experimental_decorators: bool,
    external_module: bool,
    lowers_private_names: bool,
    calls: Calls,
}

fn schedule_file(program: &Program, op: &mut Operation<'_>, file: &ProgramFile) -> Result<Value> {
    let source = file.bound().view().source_file().map_err(arena)?;
    let view = file.bound().view().ast();
    let root = view
        .file_info()
        .root
        .ok_or_else(|| arena(ArenaError::WrongOwner))?;
    let options = program.options();
    // emitter.go:getScriptTransformers guards.
    let import_elision = !options.verbatim_module_syntax.is_true()
        && !tsr_ast::utilities::is_in_js_file(Some(&view.node(root).map_err(arena)?));
    let const_enum_inlining = !options.isolated_modules();
    let mut schedule = Schedule {
        view,
        op,
        preserve_const_enums: options.should_preserve_const_enums(),
        experimental_decorators: options.experimental_decorators.is_true(),
        external_module: tsr_ast::utilities::is_external_module(&source),
        // classfields.go:newClassFieldsTransformer
        lowers_private_names: options.emit_script_target() < tsr_core::ScriptTarget::ES2022,
        calls: Calls::default(),
    };
    if import_elision {
        schedule.elide_imports(root)?;
    }
    schedule.enum_members(root)?;
    if const_enum_inlining {
        schedule.inline_const_enums(root)?;
    }
    let c = &schedule.calls;
    Ok(json!({
        "file_hex": executor::diagnostics::hex(source.parse_options().file_name.as_bytes()),
        "import_elision": import_elision,
        "const_enum_inlining": const_enum_inlining,
        "calls": {
            "mark_linked_references_recursively": c.mark_linked_references_recursively,
            "is_referenced_alias_declaration": c.is_referenced_alias_declaration,
            "is_value_alias_declaration": c.is_value_alias_declaration,
            "is_top_level_value_import_equals_with_entity_name":
                c.is_top_level_value_import_equals_with_entity_name,
            "enum_member_value": c.enum_member_value,
            "constant_value": c.constant_value,
        },
    }))
}

struct Children<'a> {
    view: AstView<'a>,
    nodes: Vec<NodeId>,
    error: Option<ArenaError>,
}
impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, id: NodeId) -> ControlFlow<()> {
        self.nodes.push(id);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, id: NodeListId) -> ControlFlow<()> {
        match self.view.list(id) {
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

impl Schedule<'_, '_> {
    fn children(&self, node: NodeId) -> Result<Vec<NodeId>> {
        let mut children = Children {
            view: self.view,
            nodes: Vec::new(),
            error: None,
        };
        let _ = self
            .view
            .node(node)
            .map_err(arena)?
            .for_each_child(&mut children);
        match children.error {
            Some(error) => Err(arena(error)),
            None => Ok(children.nodes),
        }
    }

    fn list(&self, list: Option<NodeListId>) -> Result<Vec<NodeId>> {
        let Some(list) = list else {
            return Ok(Vec::new());
        };
        let list = self.view.list(list).map_err(arena)?;
        Ok(self
            .view
            .node_slice(list.nodes())
            .map_err(arena)?
            .iter()
            .flatten()
            .collect())
    }

    fn modifier(&self, node: NodeId, flags: u32) -> Result<bool> {
        tsr_ast::utilities::has_syntactic_modifier(self.view, node, flags).map_err(arena)
    }

    fn body_missing(&self, node: NodeId) -> Result<bool> {
        let body = self.view.node(node).map_err(arena)?.body();
        Ok(match body {
            None => true,
            Some(body) => tsr_ast::node_is_missing(Some(&self.view.node(body).map_err(arena)?)),
        })
    }

    fn is_import_clause_type_only(&self, clause: NodeId) -> Result<bool> {
        let read = self.view.node(clause).map_err(arena)?;
        Ok(read
            .data_source()
            .as_import_clause()
            .is_some_and(|clause| clause.phase_modifier() == K::TypeKeyword))
    }

    /// The import or export specifiers of a named import or export list that
    /// the type eraser keeps, or `None` when it keeps the empty list itself.
    fn surviving_specifiers(&self, list: NodeId) -> Result<Option<Vec<NodeId>>> {
        let elements = self.children(list)?;
        if elements.is_empty() {
            return Ok(None);
        }
        let mut kept = Vec::new();
        for element in elements {
            let read = self.view.node(element).map_err(arena)?;
            let data = read.data_source();
            let type_only = data
                .as_import_specifier()
                .map(|specifier| specifier.is_type_only())
                .or_else(|| {
                    data.as_export_specifier()
                        .map(|specifier| specifier.is_type_only())
                })
                .unwrap_or(false);
            if !type_only {
                kept.push(element);
            }
        }
        Ok(Some(kept))
    }

    /// Whether the class-fields transform replaces the access `node` with a
    /// private-field helper call before constant-enum inlining sees it: its
    /// name is a private name that an enclosing class declares, and private
    /// elements are lowered (`classfields.go:visitPropertyAccessExpression`,
    /// `accessPrivateIdentifier`). The inliner then visits only the receiver.
    fn private_access_lowered(&self, node: NodeId) -> Result<bool> {
        if !self.lowers_private_names {
            return Ok(false);
        }
        let read = self.view.node(node).map_err(arena)?;
        let Some(name) = read.name() else {
            return Ok(false);
        };
        if read.kind() != K::PropertyAccessExpression
            || self.view.node(name).map_err(arena)?.kind() != K::PrivateIdentifier
        {
            return Ok(false);
        }
        let text = self.view.node_text(name).map_err(arena)?;
        let mut ancestor = read.parent();
        while let Some(current) = ancestor {
            let current_read = self.view.node(current).map_err(arena)?;
            if tsr_ast::utilities::is_class_like(&current_read) {
                for member in self.list(current_read.member_list())? {
                    let Some(member_name) = self.view.node(member).map_err(arena)?.name() else {
                        continue;
                    };
                    if self.view.node(member_name).map_err(arena)?.kind() == K::PrivateIdentifier
                        && self.view.node_text(member_name).map_err(arena)?.as_bytes()
                            == text.as_bytes()
                    {
                        return Ok(true);
                    }
                }
            }
            ancestor = current_read.parent();
        }
        Ok(false)
    }

    /// Whether the type eraser, or the runtime syntax transform for a constant
    /// enum, removes `node` from the emitted tree (`typeeraser.go:visit`).
    fn erased(&self, node: NodeId) -> Result<bool> {
        let read = self.view.node(node).map_err(arena)?;
        let kind = read.kind();
        if tsr_ast::utilities::is_statement(self.view, node).map_err(arena)?
            && self.modifier(node, mf::AMBIENT)?
        {
            return Ok(true);
        }
        if kind != K::ExpressionWithTypeArguments && tsr_ast::utilities::is_type_node_kind(kind) {
            return Ok(true);
        }
        Ok(match kind.known() {
            Some(
                K::IndexSignature
                | K::TypeParameter
                | K::JSImportDeclaration
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration
                | K::InterfaceDeclaration
                | K::NamespaceExportDeclaration,
            ) => true,
            Some(K::ModuleDeclaration) => {
                let name = read
                    .name()
                    .map(|name| self.view.node(name))
                    .transpose()
                    .map_err(arena)?;
                // The innermost module of a dotted name needs a body.
                let mut module = node;
                let bodyless = loop {
                    let Some(body) = self.view.node(module).map_err(arena)?.body() else {
                        break true;
                    };
                    if self.view.node(body).map_err(arena)?.kind() != K::ModuleDeclaration {
                        break false;
                    }
                    module = body;
                };
                name.is_none_or(|name| name.kind() != K::Identifier)
                    || !tsr_ast::is_instantiated_module(self.view, node, self.preserve_const_enums)
                        .map_err(arena)?
                    || bodyless
            }
            Some(K::PropertyDeclaration) => {
                self.modifier(node, mf::AMBIENT | mf::ABSTRACT)?
                    && !(self.experimental_decorators
                        && tsr_ast::utilities_middle::has_decorators(self.view, &read)
                            .map_err(arena)?)
            }
            Some(K::Constructor | K::MethodDeclaration | K::FunctionDeclaration) => {
                self.body_missing(node)?
            }
            Some(K::GetAccessor | K::SetAccessor) => {
                self.body_missing(node)? && self.modifier(node, mf::ABSTRACT)?
            }
            Some(K::HeritageClause) => read
                .data_source()
                .as_heritage_clause()
                .is_some_and(|clause| clause.token() == K::ImplementsKeyword),
            Some(K::Parameter) => {
                tsr_ast::utilities_class::is_this_parameter(self.view, node).map_err(arena)?
            }
            Some(K::ImportEqualsDeclaration) => read
                .data_source()
                .as_import_equals_declaration()
                .is_some_and(|declaration| declaration.is_type_only()),
            Some(K::ImportDeclaration) => match read.import_clause() {
                None => false,
                Some(clause) => {
                    if self.is_import_clause_type_only(clause)? {
                        true
                    } else {
                        let clause_read = self.view.node(clause).map_err(arena)?;
                        let bindings = clause_read
                            .data_source()
                            .as_import_clause()
                            .and_then(|clause| clause.named_bindings());
                        let bindings_kept = match bindings {
                            None => false,
                            Some(bindings) => {
                                self.view.node(bindings).map_err(arena)?.kind() != K::NamedImports
                                    || self
                                        .surviving_specifiers(bindings)?
                                        .is_none_or(|kept| !kept.is_empty())
                            }
                        };
                        clause_read.name().is_none() && !bindings_kept
                    }
                }
            },
            Some(K::ExportDeclaration) => {
                let data = read.data_source();
                let declaration = data.as_export_declaration();
                if declaration
                    .as_ref()
                    .is_some_and(tsr_ast::ExportDeclarationDataRead::is_type_only)
                {
                    true
                } else {
                    match declaration.and_then(|d| d.export_clause()) {
                        Some(clause)
                            if self.view.node(clause).map_err(arena)?.kind() == K::NamedExports =>
                        {
                            self.surviving_specifiers(clause)?
                                .is_some_and(|kept| kept.is_empty())
                        }
                        _ => false,
                    }
                }
            }
            Some(K::EnumDeclaration) => {
                tsr_ast::utilities::is_enum_const(self.view, node).map_err(arena)?
                    && !self.preserve_const_enums
            }
            _ => false,
        })
    }

    /// `importelision.go:ImportElisionTransformer.visit` over the statements
    /// the type eraser keeps.
    fn elide_imports(&mut self, root: NodeId) -> Result<()> {
        self.calls.mark_linked_references_recursively += 1;
        self.op
            .mark_linked_references_recursively(root)
            .map_err(checker)?;
        let statements = self.children(root)?;
        self.elide_statements(&statements)
    }

    fn referenced(&mut self, node: NodeId) -> Result<bool> {
        self.calls.is_referenced_alias_declaration += 1;
        self.op
            .is_referenced_alias_declaration(node)
            .map_err(checker)
    }

    fn value_alias(&mut self, node: NodeId) -> Result<bool> {
        self.calls.is_value_alias_declaration += 1;
        self.op.is_value_alias_declaration(node).map_err(checker)
    }

    fn elide_statements(&mut self, statements: &[NodeId]) -> Result<()> {
        for &statement in statements {
            if self.erased(statement)? {
                continue;
            }
            let read = self.view.node(statement).map_err(arena)?;
            match read.kind().known() {
                Some(K::ImportEqualsDeclaration) => {
                    if tsr_ast::utilities_modules::is_external_module_import_equals_declaration(
                        self.view, statement,
                    )
                    .map_err(arena)?
                    {
                        self.referenced(statement)?;
                    } else if !self.referenced(statement)? && !self.external_module {
                        self.calls.is_top_level_value_import_equals_with_entity_name += 1;
                        self.op
                            .is_top_level_value_import_equals_with_entity_name(statement)
                            .map_err(checker)?;
                    }
                }
                Some(K::ImportDeclaration) => {
                    let Some(clause) = read.import_clause() else {
                        continue;
                    };
                    self.referenced(clause)?;
                    let bindings = self
                        .view
                        .node(clause)
                        .map_err(arena)?
                        .data_source()
                        .as_import_clause()
                        .and_then(|clause| clause.named_bindings());
                    if let Some(bindings) = bindings {
                        if self.view.node(bindings).map_err(arena)?.kind() == K::NamedImports {
                            for specifier in
                                self.surviving_specifiers(bindings)?.unwrap_or_default()
                            {
                                self.referenced(specifier)?;
                            }
                        } else {
                            self.referenced(bindings)?;
                        }
                    }
                }
                Some(K::ExportAssignment) => {
                    self.value_alias(statement)?;
                }
                Some(K::ExportDeclaration) => {
                    let clause = read
                        .data_source()
                        .as_export_declaration()
                        .and_then(|declaration| declaration.export_clause());
                    if let Some(clause) = clause {
                        if self.view.node(clause).map_err(arena)?.kind() == K::NamedExports {
                            for specifier in self.surviving_specifiers(clause)?.unwrap_or_default()
                            {
                                self.value_alias(specifier)?;
                            }
                        }
                    }
                }
                Some(K::ModuleDeclaration) => {
                    let mut body = read.body();
                    while let Some(inner) = body {
                        let inner_read = self.view.node(inner).map_err(arena)?;
                        match inner_read.kind().known() {
                            Some(K::ModuleDeclaration) => body = inner_read.body(),
                            Some(K::ModuleBlock) => {
                                let statements = self.list(inner_read.statement_list())?;
                                self.elide_statements(&statements)?;
                                break;
                            }
                            _ => break,
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// `runtimesyntax.go:transformEnumMember` asks each emitted enum member's
    /// value; a member with a constant value emits no initializer.
    fn enum_members(&mut self, root: NodeId) -> Result<()> {
        let mut work = vec![root];
        while let Some(node) = work.pop() {
            if self.erased(node)? {
                continue;
            }
            if self.view.node(node).map_err(arena)?.kind() == K::EnumDeclaration {
                for member in self.list(self.view.node(node).map_err(arena)?.member_list())? {
                    self.calls.enum_member_value += 1;
                    self.op.constant_value(member).map_err(checker)?;
                }
            }
            let mut children = self.children(node)?;
            children.reverse();
            work.extend(children);
        }
        Ok(())
    }

    /// `constenum.go:ConstEnumInliningTransformer.visit`, pre-order over the
    /// type-erased tree: an access with a constant value is replaced, so its
    /// operands are not visited.
    fn inline_const_enums(&mut self, root: NodeId) -> Result<()> {
        let mut work = vec![root];
        while let Some(node) = work.pop() {
            if self.erased(node)? {
                continue;
            }
            let read = self.view.node(node).map_err(arena)?;
            let kind = read.kind();
            if matches!(
                kind.known(),
                Some(K::PropertyAccessExpression | K::ElementAccessExpression)
            ) && !self.private_access_lowered(node)?
            {
                self.calls.constant_value += 1;
                if self.op.constant_value(node).map_err(checker)?.is_some() {
                    continue;
                }
            }
            let mut children = if kind == K::EnumMember {
                // The member's initializer is emitted only without a constant value.
                match read.initializer() {
                    Some(initializer)
                        if self.op.constant_value(node).map_err(checker)?.is_none() =>
                    {
                        vec![initializer]
                    }
                    _ => Vec::new(),
                }
            } else {
                self.children(node)?
            };
            children.reverse();
            work.extend(children);
        }
        Ok(())
    }
}
