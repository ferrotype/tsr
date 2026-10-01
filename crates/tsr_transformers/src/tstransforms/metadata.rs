//! `transformers/tstransforms/metadata.go`: the `__metadata` decorators that
//! `emitDecoratorMetadata` adds to legacy-decorated classes and members.
use super::typeserializer::{
    list_nodes, new_node_list, view, MetadataSerializer, MetadataSerializerContext,
};
use crate::transformer::{Error, Failure, SharedEmitResolver, TransformOptions, Transformer};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tsr_ast::{
    subtree_flags, FactoryMethods, JsString, NodeId, NodeListId, NodeVisitor, RuntimeFactory,
    SyntaxKind as K,
};
use tsr_core::ScriptTarget;
use tsr_printer::EmitContext;

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

const USE_NEW_TYPE_METADATA_FORMAT: bool = false;

struct MetadataTransformer<'a> {
    legacy_decorators: bool,
    resolver: SharedEmitResolver<'a>,
    emit_context: EmitContext,
    failure: Failure,

    serializer: RefCell<Option<Rc<MetadataSerializer<'a>>>>,
    language_version: ScriptTarget,
    strict_null_checks: bool,
    parent: Cell<Option<NodeId>>,
    current_lexical_scope: Cell<Option<NodeId>>,
}

// port: tsc/internal/transformers/tstransforms/metadata.go:NewMetadataTransformer
pub fn new_metadata_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let options = &opts.compiler_options;
    let tx = Rc::new(MetadataTransformer {
        legacy_decorators: options.experimental_decorators.is_true(),
        resolver: opts.emit_resolver.clone(),
        emit_context: opts.context.clone(),
        failure: opts.failure.clone(),
        serializer: RefCell::new(None),
        language_version: options.emit_script_target(),
        strict_null_checks: options.strict_option_value(options.strict_null_checks),
        parent: Cell::new(None),
        current_lexical_scope: Cell::new(None),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            let id = node.expect(NIL);
            if tx.failure.is_set() {
                return node;
            }
            match tx.visit(visitor, id) {
                Ok(result) => result,
                Err(error) => {
                    tx.failure.record(error);
                    node
                }
            }
        },
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}

/// The fields of a class declaration or expression.
struct ClassParts {
    modifiers: Option<NodeListId>,
    name: Option<NodeId>,
    type_parameters: Option<NodeListId>,
    heritage_clauses: Option<NodeListId>,
    members: Option<NodeListId>,
}

/// The fields of a method or accessor declaration.
struct FunctionParts {
    modifiers: Option<NodeListId>,
    asterisk_token: Option<NodeId>,
    name: Option<NodeId>,
    postfix_token: Option<NodeId>,
    type_parameters: Option<NodeListId>,
    parameters: Option<NodeListId>,
    type_node: Option<NodeId>,
    full_signature: Option<NodeId>,
    body: Option<NodeId>,
}

fn function_parts(factory: &dyn RuntimeFactory, node: NodeId) -> FunctionParts {
    let read = factory.node(node);
    let source = read.data_source();
    let (modifiers, asterisk_token, name, postfix_token, type_parameters, parameters) =
        if let Some(d) = source.as_method_declaration() {
            (
                d.modifiers(),
                d.asterisk_token(),
                d.name(),
                d.postfix_token(),
                d.type_parameters(),
                d.parameters(),
            )
        } else if let Some(d) = source.as_set_accessor_declaration() {
            (
                d.modifiers(),
                d.asterisk_token(),
                d.name(),
                d.postfix_token(),
                d.type_parameters(),
                d.parameters(),
            )
        } else {
            let d = source
                .as_get_accessor_declaration()
                .expect("GetAccessorDeclaration payload");
            (
                d.modifiers(),
                d.asterisk_token(),
                d.name(),
                d.postfix_token(),
                d.type_parameters(),
                d.parameters(),
            )
        };
    let (full_signature, body) = if let Some(d) = source.as_method_declaration() {
        (d.full_signature(), d.body())
    } else if let Some(d) = source.as_set_accessor_declaration() {
        (d.full_signature(), d.body())
    } else {
        let d = source
            .as_get_accessor_declaration()
            .expect("GetAccessorDeclaration payload");
        (d.full_signature(), d.body())
    };
    FunctionParts {
        modifiers,
        asterisk_token,
        name,
        postfix_token,
        type_parameters,
        parameters,
        type_node: read.type_node(),
        full_signature,
        body,
    }
}

/// Whether any parameter of a method or set accessor (after a `this`
/// parameter) is decorated: `len(getDecoratorsOfParameters(node)) != 0`.
// TODO(legacydecorators): tstransforms.getDecoratorsOfParameters, ported with
// the legacy decorators transformer; this is its length.
fn has_decorators_of_parameters(factory: &dyn RuntimeFactory, node: NodeId) -> Result<bool, Error> {
    let view = view(factory)?;
    let parameters = list_nodes(view, view.node(node)?.parameter_list())?;
    let first_parameter_is_this =
        !parameters.is_empty() && tsr_ast::utilities_class::is_this_parameter(view, parameters[0])?;
    let first_parameter_offset = usize::from(first_parameter_is_this);
    for &p in &parameters[first_parameter_offset..] {
        if tsr_ast::utilities_middle::has_decorators(view, &view.node(p)?)? {
            return Ok(true);
        }
    }
    Ok(false)
}

impl<'a> MetadataTransformer<'a> {
    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.visit
    fn visit(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Result<Option<NodeId>, Error> {
        if view(v.factory())?.subtree_facts(node) & subtree_flags::DECORATORS == 0 {
            return Ok(Some(node));
        }

        let kind = v.factory().node(node).kind();
        match kind.known() {
            Some(K::ClassDeclaration) => self.visit_class_declaration(v, node),
            Some(K::ClassExpression) => self.visit_class_expression(v, node),
            Some(K::PropertyDeclaration) => self.visit_property_declaration(v, node),
            Some(K::MethodDeclaration) => self.visit_method_declaration(v, node),
            Some(K::SetAccessor) => self.visit_set_accessor(v, node),
            Some(K::GetAccessor) => self.visit_get_accessor(v, node),
            Some(K::SourceFile) => {
                self.parent.set(None);
                self.current_lexical_scope.set(Some(node));
                *self.serializer.borrow_mut() = Some(Rc::new(MetadataSerializer::new(
                    self.resolver.clone(),
                    self.emit_context.clone(),
                    self.language_version,
                    self.strict_null_checks,
                )));
                let updated = v.visit_each_child(Some(node));
                let mut ec = self.emit_context.clone();
                let helpers = ec.read_emit_helpers();
                ec.add_emit_helper(updated.expect(NIL), &helpers);
                self.set_current_lexical_scope(None);
                self.set_parent(None);
                Ok(updated)
            }
            Some(K::ModuleBlock | K::Block | K::CaseBlock) => {
                let old_scope = self.current_lexical_scope.replace(Some(node));
                let result = v.visit_each_child(Some(node));
                self.set_current_lexical_scope(old_scope);
                Ok(result)
            }
            _ => Ok(v.visit_each_child(Some(node))),
        }
    }

    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.setParent
    fn set_parent(&self, node: Option<NodeId>) {
        self.parent.set(node);
    }

    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.setCurrentLexicalScope
    fn set_current_lexical_scope(&self, node: Option<NodeId>) {
        self.current_lexical_scope.set(node);
    }

    fn class_parts(factory: &dyn RuntimeFactory, node: NodeId) -> ClassParts {
        let read = factory.node(node);
        let source = read.data_source();
        if let Some(d) = source.as_class_declaration() {
            return ClassParts {
                modifiers: d.modifiers(),
                name: d.name(),
                type_parameters: d.type_parameters(),
                heritage_clauses: d.heritage_clauses(),
                members: d.members(),
            };
        }
        let d = source
            .as_class_expression()
            .expect("ClassExpression payload");
        ClassParts {
            modifiers: d.modifiers(),
            name: d.name(),
            type_parameters: d.type_parameters(),
            heritage_clauses: d.heritage_clauses(),
            members: d.members(),
        }
    }

    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.visitClassExpression
    fn visit_class_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let old_parent = self.parent.replace(Some(node));
        let result = self.visit_class_like(v, node);
        self.set_parent(old_parent);
        result
    }

    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.visitClassDeclaration
    fn visit_class_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let old_parent = self.parent.replace(Some(node));
        let result = self.visit_class_like(v, node);
        self.set_parent(old_parent);
        result
    }

    /// The shared body of `visitClassExpression` and `visitClassDeclaration`.
    fn visit_class_like(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        if !tsr_ast::utilities_class::class_or_constructor_parameter_is_decorated(
            view(v.factory())?,
            self.legacy_decorators,
            node,
        )? {
            return Ok(v.visit_each_child(Some(node)));
        }
        let parts = Self::class_parts(v.factory(), node);
        let visited = v.visit_modifiers(parts.modifiers);
        let modifiers = self.inject_class_type_metadata(v, visited, node)?;
        let name = v.visit_node(parts.name);
        let type_parameters = v.visit_nodes(parts.type_parameters);
        let heritage_clauses = v.visit_nodes(parts.heritage_clauses);
        let members = v.visit_nodes(parts.members);
        let f = v.factory_mut();
        Ok(Some(if f.node(node).kind() == K::ClassDeclaration {
            f.update_class_declaration(
                node,
                modifiers,
                name,
                type_parameters,
                heritage_clauses,
                members,
            )
        } else {
            f.update_class_expression(
                node,
                modifiers,
                name,
                type_parameters,
                heritage_clauses,
                members,
            )
        }))
    }

    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.visitPropertyDeclaration
    fn visit_property_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let has_decorators = {
            let view = view(v.factory())?;
            tsr_ast::utilities_middle::has_decorators(view, &view.node(node)?)?
        };
        if !has_decorators {
            return Ok(v.visit_each_child(Some(node)));
        }

        let (modifiers, name, postfix_token, type_node, initializer) = {
            let read = v.factory().node(node);
            let source = read.data_source();
            let d = source
                .as_property_declaration()
                .expect("PropertyDeclaration payload");
            (
                d.modifiers(),
                d.name(),
                d.postfix_token(),
                read.type_node(),
                d.initializer(),
            )
        };
        let visited = v.visit_modifiers(modifiers);
        let modifiers =
            self.inject_class_element_type_metadata(v, visited, node, self.parent.get())?;
        let name = v.visit_node(name);
        let postfix_token = v.visit_node(postfix_token);
        let type_node = v.visit_node(type_node);
        let initializer = v.visit_node(initializer);
        Ok(Some(v.factory_mut().update_property_declaration(
            node,
            modifiers,
            name,
            postfix_token,
            type_node,
            initializer,
        )))
    }

    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.visitMethodDeclaration
    fn visit_method_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let has_decorators = {
            let view = view(v.factory())?;
            tsr_ast::utilities_middle::has_decorators(view, &view.node(node)?)?
        };
        if !has_decorators && !has_decorators_of_parameters(v.factory(), node)? {
            return Ok(v.visit_each_child(Some(node)));
        }

        let parts = function_parts(v.factory(), node);
        let visited = v.visit_modifiers(parts.modifiers);
        let modifiers =
            self.inject_class_element_type_metadata(v, visited, node, self.parent.get())?;
        let asterisk_token = v.visit_node(parts.asterisk_token);
        let name = v.visit_node(parts.name);
        let postfix_token = v.visit_node(parts.postfix_token);
        let type_parameters = v.visit_nodes(parts.type_parameters);
        let parameters = v.visit_nodes(parts.parameters);
        let type_node = v.visit_node(parts.type_node);
        let full_signature = v.visit_node(parts.full_signature);
        let body = v.visit_node(parts.body);
        Ok(Some(v.factory_mut().update_method_declaration(
            node,
            modifiers,
            asterisk_token,
            name,
            postfix_token,
            type_parameters,
            parameters,
            type_node,
            full_signature,
            body,
        )))
    }

    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.visitSetAccessor
    fn visit_set_accessor(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let has_decorators = {
            let view = view(v.factory())?;
            tsr_ast::utilities_middle::has_decorators(view, &view.node(node)?)?
        };
        if !has_decorators && !has_decorators_of_parameters(v.factory(), node)? {
            return Ok(v.visit_each_child(Some(node)));
        }

        let parts = function_parts(v.factory(), node);
        let visited = v.visit_modifiers(parts.modifiers);
        let modifiers =
            self.inject_class_element_type_metadata(v, visited, node, self.parent.get())?;
        let name = v.visit_node(parts.name);
        let type_parameters = v.visit_nodes(parts.type_parameters);
        let parameters = v.visit_nodes(parts.parameters);
        let type_node = v.visit_node(parts.type_node);
        let full_signature = v.visit_node(parts.full_signature);
        let body = v.visit_node(parts.body);
        Ok(Some(v.factory_mut().update_set_accessor_declaration(
            node,
            modifiers,
            name,
            type_parameters,
            parameters,
            type_node,
            full_signature,
            body,
        )))
    }

    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.visitGetAccessor
    fn visit_get_accessor(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let has_decorators = {
            let view = view(v.factory())?;
            tsr_ast::utilities_middle::has_decorators(view, &view.node(node)?)?
        };
        if !has_decorators {
            return Ok(v.visit_each_child(Some(node)));
        }

        let parts = function_parts(v.factory(), node);
        let visited = v.visit_modifiers(parts.modifiers);
        let modifiers =
            self.inject_class_element_type_metadata(v, visited, node, self.parent.get())?;
        let name = v.visit_node(parts.name);
        let type_parameters = v.visit_nodes(parts.type_parameters);
        let parameters = v.visit_nodes(parts.parameters);
        let type_node = v.visit_node(parts.type_node);
        let full_signature = v.visit_node(parts.full_signature);
        let body = v.visit_node(parts.body);
        Ok(Some(v.factory_mut().update_get_accessor_declaration(
            node,
            modifiers,
            name,
            type_parameters,
            parameters,
            type_node,
            full_signature,
            body,
        )))
    }

    /// The nodes of a modifier list; a nil list has none.
    fn modifier_nodes(factory: &dyn RuntimeFactory, list: Option<NodeListId>) -> Vec<NodeId> {
        list.map_or_else(Vec::new, |list| {
            factory
                .read_nodes(factory.read_list(list).nodes())
                .iter()
                .map(|node| node.expect(NIL))
                .collect()
        })
    }

    /// `NodeFactory.NewModifierList(nodes)` at `list`'s location, if any.
    fn new_modifier_list_at(
        factory: &mut dyn RuntimeFactory,
        nodes: Vec<NodeId>,
        list: Option<NodeListId>,
    ) -> NodeListId {
        let slice = factory.alloc_nodes(nodes.into_iter().map(Some).collect());
        let res = factory.new_modifier_list(slice);
        if let Some(list) = list {
            let loc = factory.read_list(list).loc();
            factory.set_list_location(res, loc);
        }
        res
    }

    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.injectClassTypeMetadata
    fn inject_class_type_metadata(
        &self,
        v: &mut NodeVisitor<'_>,
        list: Option<NodeListId>,
        node: NodeId,
    ) -> Result<Option<NodeListId>, Error> {
        let metadata = self.get_type_metadata(v.factory_mut(), node, Some(node))?;
        if !metadata.is_empty() {
            let f = v.factory_mut();
            let original_nodes = Self::modifier_nodes(f, list);
            if original_nodes.is_empty() {
                return Ok(Some(Self::new_modifier_list_at(f, metadata, list)));
            }
            let kind = |f: &dyn RuntimeFactory, node: NodeId| f.node(node).kind();
            let mut modifiers_array = Vec::new();
            if tsr_ast::utilities::is_modifier(&f.node(original_nodes[0]))
                && matches!(
                    kind(f, original_nodes[0]).known(),
                    Some(K::DefaultKeyword | K::ExportKeyword)
                )
            {
                modifiers_array.push(original_nodes[0]);
                if original_nodes.len() > 1
                    && matches!(
                        kind(f, original_nodes[1]).known(),
                        Some(K::DefaultKeyword | K::ExportKeyword)
                    )
                {
                    modifiers_array.push(original_nodes[1]);
                }
            }
            let rest_start = modifiers_array.len();
            let decos = original_nodes
                .iter()
                .copied()
                .filter(|&node| tsr_ast::is_decorator(&f.node(node)));
            modifiers_array.extend(decos);
            modifiers_array.extend(metadata);
            let other_modifiers = original_nodes[rest_start..]
                .iter()
                .copied()
                .filter(|&node| tsr_ast::utilities::is_modifier(&f.node(node)));
            modifiers_array.extend(other_modifiers);
            return Ok(Some(Self::new_modifier_list_at(
                f,
                modifiers_array,
                Some(list.expect(NIL)),
            )));
        }
        Ok(list)
    }

    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.injectClassElementTypeMetadata
    fn inject_class_element_type_metadata(
        &self,
        v: &mut NodeVisitor<'_>,
        list: Option<NodeListId>,
        node: NodeId,
        container: Option<NodeId>,
    ) -> Result<Option<NodeListId>, Error> {
        if !tsr_ast::utilities::is_class_like(&v.factory().node(container.expect(NIL))) {
            return Ok(list);
        }
        if !tsr_ast::utilities_class::class_element_or_class_element_parameter_is_decorated(
            view(v.factory())?,
            self.legacy_decorators,
            node,
            container,
        )? {
            return Ok(list);
        }
        let metadata = self.get_type_metadata(v.factory_mut(), node, container)?;
        if !metadata.is_empty() {
            let f = v.factory_mut();
            let original_nodes = Self::modifier_nodes(f, list);
            if original_nodes.is_empty() {
                return Ok(Some(Self::new_modifier_list_at(f, metadata, list)));
            }
            let mut modifiers_array = Vec::new();
            let decos = original_nodes
                .iter()
                .copied()
                .filter(|&node| tsr_ast::is_decorator(&f.node(node)));
            modifiers_array.extend(decos);
            modifiers_array.extend(metadata);
            let modifiers = original_nodes
                .iter()
                .copied()
                .filter(|&node| tsr_ast::utilities::is_modifier(&f.node(node)));
            modifiers_array.extend(modifiers);
            return Ok(Some(Self::new_modifier_list_at(
                f,
                modifiers_array,
                Some(list.expect(NIL)),
            )));
        }
        Ok(list)
    }

    fn serializer(&self) -> Rc<MetadataSerializer<'a>> {
        self.serializer.borrow().clone().expect(NIL)
    }

    fn serializer_context(&self, container: Option<NodeId>) -> MetadataSerializerContext {
        MetadataSerializerContext {
            current_lexical_scope: self.current_lexical_scope.get(),
            current_name_scope: container,
            serializing_conditional_type_branch: false,
        }
    }

    /// Gets optional type metadata for a declaration.
    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.getTypeMetadata
    fn get_type_metadata(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
        container: Option<NodeId>,
    ) -> Result<Vec<NodeId>, Error> {
        // Decorator metadata is not yet supported for ES decorators.
        if !self.legacy_decorators {
            return Ok(Vec::new());
        }
        if USE_NEW_TYPE_METADATA_FORMAT {
            return self.get_new_type_metadata(f, node, container);
        }
        self.get_old_type_metadata(f, node, container)
    }

    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.getOldTypeMetadata
    fn get_old_type_metadata(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
        container: Option<NodeId>,
    ) -> Result<Vec<NodeId>, Error> {
        let mut ec = self.emit_context.clone();
        let mut decorators = Vec::new();
        if Self::should_add_type_metadata(f, node) {
            let ctx = self.serializer_context(container);
            let value = self
                .serializer()
                .serialize_type_of_node_in(f, ctx, node, container)?;
            let type_metadata = ec.new_metadata_helper(f, b"design:type", value);
            decorators.push(f.new_decorator(Some(type_metadata)));
        }
        if Self::should_add_param_types_metadata(f, node)? {
            let ctx = self.serializer_context(container);
            let value = self
                .serializer()
                .serialize_parameter_types_of_node_in(f, ctx, node, container)?;
            let param_types_metadata = ec.new_metadata_helper(f, b"design:paramtypes", value);
            decorators.push(f.new_decorator(Some(param_types_metadata)));
        }
        if Self::should_add_return_type_metadata(f, node) {
            let ctx = self.serializer_context(container);
            let value = self
                .serializer()
                .serialize_return_type_of_node_in(f, ctx, node)?;
            let return_type_metadata = ec.new_metadata_helper(f, b"design:returntype", value);
            decorators.push(f.new_decorator(Some(return_type_metadata)));
        }
        Ok(decorators)
    }

    /// `type: () => <serialized>`, one property of the new format.
    fn new_type_info_property(f: &mut dyn RuntimeFactory, name: &[u8], body: NodeId) -> NodeId {
        let name = f.new_identifier(JsString::from_bytes(name));
        let parameters = new_node_list(f, Vec::new());
        let arrow = f.new_token(K::EqualsGreaterThanToken.into());
        let function = f.new_arrow_function(
            None,
            None,
            Some(parameters),
            None,
            None,
            Some(arrow),
            Some(body),
        );
        f.new_property_assignment(None, Some(name), None, None, Some(function))
    }

    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.getNewTypeMetadata
    fn get_new_type_metadata(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
        container: Option<NodeId>,
    ) -> Result<Vec<NodeId>, Error> {
        let mut properties = Vec::new();
        if Self::should_add_type_metadata(f, node) {
            let ctx = self.serializer_context(container);
            let body = self
                .serializer()
                .serialize_type_of_node_in(f, ctx, node, container)?;
            properties.push(Self::new_type_info_property(f, b"type", body));
        }
        if Self::should_add_param_types_metadata(f, node)? {
            let ctx = self.serializer_context(container);
            let body = self
                .serializer()
                .serialize_parameter_types_of_node_in(f, ctx, node, container)?;
            properties.push(Self::new_type_info_property(f, b"paramTypes", body));
        }
        if Self::should_add_return_type_metadata(f, node) {
            let ctx = self.serializer_context(container);
            let body = self
                .serializer()
                .serialize_return_type_of_node_in(f, ctx, node)?;
            properties.push(Self::new_type_info_property(f, b"returnType", body));
        }
        if !properties.is_empty() {
            let properties = new_node_list(f, properties);
            let object = f.new_object_literal_expression(Some(properties), true);
            let type_info_metadata =
                self.emit_context
                    .clone()
                    .new_metadata_helper(f, b"design:typeinfo", object);
            return Ok(vec![f.new_decorator(Some(type_info_metadata))]);
        }
        Ok(Vec::new())
    }

    /// Determines whether to emit the "design:type" metadata based on the
    /// node's kind. The caller should have already tested whether the node
    /// has decorators and whether the emitDecoratorMetadata compiler option
    /// is set.
    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.shouldAddTypeMetadata
    fn should_add_type_metadata(f: &dyn RuntimeFactory, node: NodeId) -> bool {
        matches!(
            f.node(node).kind().known(),
            Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor | K::PropertyDeclaration)
        )
    }

    /// Determines whether to emit the "design:returntype" metadata based on
    /// the node's kind. The caller should have already tested whether the
    /// node has decorators and whether the emitDecoratorMetadata compiler
    /// option is set.
    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.shouldAddReturnTypeMetadata
    fn should_add_return_type_metadata(f: &dyn RuntimeFactory, node: NodeId) -> bool {
        f.node(node).kind() == K::MethodDeclaration
    }

    /// Determines whether to emit the "design:paramtypes" metadata based on
    /// the node's kind. The caller should have already tested whether the
    /// node has decorators and whether the emitDecoratorMetadata compiler
    /// option is set.
    // port: tsc/internal/transformers/tstransforms/metadata.go:MetadataTransformer.shouldAddParamTypesMetadata
    fn should_add_param_types_metadata(
        f: &dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<bool, Error> {
        match f.node(node).kind().known() {
            Some(K::ClassDeclaration | K::ClassExpression) => Ok(
                tsr_ast::utilities_class::get_first_constructor_with_body(view(f)?, node)?
                    .is_some(),
            ),
            Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor) => Ok(true),
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
#[path = "metadata_tests.rs"]
mod metadata_tests;
