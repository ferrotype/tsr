//! The JSX transform (`transformers/jsxtransforms/jsx.go`): JSX elements and
//! fragments become calls of the classic factory (`React.createElement` or the
//! resolver's factory entity) or of the automatic runtime (`jsx`, `jsxs`,
//! `jsxDEV`, `Fragment`) imported from the file's implicit import source.
//!
//! Upstream reads `ast` predicates of the visited nodes through the node
//! itself. Here the transformer's factory is a `dyn RuntimeFactory`, which
//! offers no syntax view; the three predicates that need one
//! (`Node.SubtreeFacts`, `GetSemanticJsxChildren`, `IsPrologueDirective`) are
//! read through the factory by private helpers marked `TODO(ast_view)`.
use crate::transformer::{SharedEmitResolver, TransformOptions, Transformer};
use crate::Failure;
use std::cell::{Cell, RefCell};
use std::ops::ControlFlow;
use std::rc::Rc;
use std::sync::Arc;
use tsr_ast::utilities_middle::{
    get_pragma_argument, get_pragma_from_source_file, is_whitespace_only_jsx_text,
};
use tsr_ast::{
    node_flags, token_flags, ChildVisitor, Factory, FactoryMethods, JsString, NodeData, NodeId,
    NodeKind, NodeListId, NodeSlice, NodeVisitor, RuntimeFactory, SyntaxKind as K,
};
use tsr_core::{CompilerOptions, JsxEmit, LanguageVariant, ScriptTarget, TextRange};
use tsr_jsstring::classify::{is_digit, is_hex_digit, is_line_break, is_white_space_single_line};
use tsr_jsstring::wtf8::{decode_utf8, encode_rune};
use tsr_printer::{emit_flags, generated_identifier_flags as g, AutoGenerateOptions, EmitContext};

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// The import specifiers of one runtime import source, by imported name.
type ImportSpecifiers = Vec<(JsString, NodeId)>;

/// Upstream's `JSXTransformer`. The per-file fields are reset by
/// `visitSourceFile`; none is borrowed across a visit.
struct JsxTransformer<'a> {
    compiler_options: Arc<CompilerOptions>,
    emit_resolver: SharedEmitResolver<'a>,
    context: EmitContext,
    failure: Failure,

    import_specifier: RefCell<JsString>,
    filename_declaration: Cell<Option<NodeId>>,
    /// Upstream's `collections.OrderedMap[string, map[string]*ast.Node]`:
    /// import sources in insertion order, each with its specifiers by name.
    utilized_implicit_runtime_imports: RefCell<Vec<(JsString, ImportSpecifiers)>>,
    in_jsx_child: Cell<bool>,

    current_source_file: Cell<Option<NodeId>>,
}

// port: tsc/internal/transformers/jsxtransforms/jsx.go:NewJSXTransformer
pub fn new_jsx_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let compiler_options = opts.compiler_options.clone();
    let emit_context = opts.context.clone();
    let tx = Rc::new(JsxTransformer {
        compiler_options,
        emit_resolver: opts.emit_resolver.clone(),
        context: emit_context.clone(),
        failure: opts.failure.clone(),
        import_specifier: RefCell::default(),
        filename_declaration: Cell::new(None),
        utilized_implicit_runtime_imports: RefCell::default(),
        in_jsx_child: Cell::new(false),
        current_source_file: Cell::new(None),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(emit_context),
        opts.failure.clone(),
    ))
}

/// `NodeFactory.NewNodeList`: an undefined location.
fn new_node_list(factory: &mut dyn RuntimeFactory, nodes: Vec<NodeId>) -> NodeListId {
    let nodes = factory.alloc_nodes(nodes.into_iter().map(Some).collect());
    factory.alloc_list(TextRange::new(-1, -1), nodes)
}

/// The nodes of a slice; a nil element is upstream's nil dereference.
fn slice_nodes(factory: &dyn RuntimeFactory, nodes: NodeSlice) -> Vec<NodeId> {
    factory
        .read_nodes(nodes)
        .iter()
        .map(|node| node.expect(NIL))
        .collect()
}

/// `NodeList.Nodes`; a nil list has none.
fn list_nodes(factory: &dyn RuntimeFactory, list: Option<NodeListId>) -> Vec<NodeId> {
    match list {
        Some(list) => slice_nodes(factory, factory.read_list(list).nodes()),
        None => Vec::new(),
    }
}

fn kind(factory: &dyn RuntimeFactory, node: NodeId) -> NodeKind {
    factory.node(node).kind()
}

/// `Node.Text()` of an identifier.
fn identifier_text(factory: &dyn RuntimeFactory, node: NodeId) -> JsString {
    factory
        .node(node)
        .as_identifier()
        .expect("Identifier payload")
        .text_owned()
}

/// `Node.Name()`.
fn name_of(factory: &dyn RuntimeFactory, node: NodeId) -> Option<NodeId> {
    factory.node(node).name()
}

/// `Node.Expression()`.
fn expression_of(factory: &dyn RuntimeFactory, node: NodeId) -> Option<NodeId> {
    factory.node(node).expression()
}

/// `Node.Attributes().Properties()` of a JSX opening-like element.
fn attribute_properties(factory: &dyn RuntimeFactory, opener: NodeId) -> Vec<NodeId> {
    let attributes = {
        let read = factory.node(opener);
        if let Some(data) = read.as_jsx_opening_element() {
            data.attributes()
        } else if let Some(data) = read.as_jsx_self_closing_element() {
            data.attributes()
        } else {
            panic!("{NIL}")
        }
    };
    let properties = factory
        .node(attributes.expect(NIL))
        .as_jsx_attributes()
        .expect("JsxAttributes payload")
        .properties();
    list_nodes(factory, properties)
}

/// `Node.Properties()` of an object literal.
fn object_literal_properties(factory: &dyn RuntimeFactory, node: NodeId) -> Vec<NodeId> {
    let properties = factory
        .node(node)
        .as_object_literal_expression()
        .expect("ObjectLiteralExpression payload")
        .properties();
    list_nodes(factory, properties)
}

/// `JsxNamespacedName`'s `namespace:name` text.
fn namespaced_name_text(factory: &dyn RuntimeFactory, node: NodeId) -> JsString {
    let (namespace, name) = {
        let read = factory.node(node);
        let data = read
            .as_jsx_namespaced_name()
            .expect("JsxNamespacedName payload");
        (data.namespace(), data.name())
    };
    let namespace = identifier_text(factory, namespace.expect(NIL));
    let name = identifier_text(factory, name.expect(NIL));
    let mut text = namespace.as_bytes().to_vec();
    text.push(b':');
    text.extend_from_slice(name.as_bytes());
    JsString::from_bytes(text)
}

/// `node.SubtreeFacts()&ast.SubtreeContainsJsx != 0`: every JSX kind
/// contributes the fact and no exclusion removes it, so it holds exactly when
/// the subtree has a JSX node.
// TODO(ast_view): ast.Node.SubtreeFacts, through the factory's syntax view.
fn subtree_contains_jsx(factory: &dyn RuntimeFactory, node: NodeId) -> bool {
    struct Finder<'f> {
        factory: &'f dyn RuntimeFactory,
    }
    impl ChildVisitor for Finder<'_> {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            if subtree_contains_jsx(self.factory, node) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }
        fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
            self.visit_node_slice(self.factory.read_list(list).nodes())
        }
        fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
            let nodes: Vec<NodeId> = self.factory.read_nodes(nodes).iter().flatten().collect();
            for node in nodes {
                self.visit_node(node)?;
            }
            ControlFlow::Continue(())
        }
    }
    let read = factory.node(node);
    if matches!(
        read.kind().known(),
        Some(
            K::JsxElement
                | K::JsxAttributes
                | K::JsxNamespacedName
                | K::JsxOpeningElement
                | K::JsxSelfClosingElement
                | K::JsxFragment
                | K::JsxOpeningFragment
                | K::JsxClosingFragment
                | K::JsxAttribute
                | K::JsxSpreadAttribute
                | K::JsxClosingElement
                | K::JsxExpression
                | K::JsxText
        )
    ) {
        return true;
    }
    read.for_each_child(&mut Finder { factory }).is_break()
}

/// `ast.GetSemanticJsxChildren`.
// TODO(ast_view): tsr_ast::utilities_middle::get_semantic_jsx_children.
fn semantic_jsx_children(factory: &dyn RuntimeFactory, children: &[NodeId]) -> Vec<NodeId> {
    children
        .iter()
        .copied()
        .filter(|&child| {
            let read = factory.node(child);
            match read.kind().known() {
                Some(K::JsxExpression) => read.expression().is_some(),
                Some(K::JsxText) => !is_whitespace_only_jsx_text(&read),
                _ => true,
            }
        })
        .collect()
}

/// `ast.IsPrologueDirective`.
// TODO(ast_view): tsr_ast::utilities::is_prologue_directive.
fn is_prologue_directive(factory: &dyn RuntimeFactory, node: NodeId) -> bool {
    let read = factory.node(node);
    read.kind() == K::ExpressionStatement
        && factory.node(read.expression().expect(NIL)).kind() == K::StringLiteral
}

/// `ast.GetJSXImplicitImportBase`, whose port is the compiler's
/// `metadata::jsx_implicit_import_base`; the transformers cannot depend on the
/// compiler.
fn get_jsx_implicit_import_base(
    compiler_options: &CompilerOptions,
    file: &tsr_ast::SourceFileRead<'_>,
) -> Result<JsString, tsr_arena::Error> {
    let pragmas = file.pragmas()?;
    let jsx_import_source_pragma = get_pragma_from_source_file(pragmas.iter(), b"jsximportsource");
    let jsx_runtime_pragma = get_pragma_from_source_file(pragmas.iter(), b"jsxruntime");
    let factory = JsString::from_bytes(&b"factory"[..]);
    if get_pragma_argument(jsx_runtime_pragma, &factory) == b"classic" {
        return Ok(JsString::default());
    }
    if compiler_options.jsx == JsxEmit::REACT_JSX
        || compiler_options.jsx == JsxEmit::REACT_JSX_DEV
        || !compiler_options.jsx_import_source.is_empty()
        || jsx_import_source_pragma.is_some()
        || get_pragma_argument(jsx_runtime_pragma, &factory) == b"automatic"
    {
        let mut result = get_pragma_argument(jsx_import_source_pragma, &factory);
        if result.is_empty() {
            result = compiler_options.jsx_import_source.as_bytes();
        }
        if result.is_empty() {
            result = b"react";
        }
        return Ok(JsString::from_bytes(result));
    }
    Ok(JsString::default())
}

/// `ast.GetJSXRuntimeImport`, whose port is the compiler's
/// `metadata::jsx_runtime_import`.
fn get_jsx_runtime_import(base: &JsString, options: &CompilerOptions) -> JsString {
    if base.is_empty() {
        return base.clone();
    }
    let mut name = base.as_bytes().to_vec();
    name.push(b'/');
    name.extend_from_slice(if options.jsx == JsxEmit::REACT_JSX_DEV {
        b"jsx-dev-runtime"
    } else {
        b"jsx-runtime"
    });
    JsString::from_bytes(name)
}

impl JsxTransformer<'_> {
    fn context(&self) -> EmitContext {
        self.context.clone()
    }

    fn current_source_file(&self) -> NodeId {
        self.current_source_file.get().expect(NIL)
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.getCurrentFileNameExpression
    fn get_current_file_name_expression(&self, visitor: &mut NodeVisitor<'_>) -> NodeId {
        if let Some(declaration) = self.filename_declaration.get() {
            return name_of(visitor.factory(), declaration).expect(NIL);
        }
        let name = self.context().new_unique_name_ex(
            visitor,
            JsString::from_bytes(&b"_jsxFileName"[..]),
            AutoGenerateOptions {
                flags: g::OPTIMISTIC | g::FILE_LEVEL,
                ..AutoGenerateOptions::default()
            },
        );
        let file_name = match visitor
            .factory()
            .read_source_file(self.current_source_file())
        {
            Ok(file) => JsString::from_bytes(file.file_name()),
            Err(error) => {
                self.failure.record(error);
                JsString::default()
            }
        };
        let file_name = visitor.new_string_literal(file_name, token_flags::NONE);
        let d = visitor.new_variable_declaration(Some(name), None, None, Some(file_name));
        self.filename_declaration.set(Some(d));
        name_of(visitor.factory(), d).expect(NIL)
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.getJsxFactoryCalleePrimitive
    fn get_jsx_factory_callee_primitive(&self, is_static_children: bool) -> &'static [u8] {
        if self.compiler_options.jsx == JsxEmit::REACT_JSX_DEV {
            return b"jsxDEV";
        }
        if is_static_children {
            return b"jsxs";
        }
        b"jsx"
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.getJsxFactoryCallee
    fn get_jsx_factory_callee(
        &self,
        visitor: &mut NodeVisitor<'_>,
        is_static_children: bool,
    ) -> NodeId {
        let t = self.get_jsx_factory_callee_primitive(is_static_children);
        self.get_implicit_import_for_name(visitor, t)
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.getImplicitJsxFragmentReference
    fn get_implicit_jsx_fragment_reference(&self, visitor: &mut NodeVisitor<'_>) -> NodeId {
        self.get_implicit_import_for_name(visitor, b"Fragment")
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.getImplicitImportForName
    fn get_implicit_import_for_name(&self, visitor: &mut NodeVisitor<'_>, name: &[u8]) -> NodeId {
        let mut import_source = self.import_specifier.borrow().clone();
        if name != b"createElement" {
            import_source = get_jsx_runtime_import(&import_source, &self.compiler_options);
        }
        let existing = {
            let mut imports = self.utilized_implicit_runtime_imports.borrow_mut();
            if let Some((_, existing)) = imports
                .iter()
                .find(|(source, _)| source.as_bytes() == import_source.as_bytes())
            {
                existing
                    .iter()
                    .find(|(specifier_name, _)| specifier_name.as_bytes() == name)
                    .map(|(_, elem)| *elem)
            } else {
                imports.push((import_source.clone(), Vec::new()));
                None
            }
        };
        if let Some(elem) = existing {
            return name_of(visitor.factory(), elem).expect(NIL);
        }

        let mut generated_text = b"_".to_vec();
        generated_text.extend_from_slice(name);
        let generated_name = self.context().new_unique_name_ex(
            visitor,
            JsString::from_bytes(generated_text),
            AutoGenerateOptions {
                flags: g::OPTIMISTIC | g::FILE_LEVEL | g::ALLOW_NAME_SUBSTITUTION,
                ..AutoGenerateOptions::default()
            },
        );
        let property_name = visitor.new_identifier(JsString::from_bytes(name));
        let specifier =
            visitor.new_import_specifier(false, Some(property_name), Some(generated_name));
        let result = self
            .emit_resolver
            .borrow_mut()
            .set_referenced_import_declaration(generated_name, specifier);
        self.failure.ok(result);
        if let Some((_, existing)) = self
            .utilized_implicit_runtime_imports
            .borrow_mut()
            .iter_mut()
            .find(|(source, _)| source.as_bytes() == import_source.as_bytes())
        {
            existing.push((JsString::from_bytes(name), specifier));
        }
        name_of(visitor.factory(), specifier).expect(NIL)
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.setInChild
    fn set_in_child(&self, v: bool) {
        self.in_jsx_child.set(v);
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        let node = node?;
        if self.failure.is_set() {
            return Some(node);
        }
        if !subtree_contains_jsx(visitor.factory(), node) {
            return Some(node);
        }
        match kind(visitor.factory(), node).known() {
            Some(K::SourceFile) => {
                self.set_in_child(false);
                return Some(self.visit_source_file(visitor, node));
            }
            Some(K::JsxElement) => return Some(self.visit_jsx_element(visitor, node)),
            Some(K::JsxSelfClosingElement) => {
                return Some(self.visit_jsx_self_closing_element(visitor, node));
            }
            Some(K::JsxFragment) => return Some(self.visit_jsx_fragment(visitor, node)),
            Some(K::JsxOpeningElement) => {
                panic!("JsxOpeningElement should not be visited, handled in visitJsxElement")
            }
            Some(K::JsxOpeningFragment) => {
                panic!("JsxOpeningFragment should not be visited, handled in visitJsxFragment")
            }
            Some(K::JsxText) => {
                self.set_in_child(false);
                return self.visit_jsx_text(visitor, node);
            }
            Some(K::JsxExpression) => {
                self.set_in_child(false);
                return self.visit_jsx_expression(visitor, node);
            }
            _ => {}
        }
        self.set_in_child(false);
        visitor.visit_each_child(Some(node)) // by default, do nothing
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.shouldUseCreateElement
    fn should_use_create_element(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        self.import_specifier.borrow().is_empty() || has_key_after_props_spread(factory, node)
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.isAnyPrologueDirective
    fn is_any_prologue_directive(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        is_prologue_directive(factory, node)
            || (self.context.emit_flags(node) & emit_flags::CUSTOM_PROLOGUE != 0)
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.insertStatementAfterCustomPrologue
    fn insert_statement_after_custom_prologue(
        &self,
        factory: &dyn RuntimeFactory,
        to: &mut Vec<NodeId>,
        statement: NodeId,
    ) {
        insert_statement_after_prologue(to, Some(statement), |node| {
            self.is_any_prologue_directive(factory, node)
        });
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.visitSourceFile
    fn visit_source_file(&self, visitor: &mut NodeVisitor<'_>, file: NodeId) -> NodeId {
        let facts = visitor.factory().read_source_file(file).and_then(|state| {
            let import_specifier = get_jsx_implicit_import_base(&self.compiler_options, &state)?;
            Ok((
                state.is_declaration_file,
                import_specifier,
                tsr_ast::utilities::is_external_module(&state),
                tsr_ast::utilities::is_external_or_common_js_module(&state),
            ))
        });
        let Some((is_declaration_file, import_specifier, is_external_module, is_common_js)) =
            self.failure.ok(facts)
        else {
            return file;
        };
        if is_declaration_file {
            return file;
        }

        self.current_source_file.set(Some(file));
        *self.import_specifier.borrow_mut() = import_specifier;
        self.filename_declaration.set(None);
        self.utilized_implicit_runtime_imports.borrow_mut().clear();

        let visited = visitor.visit_each_child(Some(file)).expect(NIL);
        let mut context = self.context();
        let helpers = context.read_emit_helpers();
        context.add_emit_helper(visited, &helpers);
        let statements_list = visitor
            .factory()
            .node(visited)
            .as_source_file()
            .expect("SourceFile payload")
            .statements();
        let mut statements = list_nodes(visitor.factory(), statements_list);
        let mut statements_updated = false;
        if let Some(filename_declaration) = self.filename_declaration.get() {
            let declarations = new_node_list(visitor.factory_mut(), vec![filename_declaration]);
            let list = visitor.new_variable_declaration_list(Some(declarations), node_flags::CONST);
            let statement = visitor.new_variable_statement(None, Some(list));
            self.insert_statement_after_custom_prologue(
                visitor.factory(),
                &mut statements,
                statement,
            );
            statements_updated = true;
        }

        let imports = self.utilized_implicit_runtime_imports.borrow().clone();
        if !imports.is_empty() {
            if is_external_module {
                statements_updated = true;
                let mut new_statements = Vec::with_capacity(imports.len());
                for (import_source, import_specifiers_map) in &imports {
                    let sorted = get_sorted_specifiers(visitor.factory(), import_specifiers_map);
                    let elements = new_node_list(visitor.factory_mut(), sorted);
                    let named_imports = visitor.new_named_imports(Some(elements));
                    let clause =
                        visitor.new_import_clause(K::Unknown.into(), None, Some(named_imports));
                    let module_specifier =
                        visitor.new_string_literal(import_source.clone(), token_flags::NONE);
                    let s = visitor.new_import_declaration(
                        None,
                        Some(clause),
                        Some(module_specifier),
                        None,
                    );
                    tsr_ast::set_parent_in_children(visitor.factory_mut(), s);
                    new_statements.push(s);
                }
                for e in new_statements {
                    self.insert_statement_after_custom_prologue(
                        visitor.factory(),
                        &mut statements,
                        e,
                    );
                }
            } else if is_common_js {
                statements_updated = true;
                let mut new_statements = Vec::with_capacity(imports.len());
                for (import_source, import_specifiers_map) in &imports {
                    let sorted = get_sorted_specifiers(visitor.factory(), import_specifiers_map);
                    let mut as_binding_elems = Vec::with_capacity(sorted.len());
                    for elem in sorted {
                        let (property_name, name) = {
                            let read = visitor.factory().node(elem);
                            let data = read.as_import_specifier().expect("ImportSpecifier payload");
                            (data.property_name(), data.name())
                        };
                        as_binding_elems.push(visitor.new_binding_element(
                            None,
                            property_name,
                            name,
                            None,
                        ));
                    }
                    let elements = new_node_list(visitor.factory_mut(), as_binding_elems);
                    let pattern =
                        visitor.new_binding_pattern(K::ObjectBindingPattern.into(), Some(elements));
                    let require = visitor.new_identifier(JsString::from_bytes(&b"require"[..]));
                    let argument =
                        visitor.new_string_literal(import_source.clone(), token_flags::NONE);
                    let arguments = new_node_list(visitor.factory_mut(), vec![argument]);
                    let call = visitor.new_call_expression(
                        Some(require),
                        None,
                        None,
                        Some(arguments),
                        node_flags::NONE,
                    );
                    let declaration =
                        visitor.new_variable_declaration(Some(pattern), None, None, Some(call));
                    let declarations = new_node_list(visitor.factory_mut(), vec![declaration]);
                    let list = visitor
                        .new_variable_declaration_list(Some(declarations), node_flags::CONST);
                    let s = visitor.new_variable_statement(None, Some(list));
                    tsr_ast::set_parent_in_children(visitor.factory_mut(), s);
                    new_statements.push(s);
                }
                for e in new_statements {
                    self.insert_statement_after_custom_prologue(
                        visitor.factory(),
                        &mut statements,
                        e,
                    );
                }
            } else {
                // Do nothing (script file) - consider an error in the checker?
            }
        }

        let mut visited = visited;
        if statements_updated {
            let end_of_file_token = visitor
                .factory()
                .node(file)
                .as_source_file()
                .expect("SourceFile payload")
                .end_of_file_token();
            let list = new_node_list(visitor.factory_mut(), statements);
            visited = visitor
                .factory_mut()
                .update_source(file, Some(list), end_of_file_token);
        }

        self.current_source_file.set(None);
        *self.import_specifier.borrow_mut() = JsString::default();
        self.filename_declaration.set(None);
        self.utilized_implicit_runtime_imports.borrow_mut().clear();

        visited
    }

    /// `core.NewTextRange(scanner.SkipTrivia(text, node.Pos()), node.End())`
    /// over the current file's text.
    fn element_location(&self, factory: &dyn RuntimeFactory, node: NodeId) -> TextRange {
        let (pos, end) = {
            let read = factory.node(node);
            (read.pos(), read.end())
        };
        let start = match factory.read_source_file(self.current_source_file()) {
            Ok(file) => tsr_scanner::skip_trivia(file.text().as_bytes(), i64::from(pos)),
            Err(error) => {
                self.failure.record(error);
                i64::from(pos)
            }
        };
        TextRange::new(start, i64::from(end))
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.visitJsxElement
    fn visit_jsx_element(&self, visitor: &mut NodeVisitor<'_>, element: NodeId) -> NodeId {
        let use_create_element = self.should_use_create_element(visitor.factory(), element);
        let location = self.element_location(visitor.factory(), element);
        let (opening_element, children) = {
            let read = visitor.factory().node(element);
            let data = read.as_jsx_element().expect("JsxElement payload");
            (data.opening_element(), data.children())
        };
        let opening_element = opening_element.expect(NIL);
        if use_create_element {
            self.visit_jsx_opening_like_element_create_element(
                visitor,
                opening_element,
                children,
                location,
            )
        } else {
            self.visit_jsx_opening_like_element_jsx(visitor, opening_element, children, location)
        }
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.visitJsxSelfClosingElement
    fn visit_jsx_self_closing_element(
        &self,
        visitor: &mut NodeVisitor<'_>,
        element: NodeId,
    ) -> NodeId {
        let use_create_element = self.should_use_create_element(visitor.factory(), element);
        let location = self.element_location(visitor.factory(), element);
        if use_create_element {
            self.visit_jsx_opening_like_element_create_element(visitor, element, None, location)
        } else {
            self.visit_jsx_opening_like_element_jsx(visitor, element, None, location)
        }
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.visitJsxFragment
    fn visit_jsx_fragment(&self, visitor: &mut NodeVisitor<'_>, fragment: NodeId) -> NodeId {
        let use_create_element = self.import_specifier.borrow().is_empty();
        let location = self.element_location(visitor.factory(), fragment);
        let (opening_fragment, children) = {
            let read = visitor.factory().node(fragment);
            let data = read.as_jsx_fragment().expect("JsxFragment payload");
            (data.opening_fragment(), data.children())
        };
        let opening_fragment = opening_fragment.expect(NIL);
        if use_create_element {
            self.visit_jsx_opening_fragment_create_element(
                visitor,
                opening_fragment,
                children,
                location,
            )
        } else {
            self.visit_jsx_opening_fragment_jsx(visitor, children, location)
        }
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.convertJsxChildrenToChildrenPropObject
    fn convert_jsx_children_to_children_prop_object(
        &self,
        visitor: &mut NodeVisitor<'_>,
        children: &[NodeId],
    ) -> Option<NodeId> {
        let prop = self.convert_jsx_children_to_children_prop_assignment(visitor, children)?;
        let properties = new_node_list(visitor.factory_mut(), vec![prop]);
        Some(visitor.new_object_literal_expression(Some(properties), false))
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.transformJsxChildToExpression
    fn transform_jsx_child_to_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let prev = self.in_jsx_child.get();
        self.set_in_child(true);
        let result = self.visit(visitor, Some(node));
        self.set_in_child(prev);
        result
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.convertJsxChildrenToChildrenPropAssignment
    fn convert_jsx_children_to_children_prop_assignment(
        &self,
        visitor: &mut NodeVisitor<'_>,
        children: &[NodeId],
    ) -> Option<NodeId> {
        let non_whitespce_children = semantic_jsx_children(visitor.factory(), children);
        if non_whitespce_children.len() == 1
            && (kind(visitor.factory(), non_whitespce_children[0]) != K::JsxExpression
                || jsx_expression_dot_dot_dot(visitor.factory(), non_whitespce_children[0])
                    .is_none())
        {
            let result =
                self.transform_jsx_child_to_expression(visitor, non_whitespce_children[0])?;
            let name = visitor.new_identifier(JsString::from_bytes(&b"children"[..]));
            return Some(visitor.new_property_assignment(
                None,
                Some(name),
                None,
                None,
                Some(result),
            ));
        }
        // For multiple children in the children property array, don't set StartOnNewLine
        // on child elements — the array literal is single-line.
        let mut results = Vec::with_capacity(non_whitespce_children.len());
        for child in non_whitespce_children {
            let Some(res) = self.transform_jsx_child_to_expression(visitor, child) else {
                continue;
            };
            let mut context = self.context();
            let flags = context.emit_flags(res);
            context.set_emit_flags(res, flags & !emit_flags::START_ON_NEW_LINE);
            results.push(res);
        }
        if results.is_empty() {
            return None;
        }
        let name = visitor.new_identifier(JsString::from_bytes(&b"children"[..]));
        let elements = new_node_list(visitor.factory_mut(), results);
        let array = visitor.new_array_literal_expression(Some(elements), false);
        Some(visitor.new_property_assignment(None, Some(name), None, None, Some(array)))
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.getTagName
    fn get_tag_name(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let node_kind = kind(visitor.factory(), node);
        if node_kind == K::JsxElement {
            let opening_element = visitor
                .factory()
                .node(node)
                .as_jsx_element()
                .expect("JsxElement payload")
                .opening_element()
                .expect(NIL);
            return self.get_tag_name(visitor, opening_element);
        } else if matches!(
            node_kind.known(),
            Some(K::JsxOpeningElement | K::JsxSelfClosingElement)
        ) {
            let tag_name = {
                let read = visitor.factory().node(node);
                if let Some(data) = read.as_jsx_opening_element() {
                    data.tag_name()
                } else {
                    read.as_jsx_self_closing_element()
                        .expect("JsxSelfClosingElement payload")
                        .tag_name()
                }
            };
            let tag_name = tag_name.expect(NIL);
            let tag_kind = kind(visitor.factory(), tag_name);
            if tag_kind == K::Identifier
                && tsr_scanner::is_intrinsic_jsx_name(
                    identifier_text(visitor.factory(), tag_name).as_bytes(),
                )
            {
                let text = identifier_text(visitor.factory(), tag_name);
                return visitor.new_string_literal(text, token_flags::NONE);
            } else if tag_kind == K::JsxNamespacedName {
                let text = namespaced_name_text(visitor.factory(), tag_name);
                return visitor.new_string_literal(text, token_flags::NONE);
            }
            return self
                .context
                .create_expression_from_entity_name(visitor.factory_mut(), tag_name);
        }
        panic!(
            "unhandled node kind passed to getTagName: {:?}",
            node_kind.known()
        )
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.visitJsxOpeningLikeElementJSX
    fn visit_jsx_opening_like_element_jsx(
        &self,
        visitor: &mut NodeVisitor<'_>,
        element: NodeId,
        children: Option<NodeListId>,
        location: TextRange,
    ) -> NodeId {
        let tag_name = self.get_tag_name(visitor, element);
        let children_nodes = list_nodes(visitor.factory(), children);
        let mut children_prop = None;
        if children.is_some() && !children_nodes.is_empty() {
            children_prop =
                self.convert_jsx_children_to_children_prop_assignment(visitor, &children_nodes);
        }
        let mut key_attr = None;
        let mut attrs = attribute_properties(visitor.factory(), element);
        for (i, &p) in attrs.iter().enumerate() {
            if kind(visitor.factory(), p) == K::JsxAttribute
                && name_of(visitor.factory(), p).is_some_and(|name| {
                    kind(visitor.factory(), name) == K::Identifier
                        && identifier_text(visitor.factory(), name).as_bytes() == b"key"
                })
            {
                key_attr = Some(p);
                attrs.remove(i);
                break;
            }
        }
        let object = if attrs.is_empty() {
            let mut object_children = Vec::new();
            if let Some(children_prop) = children_prop {
                object_children.push(children_prop);
            }
            let properties = new_node_list(visitor.factory_mut(), object_children);
            // When there are no attributes, React wants {}
            visitor.new_object_literal_expression(Some(properties), false)
        } else {
            self.transform_jsx_attributes_to_object_props(visitor, &attrs, children_prop)
        };
        self.visit_jsx_opening_like_element_or_fragment_jsx(
            visitor, tag_name, object, key_attr, children, location,
        )
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.transformJsxAttributesToObjectProps
    fn transform_jsx_attributes_to_object_props(
        &self,
        visitor: &mut NodeVisitor<'_>,
        attrs: &[NodeId],
        children_prop: Option<NodeId>,
    ) -> NodeId {
        let target = self.compiler_options.emit_script_target();
        if target >= ScriptTarget::ES2018 {
            // target has object spreads, can keep as-is
            let props = self.transform_jsx_attributes_to_props(visitor, attrs, children_prop);
            let properties = new_node_list(visitor.factory_mut(), props);
            return visitor.new_object_literal_expression(Some(properties), false);
        }
        self.transform_jsx_attributes_to_expression(visitor, attrs, children_prop)
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.transformJsxAttributesToExpression
    fn transform_jsx_attributes_to_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        attrs: &[NodeId],
        children_prop: Option<NodeId>,
    ) -> NodeId {
        let mut expressions: Vec<Option<NodeId>> = Vec::with_capacity(2);
        let mut properties: Vec<Option<NodeId>> = Vec::with_capacity(attrs.len());

        for &attr in attrs {
            if kind(visitor.factory(), attr) == K::JsxSpreadAttribute {
                let expression = expression_of(visitor.factory(), attr).expect(NIL);
                // as an optimization we try to flatten the first level of spread inline object
                // as if its props would be passed as JSX attributes
                if kind(visitor.factory(), expression) == K::ObjectLiteralExpression
                    && !has_proto(visitor.factory(), expression)
                {
                    for prop in object_literal_properties(visitor.factory(), expression) {
                        if kind(visitor.factory(), prop) == K::SpreadAssignment {
                            self.combine_properties_into_new_expression(
                                visitor,
                                &mut expressions,
                                &mut properties,
                            );
                            let inner = expression_of(visitor.factory(), prop);
                            expressions.push(self.visit(visitor, inner));
                            continue;
                        }
                        properties.push(self.visit(visitor, Some(prop)));
                    }
                    continue;
                }
                self.combine_properties_into_new_expression(
                    visitor,
                    &mut expressions,
                    &mut properties,
                );
                expressions.push(self.visit(visitor, Some(expression)));
                continue;
            }
            properties.push(Some(
                self.transform_jsx_attribute_to_object_literal_element(visitor, attr),
            ));
        }

        if let Some(children_prop) = children_prop {
            properties.push(Some(children_prop));
        }

        self.combine_properties_into_new_expression(visitor, &mut expressions, &mut properties);

        if !expressions.is_empty()
            && kind(visitor.factory(), expressions[0].expect(NIL)) != K::ObjectLiteralExpression
        {
            // We must always emit at least one object literal before a spread attribute
            // as the JSX always factory expects a fresh object, so we need to make a copy here
            // we also avoid mutating an external reference by doing this (first expression is used as assign's target)
            let properties = new_node_list(visitor.factory_mut(), Vec::new());
            let empty = visitor.new_object_literal_expression(Some(properties), false);
            expressions.insert(0, Some(empty));
        }

        if expressions.len() == 1 {
            return expressions[0].expect(NIL);
        }
        let expressions = expressions
            .into_iter()
            .map(|expression| expression.expect(NIL))
            .collect();
        self.context.new_assign_helper(
            visitor.factory_mut(),
            expressions,
            self.compiler_options.emit_script_target(),
        )
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.combinePropertiesIntoNewExpression
    #[allow(clippy::unused_self)] // upstream's method
    fn combine_properties_into_new_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        expressions: &mut Vec<Option<NodeId>>,
        props: &mut Vec<Option<NodeId>>,
    ) {
        if props.is_empty() {
            return;
        }
        let nodes = visitor.factory_mut().alloc_nodes(std::mem::take(props));
        let list = visitor
            .factory_mut()
            .alloc_list(TextRange::new(-1, -1), nodes);
        let new_obj = visitor.new_object_literal_expression(Some(list), false);
        expressions.push(Some(new_obj));
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.transformJsxAttributesToProps
    fn transform_jsx_attributes_to_props(
        &self,
        visitor: &mut NodeVisitor<'_>,
        attrs: &[NodeId],
        children_prop: Option<NodeId>,
    ) -> Vec<NodeId> {
        let mut props = Vec::with_capacity(attrs.len());
        for &attr in attrs {
            if kind(visitor.factory(), attr) == K::JsxSpreadAttribute {
                let res = self.transform_jsx_spread_attributes_to_props(visitor, attr);
                props.extend(res);
            } else {
                props.push(self.transform_jsx_attribute_to_object_literal_element(visitor, attr));
            }
        }
        if let Some(children_prop) = children_prop {
            props.push(children_prop);
        }
        props
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.transformJsxSpreadAttributesToProps
    fn transform_jsx_spread_attributes_to_props(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Vec<NodeId> {
        let expression = expression_of(visitor.factory(), node).expect(NIL);
        if kind(visitor.factory(), expression) == K::ObjectLiteralExpression
            && !has_proto(visitor.factory(), expression)
        {
            let properties = visitor
                .factory()
                .node(expression)
                .as_object_literal_expression()
                .expect("ObjectLiteralExpression payload")
                .properties();
            let nodes = properties.map_or_else(NodeSlice::empty, |list| {
                visitor.factory().read_list(list).nodes()
            });
            let (res, _) = visitor.visit_slice(nodes);
            return slice_nodes(visitor.factory(), res);
        }
        let visited = self.visit(visitor, Some(expression));
        vec![visitor.new_spread_assignment(visited)]
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.transformJsxAttributeToObjectLiteralElement
    fn transform_jsx_attribute_to_object_literal_element(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let name = self.get_attribute_name(visitor, node);
        let initializer = visitor
            .factory()
            .node(node)
            .as_jsx_attribute()
            .expect("JsxAttribute payload")
            .initializer();
        let expression = self.transform_jsx_attribute_initializer(visitor, initializer);
        visitor.new_property_assignment(None, Some(name), None, None, expression)
    }

    /// Emit an attribute name, which is quoted if it needs to be quoted. Because
    /// these emit into an object literal property name, we don't need to be worried
    /// about keywords, just non-identifier characters
    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.getAttributeName
    #[allow(clippy::unused_self)] // upstream's method
    fn get_attribute_name(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let name = name_of(visitor.factory(), node).expect(NIL);
        if kind(visitor.factory(), name) == K::Identifier {
            let text = identifier_text(visitor.factory(), name);
            if tsr_scanner::is_identifier_text(text.as_bytes(), LanguageVariant::STANDARD) {
                return name;
            }
            return visitor.new_string_literal(text, token_flags::NONE);
        }
        // must be jsx namespace
        let text = namespaced_name_text(visitor.factory(), name);
        visitor.new_string_literal(text, token_flags::NONE)
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.transformJsxAttributeInitializer
    fn transform_jsx_attribute_initializer(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: Option<NodeId>,
    ) -> Option<NodeId> {
        let Some(node) = node else {
            return Some(self.context.new_true_expression(visitor));
        };
        let node_kind = kind(visitor.factory(), node);
        if node_kind == K::StringLiteral {
            // Always recreate the literal to escape any escape sequences or newlines which may be in the original jsx string and which
            // Need to be escaped to be handled correctly in a normal string
            let (text, flags, loc) = {
                let read = visitor.factory().node(node);
                let data = read.as_string_literal().expect("StringLiteral payload");
                (
                    decode_entities(data.text()),
                    data.token_flags(),
                    read.range(),
                )
            };
            let res = visitor.new_string_literal(JsString::from_bytes(text), flags);
            visitor.set_node_range(res, loc);
            // Preserve the original quote style (single vs double quotes)
            if let NodeData::StringLiteral(data) = visitor.node_mut(res).data_mut() {
                data.token_flags = flags;
            }
            return Some(res);
        }
        if node_kind == K::JsxExpression {
            let Some(expression) = expression_of(visitor.factory(), node) else {
                return Some(self.context.new_true_expression(visitor));
            };
            return self.visit(visitor, Some(expression));
        }
        if matches!(
            node_kind.known(),
            Some(K::JsxElement | K::JsxSelfClosingElement | K::JsxFragment)
        ) {
            self.set_in_child(false);
            return self.visit(visitor, Some(node));
        }
        panic!(
            "Unhandled node kind found in jsx initializer: {:?}",
            node_kind.known()
        )
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.visitJsxOpeningLikeElementOrFragmentJSX
    fn visit_jsx_opening_like_element_or_fragment_jsx(
        &self,
        visitor: &mut NodeVisitor<'_>,
        tag_name: NodeId,
        object: NodeId,
        key_attr: Option<NodeId>,
        children: Option<NodeListId>,
        location: TextRange,
    ) -> NodeId {
        let non_whitespace_children = if children.is_some() {
            let nodes = list_nodes(visitor.factory(), children);
            semantic_jsx_children(visitor.factory(), &nodes)
        } else {
            Vec::new()
        };
        let is_static_children = non_whitespace_children.len() > 1
            || (non_whitespace_children.len() == 1
                && kind(visitor.factory(), non_whitespace_children[0]) == K::JsxExpression
                && jsx_expression_dot_dot_dot(visitor.factory(), non_whitespace_children[0])
                    .is_some());
        let mut args = Vec::with_capacity(3);
        args.push(tag_name);
        args.push(object);
        // function jsx(type, config, maybeKey) {}
        // "maybeKey" is optional. It is acceptable to use "_jsx" without a third argument
        if let Some(key_attr) = key_attr {
            let initializer = visitor
                .factory()
                .node(key_attr)
                .as_jsx_attribute()
                .expect("JsxAttribute payload")
                .initializer();
            args.push(
                self.transform_jsx_attribute_initializer(visitor, initializer)
                    .expect(NIL),
            );
        }

        if self.compiler_options.jsx == JsxEmit::REACT_JSX_DEV {
            let original_file = self.context.most_original(self.current_source_file());
            if kind(visitor.factory(), original_file) == K::SourceFile {
                // "maybeKey" has to be replaced with "void 0" to not break the jsxDEV signature
                if key_attr.is_none() {
                    args.push(self.context.new_void_zero_expression(visitor));
                }
                // isStaticChildren development flag
                if is_static_children {
                    args.push(self.context.new_true_expression(visitor));
                } else {
                    args.push(self.context.new_false_expression(visitor));
                }
                // __source development flag
                let (line, col) =
                    self.ecma_line_and_utf16_character(visitor.factory(), original_file, location);
                let file_name_key = visitor.new_identifier(JsString::from_bytes(&b"fileName"[..]));
                let file_name = self.get_current_file_name_expression(visitor);
                let file_name = visitor.new_property_assignment(
                    None,
                    Some(file_name_key),
                    None,
                    None,
                    Some(file_name),
                );
                let line_key = visitor.new_identifier(JsString::from_bytes(&b"lineNumber"[..]));
                let line_number = visitor.new_numeric_literal(
                    JsString::from_bytes((line + 1).to_string().into_bytes()),
                    token_flags::NONE,
                );
                let line_number = visitor.new_property_assignment(
                    None,
                    Some(line_key),
                    None,
                    None,
                    Some(line_number),
                );
                let column_key = visitor.new_identifier(JsString::from_bytes(&b"columnNumber"[..]));
                let column_number = visitor.new_numeric_literal(
                    JsString::from_bytes((col + 1).to_string().into_bytes()),
                    token_flags::NONE,
                );
                let column_number = visitor.new_property_assignment(
                    None,
                    Some(column_key),
                    None,
                    None,
                    Some(column_number),
                );
                let properties = new_node_list(
                    visitor.factory_mut(),
                    vec![file_name, line_number, column_number],
                );
                args.push(visitor.new_object_literal_expression(Some(properties), false));
                // __self development flag
                args.push(self.context.new_this_expression(visitor));
            }
        }

        let callee = self.get_jsx_factory_callee(visitor, is_static_children);
        let arguments = new_node_list(visitor.factory_mut(), args);
        let element = visitor.new_call_expression(
            Some(callee),
            None,
            None,
            Some(arguments),
            node_flags::NONE,
        );
        visitor.set_node_range(element, location);

        if self.in_jsx_child.get() {
            self.context()
                .add_emit_flags(element, emit_flags::START_ON_NEW_LINE);
        }

        element
    }

    /// `scanner.GetECMALineAndUTF16CharacterOfPosition(originalFile, location.Pos())`.
    fn ecma_line_and_utf16_character(
        &self,
        factory: &dyn RuntimeFactory,
        original_file: NodeId,
        location: TextRange,
    ) -> (isize, isize) {
        let file = match factory.read_source_file(original_file) {
            Ok(file) => file,
            Err(error) => {
                self.failure.record(error);
                return (0, 0);
            }
        };
        let pos = location.pos() as isize;
        let line_map = file.ecma_line_map();
        let line = tsr_jsstring::scanner_positions::compute_line_of_position(line_map, pos);
        let start = usize::try_from(line_map[usize::try_from(line).expect(NIL)]).expect(NIL);
        let end = usize::try_from(pos).expect(NIL);
        let character = tsr_jsstring::line_map::utf16_len(&file.text().as_bytes()[start..end]);
        (line, character)
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.visitJsxOpeningFragmentJSX
    fn visit_jsx_opening_fragment_jsx(
        &self,
        visitor: &mut NodeVisitor<'_>,
        children: Option<NodeListId>,
        location: TextRange,
    ) -> NodeId {
        let mut children_props = None;
        let children_nodes = list_nodes(visitor.factory(), children);
        if children.is_some() && !children_nodes.is_empty() {
            let result =
                self.convert_jsx_children_to_children_prop_object(visitor, &children_nodes);
            if result.is_some() {
                children_props = result;
            }
        }
        let children_props = if let Some(props) = children_props {
            props
        } else {
            let properties = new_node_list(visitor.factory_mut(), Vec::new());
            visitor.new_object_literal_expression(Some(properties), false)
        };
        let tag_name = self.get_implicit_jsx_fragment_reference(visitor);
        self.visit_jsx_opening_like_element_or_fragment_jsx(
            visitor,
            tag_name,
            children_props,
            None,
            children,
            location,
        )
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.createReactNamespace
    fn create_react_namespace(
        &self,
        visitor: &mut NodeVisitor<'_>,
        react_namespace: &[u8],
        parent: NodeId,
    ) -> NodeId {
        // To ensure the emit resolver can properly resolve the namespace, we need to
        // treat this identifier as if it were a source tree node by clearing the `Synthesized`
        // flag and setting a parent node. TODO: Is this still true? The emit resolver is supposed to be
        // hardened aginast this, so long as the node retains original node pointers back to a parsed node
        let react_namespace = if react_namespace.is_empty() {
            &b"React"[..]
        } else {
            react_namespace
        };
        let react = visitor.new_identifier(JsString::from_bytes(react_namespace));
        let flags = visitor.factory().node(react).flags();
        visitor.set_node_flags(react, flags & !node_flags::SYNTHESIZED);

        // Set the parent that is in parse tree
        // this makes sure that parent chain is intact for checker to traverse complete scope tree
        let parse_parent = self.context.parse_node(visitor, parent);
        visitor.set_node_parent(react, parse_parent);

        // If the identifier refers to an exported member of a namespace, substitute with
        // a qualified namespace property access (e.g., `React` -> `M.React`).
        // See also: RuntimeSyntaxTransformer.visitExpressionIdentifier in runtimesyntax.go
        let container = self
            .emit_resolver
            .borrow_mut()
            .get_referenced_export_container(react, false /*prefixLocals*/);
        if let Some(container) = self.failure.ok(container).flatten() {
            if kind(visitor.factory(), container) == K::ModuleDeclaration {
                let container_name = self
                    .context()
                    .new_generated_name_for_node(visitor, container);
                return visitor.new_property_access_expression(
                    Some(container_name),
                    None,
                    Some(react),
                    node_flags::NONE,
                );
            }
        }

        react
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.createJsxFactoryExpressionFromEntityName
    fn create_jsx_factory_expression_from_entity_name(
        &self,
        visitor: &mut NodeVisitor<'_>,
        e: NodeId,
        parent: NodeId,
    ) -> NodeId {
        let qualified = visitor
            .factory()
            .node(e)
            .as_qualified_name()
            .map(|data| (data.left(), data.right()));
        if let Some((left, right)) = qualified {
            let left = self.create_jsx_factory_expression_from_entity_name(
                visitor,
                left.expect(NIL),
                parent,
            );
            let right_text = identifier_text(visitor.factory(), right.expect(NIL));
            let right = visitor.new_identifier(right_text);
            return visitor.new_property_access_expression(
                Some(left),
                None,
                Some(right),
                node_flags::NONE,
            );
        }
        let text = identifier_text(visitor.factory(), e);
        self.create_react_namespace(visitor, text.as_bytes(), parent)
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.createJsxPseudoFactoryExpression
    fn create_jsx_pseudo_factory_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        parent: NodeId,
        e: Option<NodeId>,
        target: &[u8],
    ) -> NodeId {
        if let Some(e) = e {
            return self.create_jsx_factory_expression_from_entity_name(visitor, e, parent);
        }
        let react_namespace = self.compiler_options.react_namespace.clone();
        let react = self.create_react_namespace(visitor, react_namespace.as_bytes(), parent);
        let target = visitor.new_identifier(JsString::from_bytes(target));
        visitor.new_property_access_expression(Some(react), None, Some(target), node_flags::NONE)
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.createJsxFactoryExpression
    fn create_jsx_factory_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        parent: NodeId,
    ) -> NodeId {
        let e = self
            .emit_resolver
            .borrow_mut()
            .get_jsx_factory_entity(self.current_source_file());
        let e = self.failure.ok(e).flatten();
        self.create_jsx_pseudo_factory_expression(visitor, parent, e, b"createElement")
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.createJsxFragmentFactoryExpression
    fn create_jsx_fragment_factory_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        parent: NodeId,
    ) -> NodeId {
        let e = self
            .emit_resolver
            .borrow_mut()
            .get_jsx_fragment_factory_entity(self.current_source_file());
        let e = self.failure.ok(e).flatten();
        self.create_jsx_pseudo_factory_expression(visitor, parent, e, b"Fragment")
    }

    /// The children loop shared by the two `createElement` visitors: each
    /// child visited as a JSX child, the nil results dropped, and every child
    /// starting on a new line when more than one remains.
    fn visit_create_element_children(
        &self,
        visitor: &mut NodeVisitor<'_>,
        children: Option<NodeListId>,
    ) -> Vec<NodeId> {
        let mut new_children = Vec::new();
        let children_nodes = list_nodes(visitor.factory(), children);
        if children.is_some() && !children_nodes.is_empty() {
            for c in children_nodes {
                if let Some(res) = self.transform_jsx_child_to_expression(visitor, c) {
                    new_children.push(res);
                }
            }
        }

        // Add StartOnNewLine flag only if there are multiple actual children (after filtering)
        if new_children.len() > 1 {
            let mut context = self.context();
            for &child in &new_children {
                context.add_emit_flags(child, emit_flags::START_ON_NEW_LINE);
            }
        }
        new_children
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.visitJsxOpeningLikeElementCreateElement
    fn visit_jsx_opening_like_element_create_element(
        &self,
        visitor: &mut NodeVisitor<'_>,
        element: NodeId,
        children: Option<NodeListId>,
        location: TextRange,
    ) -> NodeId {
        let tag_name = self.get_tag_name(visitor, element);
        let attrs = attribute_properties(visitor.factory(), element);
        let object_properties = if attrs.is_empty() {
            // When there are no attributes, React wants "null"
            visitor.new_keyword_expression(K::NullKeyword.into())
        } else {
            self.transform_jsx_attributes_to_object_props(visitor, &attrs, None)
        };

        let use_factory = self.import_specifier.borrow().is_empty();
        let callee = if use_factory {
            self.create_jsx_factory_expression(visitor, element)
        } else {
            self.get_implicit_import_for_name(visitor, b"createElement")
        };

        let new_children = self.visit_create_element_children(visitor, children);

        let mut args = Vec::with_capacity(new_children.len() + 2);
        args.push(tag_name);
        args.push(object_properties);
        args.extend(new_children);

        let arguments = new_node_list(visitor.factory_mut(), args);
        let result = visitor.new_call_expression(
            Some(callee),
            None,
            None,
            Some(arguments),
            node_flags::NONE,
        );
        visitor.set_node_range(result, location);

        if self.in_jsx_child.get() {
            self.context()
                .add_emit_flags(result, emit_flags::START_ON_NEW_LINE);
        }
        result
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.visitJsxOpeningFragmentCreateElement
    fn visit_jsx_opening_fragment_create_element(
        &self,
        visitor: &mut NodeVisitor<'_>,
        fragment: NodeId,
        children: Option<NodeListId>,
        location: TextRange,
    ) -> NodeId {
        let tag_name = self.create_jsx_fragment_factory_expression(visitor, fragment);
        let callee = self.create_jsx_factory_expression(visitor, fragment);

        let new_children = self.visit_create_element_children(visitor, children);

        let mut args = Vec::with_capacity(new_children.len() + 2);
        args.push(tag_name);
        args.push(visitor.new_keyword_expression(K::NullKeyword.into()));
        args.extend(new_children);

        let arguments = new_node_list(visitor.factory_mut(), args);
        let result = visitor.new_call_expression(
            Some(callee),
            None,
            None,
            Some(arguments),
            node_flags::NONE,
        );
        visitor.set_node_range(result, location);

        if self.in_jsx_child.get() {
            self.context()
                .add_emit_flags(result, emit_flags::START_ON_NEW_LINE);
        }
        result
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.visitJsxText
    #[allow(clippy::unused_self)] // upstream's method
    fn visit_jsx_text(&self, visitor: &mut NodeVisitor<'_>, text: NodeId) -> Option<NodeId> {
        let fixed = {
            let read = visitor.factory().node(text);
            fixup_whitespace_and_decode_entities(
                read.as_jsx_text().expect("JsxText payload").text(),
            )
        };
        if fixed.is_empty() {
            return None;
        }
        Some(visitor.new_string_literal(JsString::from_bytes(fixed), token_flags::NONE))
    }

    // port: tsc/internal/transformers/jsxtransforms/jsx.go:JSXTransformer.visitJsxExpression
    fn visit_jsx_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        expression: NodeId,
    ) -> Option<NodeId> {
        let (inner, dot_dot_dot_token) = {
            let read = visitor.factory().node(expression);
            let data = read.as_jsx_expression().expect("JsxExpression payload");
            (data.expression(), data.dot_dot_dot_token())
        };
        let e = self.visit(visitor, inner);
        if dot_dot_dot_token.is_some() {
            return Some(visitor.new_spread_element(e));
        }
        e
    }
}

/// `AsJsxExpression().DotDotDotToken`.
fn jsx_expression_dot_dot_dot(factory: &dyn RuntimeFactory, node: NodeId) -> Option<NodeId> {
    factory
        .node(node)
        .as_jsx_expression()
        .expect("JsxExpression payload")
        .dot_dot_dot_token()
}

/// The react jsx/jsxs transform falls back to `createElement` when an explicit `key` argument comes after a spread
// port: tsc/internal/transformers/jsxtransforms/jsx.go:hasKeyAfterPropsSpread
fn has_key_after_props_spread(factory: &dyn RuntimeFactory, node: NodeId) -> bool {
    let mut spread = false;
    let mut opener = node;
    if kind(factory, node) == K::JsxElement {
        opener = factory
            .node(node)
            .as_jsx_element()
            .expect("JsxElement payload")
            .opening_element()
            .expect(NIL);
    } // otherwise self-closing
    for elem in attribute_properties(factory, opener) {
        let elem_kind = kind(factory, elem);
        if elem_kind == K::JsxSpreadAttribute && {
            let expression = expression_of(factory, elem).expect(NIL);
            kind(factory, expression) != K::ObjectLiteralExpression
                || object_literal_properties(factory, expression)
                    .into_iter()
                    .any(|property| kind(factory, property) == K::SpreadAssignment)
        } {
            spread = true;
        } else if spread && elem_kind == K::JsxAttribute && {
            let name = name_of(factory, elem).expect(NIL);
            kind(factory, name) == K::Identifier
                && identifier_text(factory, name).as_bytes() == b"key"
        } {
            return true;
        }
    }
    false
}

// port: tsc/internal/transformers/jsxtransforms/jsx.go:insertStatementAfterPrologue
fn insert_statement_after_prologue(
    to: &mut Vec<NodeId>,
    statement: Option<NodeId>,
    is_prologue_directive: impl Fn(NodeId) -> bool,
) {
    let Some(statement) = statement else {
        return;
    };
    let mut statement_idx = 0;
    // skip all prologue directives to insert at the correct position
    while statement_idx < to.len() {
        if !is_prologue_directive(to[statement_idx]) {
            break;
        }
        statement_idx += 1;
    }
    to.insert(statement_idx, statement);
}

// port: tsc/internal/transformers/jsxtransforms/jsx.go:sortImportSpecifiers
fn sort_import_specifiers(
    factory: &dyn RuntimeFactory,
    a: NodeId,
    b: NodeId,
) -> std::cmp::Ordering {
    let names = |node: NodeId| {
        let read = factory.node(node);
        let data = read.as_import_specifier().expect("ImportSpecifier payload");
        (data.property_name().expect(NIL), data.name().expect(NIL))
    };
    let (a_property, a_name) = names(a);
    let (b_property, b_name) = names(b);
    let res = tsr_jsstring::compare::compare_case_sensitive(
        identifier_text(factory, a_property).as_bytes(),
        identifier_text(factory, b_property).as_bytes(),
    );
    if res != std::cmp::Ordering::Equal {
        return res;
    }
    tsr_jsstring::compare::compare_case_sensitive(
        identifier_text(factory, a_name).as_bytes(),
        identifier_text(factory, b_name).as_bytes(),
    )
}

// port: tsc/internal/transformers/jsxtransforms/jsx.go:getSortedSpecifiers
fn get_sorted_specifiers(factory: &dyn RuntimeFactory, m: &[(JsString, NodeId)]) -> Vec<NodeId> {
    let mut res: Vec<NodeId> = m.iter().map(|(_, specifier)| *specifier).collect();
    res.sort_by(|&a, &b| sort_import_specifiers(factory, a, b));
    res
}

// port: tsc/internal/transformers/jsxtransforms/jsx.go:hasProto
fn has_proto(factory: &dyn RuntimeFactory, obj: NodeId) -> bool {
    for p in object_literal_properties(factory, obj) {
        if kind(factory, p) != K::PropertyAssignment {
            continue;
        }
        let name = name_of(factory, p).expect(NIL);
        let text = {
            let read = factory.node(name);
            if let Some(data) = read.as_string_literal() {
                Some(data.text_owned())
            } else {
                read.as_identifier().map(|data| data.text_owned())
            }
        };
        if text.is_some_and(|text| text.as_bytes() == b"__proto__") {
            return true;
        }
    }
    false
}

// port: tsc/internal/transformers/jsxtransforms/jsx.go:addLineOfJsxText
fn add_line_of_jsx_text(b: &mut Vec<u8>, trimmed_line: &[u8], is_initial: bool) {
    // We do not escape the string here as that is handled by the printer
    // when it emits the literal. We do, however, need to decode JSX entities.
    let decoded = decode_entities(trimmed_line);
    if !is_initial {
        b.push(b' ');
    }
    b.extend_from_slice(&decoded);
}

/// JSX trims whitespace at the end and beginning of lines, except that the
/// start/end of a tag is considered a start/end of a line only if that line is
/// on the same line as the closing tag. See examples in
/// tests/cases/conformance/jsx/tsxReactEmitWhitespace.tsx
/// See also <https://www.w3.org/TR/html4/struct/text.html#h-9.1> and <https://www.w3.org/TR/CSS2/text.html#white-space-model>
///
/// An equivalent algorithm would be:
/// - If there is only one line, return it.
/// - If there is only whitespace (but multiple lines), return `undefined`.
/// - Split the text into lines.
/// - 'trimRight' the first line, 'trimLeft' the last line, 'trim' middle lines.
/// - Decode entities on each line (individually).
/// - Remove empty lines and join the rest with " ".
// port: tsc/internal/transformers/jsxtransforms/jsx.go:fixupWhitespaceAndDecodeEntities
fn fixup_whitespace_and_decode_entities(text: &[u8]) -> Vec<u8> {
    let mut acc = Vec::new();
    let mut initial = true;
    // First non-whitespace character on this line.
    let mut first_non_whitespace: Option<usize> = Some(0);
    // End byte position of the last non-whitespace character on this line.
    let mut last_non_whitespace_end: Option<usize> = None;
    // These initial values are special because the first line is:
    // firstNonWhitespace = 0 to indicate that we want leading whitespace,
    // but lastNonWhitespaceEnd = -1 as a special flag to indicate that we *don't* include the line if it's all whitespace.
    let mut i = 0;
    while i < text.len() {
        let (c, size) = decode_utf8(&text[i..]);
        if is_line_break(c) {
            // If we've seen any non-whitespace characters on this line, add the 'trim' of the line.
            // (lastNonWhitespaceEnd === -1 is a special flag to detect whether the first line is all whitespace.)
            if let (Some(first), Some(last)) = (first_non_whitespace, last_non_whitespace_end) {
                add_line_of_jsx_text(&mut acc, &text[first..=last], initial);
                initial = false;
            }

            // Reset firstNonWhitespace for the next line.
            // Don't bother to reset lastNonWhitespaceEnd because we ignore it if firstNonWhitespace = -1.
            first_non_whitespace = None;
        } else if !is_white_space_single_line(c) {
            // Store the end byte position of the character
            last_non_whitespace_end = Some(i + size - 1);
            if first_non_whitespace.is_none() {
                first_non_whitespace = Some(i);
            }
        }

        if size > 1 {
            i += size - 1;
        }
        i += 1;
    }

    if let Some(first) = first_non_whitespace {
        // Last line had a non-whitespace character. Emit the 'trimLeft', meaning keep trailing whitespace.
        add_line_of_jsx_text(&mut acc, &text[first..], initial);
    }
    acc
}

/// Replace entities like "&nbsp;", "&#123;", and "&#xDEADBEEF;" with the characters they encode.
/// See <https://en.wikipedia.org/wiki/List_of_XML_and_HTML_character_entity_references>
// port: tsc/internal/transformers/jsxtransforms/jsx.go:decodeEntities
fn decode_entities(text: &[u8]) -> Vec<u8> {
    let Some(mut i) = text.iter().position(|&byte| byte == b'&') else {
        return text.to_vec();
    };

    let mut text = text;
    let mut result = Vec::with_capacity(text.len());
    loop {
        result.extend_from_slice(&text[..i]);
        text = &text[i..];

        let Some(mut semi) = text.iter().position(|&byte| byte == b';') else {
            break;
        };

        // Skip past any intervening '&' characters between the current '&'
        // and the ';'. Each such '&' is not part of a valid entity, so emit
        // it (and any text before the next '&') as literals.
        while let Some(next_amp) = text[1..semi].iter().position(|&byte| byte == b'&') {
            result.extend_from_slice(&text[..=next_amp]);
            text = &text[next_amp + 1..];
            semi -= next_amp + 1;
        }

        let entity = &text[1..semi];
        if let Some(decoded) = decode_entity(entity) {
            // Use the JS-string encoder so lone surrogates (e.g. "&#xD800;")
            // are preserved rather than being lost to U+FFFD by WriteRune.
            result.extend_from_slice(&encode_rune(decoded));
        } else {
            result.extend_from_slice(&text[..=semi]);
        }
        text = &text[semi + 1..];

        match text.iter().position(|&byte| byte == b'&') {
            Some(next) => i = next,
            None => break,
        }
    }
    result.extend_from_slice(text);
    result
}

// port: tsc/internal/transformers/jsxtransforms/jsx.go:decodeEntity
fn decode_entity(entity: &[u8]) -> Option<i32> {
    if entity.is_empty() {
        return None;
    }

    if entity[0] == b'#' {
        let mut entity = &entity[1..];
        if entity.is_empty() {
            return None;
        }

        let mut base = 10;
        if entity[0] == b'x' {
            base = 16;
            entity = &entity[1..];
        }

        if entity.is_empty() {
            return None;
        }

        // Go ranges over runes; a non-ASCII rune is never a digit, and
        // neither is any byte of its encoding.
        for &c in entity {
            if base == 16 && !is_hex_digit(i32::from(c)) {
                return None;
            }
            if base == 10 && !is_digit(i32::from(c)) {
                return None;
            }
        }

        // strconv.ParseInt(entity, base, 32): only digits remain, so the only
        // failure is a value outside int32.
        let mut parsed: i64 = 0;
        for &c in entity {
            let digit = i64::from(char::from(c).to_digit(base).expect("checked digit"));
            parsed = parsed * i64::from(base) + digit;
            if parsed > i64::from(i32::MAX) {
                return None;
            }
        }
        return Some(i32::try_from(parsed).expect("checked range"));
    }

    entities(entity)
}

/// Upstream's `entities` map.
fn entities(entity: &[u8]) -> Option<i32> {
    Some(match entity {
        b"quot" => 0x0022,
        b"amp" => 0x0026,
        b"apos" => 0x0027,
        b"lt" => 0x003C,
        b"gt" => 0x003E,
        b"nbsp" => 0x00A0,
        b"iexcl" => 0x00A1,
        b"cent" => 0x00A2,
        b"pound" => 0x00A3,
        b"curren" => 0x00A4,
        b"yen" => 0x00A5,
        b"brvbar" => 0x00A6,
        b"sect" => 0x00A7,
        b"uml" => 0x00A8,
        b"copy" => 0x00A9,
        b"ordf" => 0x00AA,
        b"laquo" => 0x00AB,
        b"not" => 0x00AC,
        b"shy" => 0x00AD,
        b"reg" => 0x00AE,
        b"macr" => 0x00AF,
        b"deg" => 0x00B0,
        b"plusmn" => 0x00B1,
        b"sup2" => 0x00B2,
        b"sup3" => 0x00B3,
        b"acute" => 0x00B4,
        b"micro" => 0x00B5,
        b"para" => 0x00B6,
        b"middot" => 0x00B7,
        b"cedil" => 0x00B8,
        b"sup1" => 0x00B9,
        b"ordm" => 0x00BA,
        b"raquo" => 0x00BB,
        b"frac14" => 0x00BC,
        b"frac12" => 0x00BD,
        b"frac34" => 0x00BE,
        b"iquest" => 0x00BF,
        b"Agrave" => 0x00C0,
        b"Aacute" => 0x00C1,
        b"Acirc" => 0x00C2,
        b"Atilde" => 0x00C3,
        b"Auml" => 0x00C4,
        b"Aring" => 0x00C5,
        b"AElig" => 0x00C6,
        b"Ccedil" => 0x00C7,
        b"Egrave" => 0x00C8,
        b"Eacute" => 0x00C9,
        b"Ecirc" => 0x00CA,
        b"Euml" => 0x00CB,
        b"Igrave" => 0x00CC,
        b"Iacute" => 0x00CD,
        b"Icirc" => 0x00CE,
        b"Iuml" => 0x00CF,
        b"ETH" => 0x00D0,
        b"Ntilde" => 0x00D1,
        b"Ograve" => 0x00D2,
        b"Oacute" => 0x00D3,
        b"Ocirc" => 0x00D4,
        b"Otilde" => 0x00D5,
        b"Ouml" => 0x00D6,
        b"times" => 0x00D7,
        b"Oslash" => 0x00D8,
        b"Ugrave" => 0x00D9,
        b"Uacute" => 0x00DA,
        b"Ucirc" => 0x00DB,
        b"Uuml" => 0x00DC,
        b"Yacute" => 0x00DD,
        b"THORN" => 0x00DE,
        b"szlig" => 0x00DF,
        b"agrave" => 0x00E0,
        b"aacute" => 0x00E1,
        b"acirc" => 0x00E2,
        b"atilde" => 0x00E3,
        b"auml" => 0x00E4,
        b"aring" => 0x00E5,
        b"aelig" => 0x00E6,
        b"ccedil" => 0x00E7,
        b"egrave" => 0x00E8,
        b"eacute" => 0x00E9,
        b"ecirc" => 0x00EA,
        b"euml" => 0x00EB,
        b"igrave" => 0x00EC,
        b"iacute" => 0x00ED,
        b"icirc" => 0x00EE,
        b"iuml" => 0x00EF,
        b"eth" => 0x00F0,
        b"ntilde" => 0x00F1,
        b"ograve" => 0x00F2,
        b"oacute" => 0x00F3,
        b"ocirc" => 0x00F4,
        b"otilde" => 0x00F5,
        b"ouml" => 0x00F6,
        b"divide" => 0x00F7,
        b"oslash" => 0x00F8,
        b"ugrave" => 0x00F9,
        b"uacute" => 0x00FA,
        b"ucirc" => 0x00FB,
        b"uuml" => 0x00FC,
        b"yacute" => 0x00FD,
        b"thorn" => 0x00FE,
        b"yuml" => 0x00FF,
        b"OElig" => 0x0152,
        b"oelig" => 0x0153,
        b"Scaron" => 0x0160,
        b"scaron" => 0x0161,
        b"Yuml" => 0x0178,
        b"fnof" => 0x0192,
        b"circ" => 0x02C6,
        b"tilde" => 0x02DC,
        b"Alpha" => 0x0391,
        b"Beta" => 0x0392,
        b"Gamma" => 0x0393,
        b"Delta" => 0x0394,
        b"Epsilon" => 0x0395,
        b"Zeta" => 0x0396,
        b"Eta" => 0x0397,
        b"Theta" => 0x0398,
        b"Iota" => 0x0399,
        b"Kappa" => 0x039A,
        b"Lambda" => 0x039B,
        b"Mu" => 0x039C,
        b"Nu" => 0x039D,
        b"Xi" => 0x039E,
        b"Omicron" => 0x039F,
        b"Pi" => 0x03A0,
        b"Rho" => 0x03A1,
        b"Sigma" => 0x03A3,
        b"Tau" => 0x03A4,
        b"Upsilon" => 0x03A5,
        b"Phi" => 0x03A6,
        b"Chi" => 0x03A7,
        b"Psi" => 0x03A8,
        b"Omega" => 0x03A9,
        b"alpha" => 0x03B1,
        b"beta" => 0x03B2,
        b"gamma" => 0x03B3,
        b"delta" => 0x03B4,
        b"epsilon" => 0x03B5,
        b"zeta" => 0x03B6,
        b"eta" => 0x03B7,
        b"theta" => 0x03B8,
        b"iota" => 0x03B9,
        b"kappa" => 0x03BA,
        b"lambda" => 0x03BB,
        b"mu" => 0x03BC,
        b"nu" => 0x03BD,
        b"xi" => 0x03BE,
        b"omicron" => 0x03BF,
        b"pi" => 0x03C0,
        b"rho" => 0x03C1,
        b"sigmaf" => 0x03C2,
        b"sigma" => 0x03C3,
        b"tau" => 0x03C4,
        b"upsilon" => 0x03C5,
        b"phi" => 0x03C6,
        b"chi" => 0x03C7,
        b"psi" => 0x03C8,
        b"omega" => 0x03C9,
        b"thetasym" => 0x03D1,
        b"upsih" => 0x03D2,
        b"piv" => 0x03D6,
        b"ensp" => 0x2002,
        b"emsp" => 0x2003,
        b"thinsp" => 0x2009,
        b"zwnj" => 0x200C,
        b"zwj" => 0x200D,
        b"lrm" => 0x200E,
        b"rlm" => 0x200F,
        b"ndash" => 0x2013,
        b"mdash" => 0x2014,
        b"lsquo" => 0x2018,
        b"rsquo" => 0x2019,
        b"sbquo" => 0x201A,
        b"ldquo" => 0x201C,
        b"rdquo" => 0x201D,
        b"bdquo" => 0x201E,
        b"dagger" => 0x2020,
        b"Dagger" => 0x2021,
        b"bull" => 0x2022,
        b"hellip" => 0x2026,
        b"permil" => 0x2030,
        b"prime" => 0x2032,
        b"Prime" => 0x2033,
        b"lsaquo" => 0x2039,
        b"rsaquo" => 0x203A,
        b"oline" => 0x203E,
        b"frasl" => 0x2044,
        b"euro" => 0x20AC,
        b"image" => 0x2111,
        b"weierp" => 0x2118,
        b"real" => 0x211C,
        b"trade" => 0x2122,
        b"alefsym" => 0x2135,
        b"larr" => 0x2190,
        b"uarr" => 0x2191,
        b"rarr" => 0x2192,
        b"darr" => 0x2193,
        b"harr" => 0x2194,
        b"crarr" => 0x21B5,
        b"lArr" => 0x21D0,
        b"uArr" => 0x21D1,
        b"rArr" => 0x21D2,
        b"dArr" => 0x21D3,
        b"hArr" => 0x21D4,
        b"forall" => 0x2200,
        b"part" => 0x2202,
        b"exist" => 0x2203,
        b"empty" => 0x2205,
        b"nabla" => 0x2207,
        b"isin" => 0x2208,
        b"notin" => 0x2209,
        b"ni" => 0x220B,
        b"prod" => 0x220F,
        b"sum" => 0x2211,
        b"minus" => 0x2212,
        b"lowast" => 0x2217,
        b"radic" => 0x221A,
        b"prop" => 0x221D,
        b"infin" => 0x221E,
        b"ang" => 0x2220,
        b"and" => 0x2227,
        b"or" => 0x2228,
        b"cap" => 0x2229,
        b"cup" => 0x222A,
        b"int" => 0x222B,
        b"there4" => 0x2234,
        b"sim" => 0x223C,
        b"cong" => 0x2245,
        b"asymp" => 0x2248,
        b"ne" => 0x2260,
        b"equiv" => 0x2261,
        b"le" => 0x2264,
        b"ge" => 0x2265,
        b"sub" => 0x2282,
        b"sup" => 0x2283,
        b"nsub" => 0x2284,
        b"sube" => 0x2286,
        b"supe" => 0x2287,
        b"oplus" => 0x2295,
        b"otimes" => 0x2297,
        b"perp" => 0x22A5,
        b"sdot" => 0x22C5,
        b"lceil" => 0x2308,
        b"rceil" => 0x2309,
        b"lfloor" => 0x230A,
        b"rfloor" => 0x230B,
        b"lang" => 0x2329,
        b"rang" => 0x232A,
        b"loz" => 0x25CA,
        b"spades" => 0x2660,
        b"clubs" => 0x2663,
        b"hearts" => 0x2665,
        b"diams" => 0x2666,
        _ => return None,
    })
}

/// The text rewrites over inputs of the `jsx` probes, with the strings the
/// pin printed for them (`tests/fixtures/phase3/transforms/pending/jsx.native.json`,
/// cases `classic-entities-text`, `classic-entities-attr` and `classic-whitespace`).
#[cfg(test)]
mod tests {
    use super::{decode_entities, fixup_whitespace_and_decode_entities};

    #[test]
    fn entities_decode_as_the_pin_prints_them() {
        let text = "&amp; &lt;b&gt; &#123; &#x41; &nbsp;&copy; &bogus; &#; &#x; &&amp; &#99999999999; &#X41;";
        assert_eq!(
            fixup_whitespace_and_decode_entities(text.as_bytes()),
            "& <b> { A \u{a0}\u{a9} &bogus; &#; &#x; && &#99999999999; &#X41;".as_bytes()
        );
        assert_eq!(
            decode_entities(b"a &amp; b &quot;c&quot;"),
            b"a & b \"c\"".to_vec()
        );
        assert_eq!(decode_entities(b"it&apos;s"), b"it's".to_vec());
        // A lone surrogate keeps its WTF-8 encoding.
        assert_eq!(decode_entities(b"&#xD800;"), vec![0xed, 0xa0, 0x80]);
        assert_eq!(decode_entities(b"no entity"), b"no entity".to_vec());
        assert_eq!(decode_entities(b"&amp"), b"&amp".to_vec());
    }

    #[test]
    fn whitespace_is_trimmed_per_line_as_the_pin_prints_it() {
        let fixup = |text: &str| fixup_whitespace_and_decode_entities(text.as_bytes());
        assert_eq!(
            fixup("\n    leading\n      middle   words  \n\n    trailing   "),
            b"leading middle   words trailing   ".to_vec()
        );
        assert_eq!(fixup("   "), b"   ".to_vec());
        assert_eq!(fixup("\n\n"), Vec::<u8>::new());
        assert_eq!(fixup("  one line  "), b"  one line  ".to_vec());
    }
}
