//! `transformers/tstransforms/typeeraser.go`: erases the TypeScript-only
//! syntax of a file (type annotations, type-only declarations and imports,
//! accessibility modifiers, overloads, assertions) and leaves the runtime
//! constructs for the later transforms.
use super::runtimesyntax::get_innermost_module_declaration_from_dotted_module;
use crate::extract_modifiers;
use crate::transformer::{Failure, TransformOptions, Transformer};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use tsr_arena::Error;
use tsr_ast::evaluator::outer_expression_kinds;
use tsr_ast::utilities::{
    has_syntactic_modifier, is_assertion_expression, is_enum_const, is_jsdoc_type_assertion,
    is_parameter_property_declaration, is_statement, skip_outer_expressions,
};
use tsr_ast::utilities_class::{decorators, is_this_parameter};
use tsr_ast::utilities_middle::has_decorators;
use tsr_ast::{
    is_binary_expression, is_identifier, is_instantiated_module, is_satisfies_expression,
    modifier_flags, node_is_missing, subtree_flags, AstView, FactoryMethods, NodeId, NodeSlice,
    NodeVisitor, SyntaxKind as K,
};
use tsr_core::{CompilerOptions, TextRange};
use tsr_printer::EmitContext;

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// `TypeEraserTransformer`.
struct TypeEraserTransformer {
    compiler_options: Arc<CompilerOptions>,
    emit_context: EmitContext,
    failure: Failure,
    parent_node: Cell<Option<NodeId>>,
    current_node: Cell<Option<NodeId>>,
}

/// `NewTypeEraserTransformer`.
// port: tsc/internal/transformers/tstransforms/typeeraser.go:NewTypeEraserTransformer
pub fn new_type_eraser_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let compiler_options = Arc::clone(&opts.compiler_options);
    let emit_context = opts.context.clone();
    let tx = Rc::new(TypeEraserTransformer {
        compiler_options,
        emit_context: emit_context.clone(),
        failure: opts.failure.clone(),
        parent_node: Cell::new(None),
        current_node: Cell::new(None),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            if tx.failure.is_set() {
                return node;
            }
            match tx.visit(visitor, node) {
                Ok(visited) => visited,
                Err(error) => {
                    tx.failure.record(error);
                    node
                }
            }
        },
        Some(emit_context),
        opts.failure.clone(),
    ))
}

/// The parsed view of the visitor's factory, which the AST predicates read.
fn view<'v>(visitor: &'v NodeVisitor<'_>) -> AstView<'v> {
    visitor
        .factory()
        .ast_view()
        .expect("the type eraser runs over a factory with complete storage")
}

/// Restores the ancestor tracking on every exit of `visit`, as upstream's
/// deferred `popNode`.
struct PopNode<'t> {
    tx: &'t TypeEraserTransformer,
    grandparent_node: Option<NodeId>,
}

impl Drop for PopNode<'_> {
    fn drop(&mut self) {
        self.tx.pop_node(self.grandparent_node);
    }
}

/// The `UpdatePropertyDeclaration` of both property branches of `visit`.
fn update_property_declaration(visitor: &mut NodeVisitor<'_>, id: NodeId) -> NodeId {
    let n = visitor
        .factory()
        .node(id)
        .data_source()
        .as_property_declaration()
        .expect("interface conversion: ast.nodeData is not *ast.PropertyDeclaration")
        .to_owned();
    let modifiers = visitor.visit_modifiers(n.modifiers);
    let name = visitor.visit_node(n.name);
    let initializer = visitor.visit_node(n.initializer);
    visitor
        .factory_mut()
        .update_property_declaration(id, modifiers, name, None, None, initializer)
}

/// A missing body (`ast.NodeIsMissing`).
fn body_is_missing(visitor: &NodeVisitor<'_>, body: Option<NodeId>) -> bool {
    node_is_missing(body.map(|body| visitor.factory().node(body)).as_ref())
}

/// The empty block upstream gives an accessor without a body:
/// `NewBlock(NewNodeList(nil), false)`.
fn new_empty_block(visitor: &mut NodeVisitor<'_>) -> NodeId {
    let factory = visitor.factory_mut();
    let statements = factory.alloc_list(TextRange::new(-1, -1), NodeSlice::empty());
    factory.new_block(Some(statements), false)
}

impl TypeEraserTransformer {
    /// Pushes a new child node onto the ancestor tracking stack, returning the
    /// grandparent node to be restored later via `pop_node`.
    // port: tsc/internal/transformers/tstransforms/typeeraser.go:TypeEraserTransformer.pushNode
    fn push_node(&self, node: NodeId) -> Option<NodeId> {
        let grandparent_node = self.parent_node.get();
        self.parent_node.set(self.current_node.get());
        self.current_node.set(Some(node));
        grandparent_node
    }

    /// Pops the last child node off the ancestor tracking stack, restoring the
    /// grandparent node.
    // port: tsc/internal/transformers/tstransforms/typeeraser.go:TypeEraserTransformer.popNode
    fn pop_node(&self, grandparent_node: Option<NodeId>) {
        self.current_node.set(self.parent_node.get());
        self.parent_node.set(grandparent_node);
    }

    // port: tsc/internal/transformers/tstransforms/typeeraser.go:TypeEraserTransformer.elide
    fn elide(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        self.emit_context
            .clone()
            .new_not_emitted_statement(visitor.factory_mut(), node)
    }

    /// `PartiallyEmittedExpression` over the visited `expression`, with the
    /// original and the location of `node`.
    fn partially_emit(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        expression: Option<NodeId>,
    ) -> NodeId {
        let visited = visitor.visit_node(expression);
        let partial = visitor
            .factory_mut()
            .new_partially_emitted_expression(visited);
        self.emit_context.clone().set_original(partial, node);
        let loc = visitor.factory().node(node).range();
        visitor.factory_mut().set_node_range(partial, loc);
        partial
    }

    // port: tsc/internal/transformers/tstransforms/typeeraser.go:TypeEraserTransformer.visit
    // Upstream's switch keeps one case per construct, some with equal bodies.
    #[allow(clippy::too_many_lines, clippy::match_same_arms)]
    fn visit(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        let id = node.expect(NIL);
        if view(visitor).subtree_facts(id) & subtree_flags::TYPE_SCRIPT == 0 {
            return Ok(node);
        }

        if is_statement(view(visitor), id)?
            && has_syntactic_modifier(view(visitor), id, modifier_flags::AMBIENT)?
        {
            return Ok(Some(self.elide(visitor, id)));
        }

        let grandparent_node = self.push_node(id);
        let _pop = PopNode {
            tx: self,
            grandparent_node,
        };

        let kind = visitor.factory().node(id).kind();
        match kind.known() {
            Some(
                // TypeScript accessibility and readonly modifiers are elided
                K::PublicKeyword
                | K::PrivateKeyword
                | K::ProtectedKeyword
                | K::AbstractKeyword
                | K::OverrideKeyword
                | K::ConstKeyword
                | K::DeclareKeyword
                | K::ReadonlyKeyword
                // TypeScript type nodes are elided.
                | K::ArrayType
                | K::TupleType
                | K::OptionalType
                | K::RestType
                | K::TypeLiteral
                | K::TypePredicate
                | K::TypeParameter
                | K::AnyKeyword
                | K::UnknownKeyword
                | K::BooleanKeyword
                | K::StringKeyword
                | K::NumberKeyword
                | K::NeverKeyword
                | K::VoidKeyword
                | K::SymbolKeyword
                | K::ConstructorType
                | K::FunctionType
                | K::TypeQuery
                | K::TypeReference
                | K::UnionType
                | K::IntersectionType
                | K::ConditionalType
                | K::ParenthesizedType
                | K::ThisType
                | K::TypeOperator
                | K::IndexedAccessType
                | K::MappedType
                | K::LiteralType
                // TypeScript index signatures are elided.
                | K::IndexSignature,
            ) => Ok(None),

            Some(K::InKeyword | K::OutKeyword) => {
                // TypeScript `in`/`out` variance modifiers are elided. These keywords are only
                // meaningful as modifiers on type parameters (which are themselves elided), but they may
                // appear as a grammar error on other declarations and must not leak into the emitted JS.
                // The `in` binary operator shares this token kind, so only elide when used as a modifier.
                if self
                    .parent_node
                    .get()
                    .is_none_or(|parent| !is_binary_expression(&visitor.factory().node(parent)))
                {
                    return Ok(None);
                }
                Ok(visitor.visit_each_child(node))
            }

            // reparsed commonjs are elided
            Some(K::JSImportDeclaration) => Ok(None),
            Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration | K::InterfaceDeclaration) => {
                // TypeScript type-only declarations are elided.
                Ok(Some(self.elide(visitor, id)))
            }

            // TypeScript namespace export declarations are elided.
            Some(K::NamespaceExportDeclaration) => Ok(None),

            Some(K::ModuleDeclaration) => {
                let name = visitor.factory().node(id).name().expect(NIL);
                if !is_identifier(&visitor.factory().node(name))
                    || !is_instantiated_module(
                        view(visitor),
                        id,
                        self.compiler_options.should_preserve_const_enums(),
                    )?
                    || visitor
                        .factory()
                        .node(get_innermost_module_declaration_from_dotted_module(
                            visitor.factory(),
                            id,
                        ))
                        .body()
                        .is_none()
                {
                    // TypeScript module declarations are elided if they are not instantiated or have no body
                    return Ok(Some(self.elide(visitor, id)));
                }
                Ok(visitor.visit_each_child(node))
            }

            Some(K::ExpressionWithTypeArguments) => {
                let expression = visitor.factory().node(id).expression();
                let expression = visitor.visit_node(expression);
                Ok(Some(
                    visitor
                        .factory_mut()
                        .update_expression_with_type_arguments(id, expression, None),
                ))
            }

            Some(K::PropertyDeclaration) => {
                if self.compiler_options.experimental_decorators.is_true()
                    && has_syntactic_modifier(
                        view(visitor),
                        id,
                        modifier_flags::AMBIENT | modifier_flags::ABSTRACT,
                    )?
                    && has_decorators(view(visitor), &visitor.factory().node(id))?
                {
                    // declare/abstract props with decorators must be preserved until the decorator transform can process them and remove them
                    return Ok(Some(update_property_declaration(visitor, id)));
                }
                if has_syntactic_modifier(
                    view(visitor),
                    id,
                    modifier_flags::AMBIENT | modifier_flags::ABSTRACT,
                )? {
                    // TypeScript `declare` fields are elided
                    return Ok(None);
                }
                Ok(Some(update_property_declaration(visitor, id)))
            }

            Some(K::Constructor) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_constructor_declaration()
                    .expect("interface conversion: ast.nodeData is not *ast.ConstructorDeclaration")
                    .to_owned();
                if body_is_missing(visitor, n.body) {
                    // TypeScript overloads are elided
                    return Ok(None);
                }
                let parameters = visitor.visit_nodes(n.parameters);
                let body = visitor.visit_node(n.body);
                Ok(Some(visitor.factory_mut().update_constructor_declaration(
                    id, None, None, parameters, None, None, body,
                )))
            }

            Some(K::MethodDeclaration) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_method_declaration()
                    .expect("interface conversion: ast.nodeData is not *ast.MethodDeclaration")
                    .to_owned();
                if body_is_missing(visitor, n.body) {
                    // TypeScript overloads are elided
                    return Ok(None);
                }
                let modifiers = visitor.visit_modifiers(n.modifiers);
                let name = visitor.visit_node(n.name);
                let parameters = visitor.visit_nodes(n.parameters);
                let body = visitor.visit_node(n.body);
                Ok(Some(visitor.factory_mut().update_method_declaration(
                    id,
                    modifiers,
                    n.asterisk_token,
                    name,
                    None,
                    None,
                    parameters,
                    None,
                    None,
                    body,
                )))
            }

            Some(K::GetAccessor) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_get_accessor_declaration()
                    .expect("interface conversion: ast.nodeData is not *ast.GetAccessorDeclaration")
                    .to_owned();
                if body_is_missing(visitor, n.body)
                    && has_syntactic_modifier(view(visitor), id, modifier_flags::ABSTRACT)?
                {
                    // Abstract accessors are elided
                    return Ok(None);
                }
                let mut body = visitor.visit_node(n.body);
                if body.is_none() {
                    body = Some(new_empty_block(visitor));
                }
                let modifiers = visitor.visit_modifiers(n.modifiers);
                let name = visitor.visit_node(n.name);
                let parameters = visitor.visit_nodes(n.parameters);
                Ok(Some(visitor.factory_mut().update_get_accessor_declaration(
                    id, modifiers, name, None, parameters, None, None, body,
                )))
            }

            Some(K::SetAccessor) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_set_accessor_declaration()
                    .expect("interface conversion: ast.nodeData is not *ast.SetAccessorDeclaration")
                    .to_owned();
                if body_is_missing(visitor, n.body)
                    && has_syntactic_modifier(view(visitor), id, modifier_flags::ABSTRACT)?
                {
                    // Abstract accessors are elided
                    return Ok(None);
                }
                let mut body = visitor.visit_node(n.body);
                if body.is_none() {
                    body = Some(new_empty_block(visitor));
                }
                let modifiers = visitor.visit_modifiers(n.modifiers);
                let name = visitor.visit_node(n.name);
                let parameters = visitor.visit_nodes(n.parameters);
                Ok(Some(visitor.factory_mut().update_set_accessor_declaration(
                    id, modifiers, name, None, parameters, None, None, body,
                )))
            }

            Some(K::VariableDeclaration) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_variable_declaration()
                    .expect("interface conversion: ast.nodeData is not *ast.VariableDeclaration")
                    .to_owned();
                let name = visitor.visit_node(n.name);
                let initializer = visitor.visit_node(n.initializer);
                let updated = visitor
                    .factory_mut()
                    .update_variable_declaration(id, name, None, None, initializer);
                if n.r#type.is_some() {
                    let updated_name = visitor.factory().node(updated).name().expect(NIL);
                    self.emit_context
                        .clone()
                        .set_type_node(updated_name, n.r#type);
                }
                Ok(Some(updated))
            }

            Some(K::HeritageClause) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_heritage_clause()
                    .expect("interface conversion: ast.nodeData is not *ast.HeritageClause")
                    .to_owned();
                if n.token == K::ImplementsKeyword {
                    // TypeScript `implements` clauses are elided
                    return Ok(None);
                }
                let types = visitor.visit_nodes(n.types);
                Ok(Some(
                    visitor
                        .factory_mut()
                        .update_heritage_clause(id, n.token, types),
                ))
            }

            Some(K::ClassDeclaration) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_class_declaration()
                    .expect("interface conversion: ast.nodeData is not *ast.ClassDeclaration")
                    .to_owned();
                let modifiers = visitor.visit_modifiers(n.modifiers);
                let name = visitor.visit_node(n.name);
                let heritage_clauses = visitor.visit_nodes(n.heritage_clauses);
                let members = visitor.visit_nodes(n.members);
                Ok(Some(visitor.factory_mut().update_class_declaration(
                    id,
                    modifiers,
                    name,
                    None,
                    heritage_clauses,
                    members,
                )))
            }

            Some(K::ClassExpression) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_class_expression()
                    .expect("interface conversion: ast.nodeData is not *ast.ClassExpression")
                    .to_owned();
                let modifiers = visitor.visit_modifiers(n.modifiers);
                let name = visitor.visit_node(n.name);
                let heritage_clauses = visitor.visit_nodes(n.heritage_clauses);
                let members = visitor.visit_nodes(n.members);
                Ok(Some(visitor.factory_mut().update_class_expression(
                    id,
                    modifiers,
                    name,
                    None,
                    heritage_clauses,
                    members,
                )))
            }

            Some(K::FunctionDeclaration) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_function_declaration()
                    .expect("interface conversion: ast.nodeData is not *ast.FunctionDeclaration")
                    .to_owned();
                if body_is_missing(visitor, n.body) {
                    // TypeScript overloads are elided
                    return Ok(Some(self.elide(visitor, id)));
                }
                let modifiers = visitor.visit_modifiers(n.modifiers);
                let name = visitor.visit_node(n.name);
                let parameters = visitor.visit_nodes(n.parameters);
                let body = visitor.visit_node(n.body);
                Ok(Some(visitor.factory_mut().update_function_declaration(
                    id,
                    modifiers,
                    n.asterisk_token,
                    name,
                    None,
                    parameters,
                    None,
                    None,
                    body,
                )))
            }

            Some(K::FunctionExpression) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_function_expression()
                    .expect("interface conversion: ast.nodeData is not *ast.FunctionExpression")
                    .to_owned();
                let modifiers = visitor.visit_modifiers(n.modifiers);
                let name = visitor.visit_node(n.name);
                let parameters = visitor.visit_nodes(n.parameters);
                let body = visitor.visit_node(n.body);
                Ok(Some(visitor.factory_mut().update_function_expression(
                    id,
                    modifiers,
                    n.asterisk_token,
                    name,
                    None,
                    parameters,
                    None,
                    None,
                    body,
                )))
            }

            Some(K::ArrowFunction) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_arrow_function()
                    .expect("interface conversion: ast.nodeData is not *ast.ArrowFunction")
                    .to_owned();
                let modifiers = visitor.visit_modifiers(n.modifiers);
                let parameters = visitor.visit_nodes(n.parameters);
                let body = visitor.visit_node(n.body);
                Ok(Some(visitor.factory_mut().update_arrow_function(
                    id,
                    modifiers,
                    None,
                    parameters,
                    None,
                    None,
                    n.equals_greater_than_token,
                    body,
                )))
            }

            Some(K::Parameter) => {
                if is_this_parameter(view(visitor), id)? {
                    // TypeScript `this` parameters are elided
                    return Ok(None);
                }
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_parameter_declaration()
                    .expect("interface conversion: ast.nodeData is not *ast.ParameterDeclaration")
                    .to_owned();
                // preserve parameter property modifiers to be handled by the runtime transformer
                let mut modifiers = None;
                let is_parameter_property = if let Some(parent) = self.parent_node.get() {
                    is_parameter_property_declaration(view(visitor), id, parent)?
                } else {
                    // A nil parent is dereferenced only once the first two tests hold.
                    assert!(
                        !has_syntactic_modifier(
                            view(visitor),
                            id,
                            modifier_flags::PARAMETER_PROPERTY_MODIFIER,
                        )?,
                        "{NIL}"
                    );
                    false
                };
                if is_parameter_property {
                    modifiers = extract_modifiers(
                        &self.emit_context,
                        visitor.factory_mut(),
                        n.modifiers,
                        modifier_flags::PARAMETER_PROPERTY_MODIFIER,
                    );
                }
                // preserve decorators for the decorator transforms
                if has_decorators(view(visitor), &visitor.factory().node(id))? {
                    let decorators = decorators(view(visitor), id)?;
                    let decorators = visitor
                        .factory_mut()
                        .alloc_nodes(decorators.into_iter().map(Some).collect());
                    let (visited, _) = visitor.visit_slice(decorators);
                    let factory = visitor.factory_mut();
                    modifiers = Some(match modifiers {
                        None => factory.new_modifier_list(visited),
                        Some(modifiers) => {
                            let existing = factory.read_list(modifiers).nodes();
                            let mut nodes: Vec<Option<NodeId>> =
                                factory.read_nodes(existing).iter().collect();
                            nodes.extend(factory.read_nodes(visited).iter());
                            let nodes = factory.alloc_nodes(nodes);
                            factory.new_modifier_list(nodes)
                        }
                    });
                }
                let name = visitor.visit_node(n.name);
                let initializer = visitor.visit_node(n.initializer);
                Ok(Some(visitor.factory_mut().update_parameter_declaration(
                    id,
                    modifiers,
                    n.dot_dot_dot_token,
                    name,
                    None,
                    None,
                    initializer,
                )))
            }

            Some(K::CallExpression) => {
                let (n, flags) = {
                    let read = visitor.factory().node(id);
                    let data = read
                        .data_source()
                        .as_call_expression()
                        .expect("interface conversion: ast.nodeData is not *ast.CallExpression")
                        .to_owned();
                    (data, read.flags())
                };
                let expression = visitor.visit_node(n.expression);
                let arguments = visitor.visit_nodes(n.arguments);
                Ok(Some(visitor.factory_mut().update_call_expression(
                    id,
                    expression,
                    n.question_dot_token,
                    None,
                    arguments,
                    flags,
                )))
            }

            Some(K::NewExpression) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_new_expression()
                    .expect("interface conversion: ast.nodeData is not *ast.NewExpression")
                    .to_owned();
                let expression = visitor.visit_node(n.expression);
                let arguments = visitor.visit_nodes(n.arguments);
                Ok(Some(visitor.factory_mut().update_new_expression(
                    id, expression, None, arguments,
                )))
            }

            Some(K::TaggedTemplateExpression) => {
                let (n, flags) = {
                    let read = visitor.factory().node(id);
                    let data = read
                        .data_source()
                        .as_tagged_template_expression()
                        .expect(
                            "interface conversion: ast.nodeData is not *ast.TaggedTemplateExpression",
                        )
                        .to_owned();
                    (data, read.flags())
                };
                let tag = visitor.visit_node(n.tag);
                let template = visitor.visit_node(n.template);
                Ok(Some(visitor.factory_mut().update_tagged_template_expression(
                    id,
                    tag,
                    n.question_dot_token,
                    None,
                    template,
                    flags,
                )))
            }

            Some(
                K::NonNullExpression
                | K::TypeAssertionExpression
                | K::AsExpression
                | K::SatisfiesExpression,
            ) => {
                let expression = visitor.factory().node(id).expression();
                Ok(Some(self.partially_emit(visitor, id, expression)))
            }

            Some(K::ParenthesizedExpression) => {
                if !is_jsdoc_type_assertion(view(visitor), Some(id))? {
                    let inner = visitor.factory().node(id).expression();
                    let expression = skip_outer_expressions(
                        view(visitor),
                        inner.expect(NIL),
                        outer_expression_kinds::ALL_EXCEPT_ASSERTIONS_OR_EXPRESSIONS_WITH_TYPE_ARGUMENTS,
                    )?;
                    let read = visitor.factory().node(expression);
                    if is_assertion_expression(&read) || is_satisfies_expression(&read) {
                        drop(read);
                        return Ok(Some(self.partially_emit(visitor, id, inner)));
                    }
                }
                Ok(visitor.visit_each_child(node))
            }

            Some(K::JsxSelfClosingElement) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_jsx_self_closing_element()
                    .expect("interface conversion: ast.nodeData is not *ast.JsxSelfClosingElement")
                    .to_owned();
                let tag_name = visitor.visit_node(n.tag_name);
                let attributes = visitor.visit_node(n.attributes);
                Ok(Some(visitor.factory_mut().update_jsx_self_closing_element(
                    id, tag_name, None, attributes,
                )))
            }

            Some(K::JsxOpeningElement) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_jsx_opening_element()
                    .expect("interface conversion: ast.nodeData is not *ast.JsxOpeningElement")
                    .to_owned();
                let tag_name = visitor.visit_node(n.tag_name);
                let attributes = visitor.visit_node(n.attributes);
                Ok(Some(visitor.factory_mut().update_jsx_opening_element(
                    id, tag_name, None, attributes,
                )))
            }

            Some(K::ImportEqualsDeclaration) => {
                if visitor.factory().node(id).is_type_only() {
                    // elide type-only imports
                    return Ok(None);
                }
                Ok(visitor.visit_each_child(node))
            }

            Some(K::ImportDeclaration) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_import_declaration()
                    .expect("interface conversion: ast.nodeData is not *ast.ImportDeclaration")
                    .to_owned();
                if n.import_clause.is_none() {
                    // Do not elide a side-effect only import declaration.
                    //  import "foo";
                    return Ok(node);
                }
                let import_clause = visitor.visit_node(n.import_clause);
                if import_clause.is_none() {
                    return Ok(None);
                }
                Ok(Some(visitor.factory_mut().update_import_declaration(
                    id,
                    n.modifiers,
                    import_clause,
                    n.module_specifier,
                    n.attributes,
                )))
            }

            Some(K::ImportClause) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_import_clause()
                    .expect("interface conversion: ast.nodeData is not *ast.ImportClause")
                    .to_owned();
                if visitor.factory().node(id).is_type_only() {
                    // Always elide type-only imports
                    return Ok(None);
                }
                let name = n.name;
                let named_bindings = visitor.visit_node(n.named_bindings);
                if name.is_none() && named_bindings.is_none() {
                    // all import bindings were elided
                    return Ok(None);
                }
                Ok(Some(visitor.factory_mut().update_import_clause(
                    id,
                    n.phase_modifier,
                    name,
                    named_bindings,
                )))
            }

            Some(K::NamedImports) => {
                let elements = visitor.factory().node(id).element_list().expect(NIL);
                if visitor.factory().read_list(elements).nodes().is_empty() {
                    // Do not elide a side-effect only import declaration.
                    return Ok(node);
                }
                let elements = visitor.visit_nodes(Some(elements));
                if !self.compiler_options.verbatim_module_syntax.is_true()
                    && visitor
                        .factory()
                        .read_list(elements.expect(NIL))
                        .nodes()
                        .is_empty()
                {
                    // all import specifiers were elided
                    return Ok(None);
                }
                Ok(Some(
                    visitor.factory_mut().update_named_imports(id, elements),
                ))
            }

            Some(K::ImportSpecifier) => {
                if visitor.factory().node(id).is_type_only() {
                    // elide type-only or unused imports
                    return Ok(None);
                }
                Ok(node)
            }

            Some(K::ExportDeclaration) => {
                let n = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_export_declaration()
                    .expect("interface conversion: ast.nodeData is not *ast.ExportDeclaration")
                    .to_owned();
                if n.is_type_only {
                    // elide type-only exports
                    return Ok(None);
                }
                let mut export_clause = None;
                if n.export_clause.is_some() {
                    export_clause = visitor.visit_node(n.export_clause);
                    if export_clause.is_none() {
                        // all export bindings were elided
                        return Ok(None);
                    }
                }
                let module_specifier = visitor.visit_node(n.module_specifier);
                let attributes = visitor.visit_node(n.attributes);
                Ok(Some(visitor.factory_mut().update_export_declaration(
                    id,
                    None,  /*modifiers*/
                    false, /*isTypeOnly*/
                    export_clause,
                    module_specifier,
                    attributes,
                )))
            }

            Some(K::NamedExports) => {
                let elements = visitor.factory().node(id).element_list().expect(NIL);
                if visitor.factory().read_list(elements).nodes().is_empty() {
                    // Do not elide an empty export declaration.
                    return Ok(node);
                }

                let elements = visitor.visit_nodes(Some(elements));
                if !self.compiler_options.verbatim_module_syntax.is_true()
                    && visitor
                        .factory()
                        .read_list(elements.expect(NIL))
                        .nodes()
                        .is_empty()
                {
                    // all export specifiers were elided
                    return Ok(None);
                }
                Ok(Some(
                    visitor.factory_mut().update_named_exports(id, elements),
                ))
            }

            Some(K::ExportSpecifier) => {
                if visitor.factory().node(id).is_type_only() {
                    // elide unused export
                    return Ok(None);
                }
                Ok(node)
            }

            Some(K::EnumDeclaration) => {
                if is_enum_const(view(visitor), id)? {
                    return Ok(node);
                }
                Ok(visitor.visit_each_child(node))
            }

            _ => Ok(visitor.visit_each_child(node)),
        }
    }
}

#[cfg(test)]
#[path = "typeeraser_tests.rs"]
mod tests;
