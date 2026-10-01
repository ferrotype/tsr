//! `transformers/tstransforms/typeserializer.go`: the runtime values that
//! decorator metadata records for type annotations.
//!
//! The serializer reads the transformer's factory through its syntax view
//! (`RuntimeFactory::ast_view`), so the shared `ast` predicates apply to
//! parsed and synthesized nodes alike; a failed read or resolver query is an
//! [`Error`] the transformer records.
use crate::transformer::{Error, SharedEmitResolver};
use crate::utilities::is_generated_identifier;
use std::cell::Cell;
use tsr_ast::{
    node_flags, token_flags, AstView, FactoryMethods, JsString, NodeId, NodeListId, RuntimeFactory,
    SyntaxKind as K,
};
use tsr_core::{debug, ScriptTarget, TextRange};
use tsr_printer::script_resolver::TypeReferenceSerializationKind;
use tsr_printer::EmitContext;

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// The factory's syntax view.
pub(crate) fn view(factory: &dyn RuntimeFactory) -> Result<AstView<'_>, Error> {
    factory.ast_view().ok_or(Error::Unsupported(
        "a transformer factory without a syntax view",
    ))
}

/// The nodes of `list`; a nil list is Go's nil-pointer panic.
pub(crate) fn list_nodes(
    view: AstView<'_>,
    list: Option<NodeListId>,
) -> Result<Vec<NodeId>, Error> {
    let list = view.list(list.expect(NIL))?;
    Ok(view.node_slice(list.nodes())?.iter().flatten().collect())
}

/// Go's `Node.Members()` of a class-like container.
pub(crate) fn members(view: AstView<'_>, container: NodeId) -> Result<Vec<NodeId>, Error> {
    let read = view.node(container)?;
    let slice = read.members(view)?;
    Ok(view.node_slice(slice)?.iter().flatten().collect())
}

/// `NodeFactory.NewNodeList`: an undefined location.
pub(crate) fn new_node_list(factory: &mut dyn RuntimeFactory, nodes: Vec<NodeId>) -> NodeListId {
    let nodes = factory.alloc_nodes(nodes.into_iter().map(Some).collect());
    factory.alloc_list(TextRange::new(-1, -1), nodes)
}

fn identifier(factory: &mut dyn RuntimeFactory, text: &[u8]) -> NodeId {
    factory.new_identifier(JsString::from_bytes(text))
}

/// Go's `Node.Text()` of an identifier, numeric or string literal.
fn text(view: AstView<'_>, node: NodeId) -> Result<Vec<u8>, Error> {
    let read = view.node(node)?;
    let source = read.data_source();
    if let Some(identifier) = source.as_identifier() {
        return Ok(identifier.text().to_vec());
    }
    if let Some(literal) = source.as_numeric_literal() {
        return Ok(literal.text().to_vec());
    }
    if let Some(literal) = source.as_string_literal() {
        return Ok(literal.text().to_vec());
    }
    panic!("Unhandled case in Node.Text: {:?}", read.kind())
}

#[derive(Clone, Copy, Default)]
pub(crate) struct MetadataSerializerContext {
    pub(crate) current_lexical_scope: Option<NodeId>,
    pub(crate) current_name_scope: Option<NodeId>,
    pub(crate) serializing_conditional_type_branch: bool,
}

/// `metadataSerializer`. Upstream's factory is the visitor's, passed to each
/// call.
pub(crate) struct MetadataSerializer<'a> {
    resolver: SharedEmitResolver<'a>,
    language_version: ScriptTarget,
    strict_null_checks: bool,
    ec: EmitContext,
    c: Cell<MetadataSerializerContext>,
}

impl<'a> MetadataSerializer<'a> {
    // port: tsc/internal/transformers/tstransforms/typeserializer.go:newMetadataSerializer
    pub(crate) fn new(
        resolver: SharedEmitResolver<'a>,
        ec: EmitContext,
        language_version: ScriptTarget,
        strict_null_checks: bool,
    ) -> Self {
        Self {
            resolver,
            language_version,
            strict_null_checks,
            ec,
            c: Cell::new(MetadataSerializerContext::default()),
        }
    }

    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.setContext
    fn set_context(&self, ctx: MetadataSerializerContext) {
        self.c.set(ctx);
    }

    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.SerializeTypeOfNode
    pub(crate) fn serialize_type_of_node_in(
        &self,
        f: &mut dyn RuntimeFactory,
        ctx: MetadataSerializerContext,
        node: NodeId,
        container: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        let old_ctx = self.c.get();
        self.c.set(ctx);
        let result = self.serialize_type_of_node(f, node, container);
        self.set_context(old_ctx);
        result
    }

    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.SerializeParameterTypesOfNode
    pub(crate) fn serialize_parameter_types_of_node_in(
        &self,
        f: &mut dyn RuntimeFactory,
        ctx: MetadataSerializerContext,
        node: NodeId,
        container: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        let old_ctx = self.c.get();
        self.c.set(ctx);
        let result = self.serialize_parameter_types_of_node(f, node, container);
        self.set_context(old_ctx);
        result
    }

    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.SerializeReturnTypeOfNode
    pub(crate) fn serialize_return_type_of_node_in(
        &self,
        f: &mut dyn RuntimeFactory,
        ctx: MetadataSerializerContext,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let old_ctx = self.c.get();
        self.c.set(ctx);
        let result = self.serialize_return_type_of_node(f, node);
        self.set_context(old_ctx);
        result
    }

    /// Serializes the type of a node for use with decorator type metadata.
    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.serializeTypeOfNode
    fn serialize_type_of_node(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
        container: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        let (kind, type_node) = {
            let read = view(f)?.node(node)?;
            (read.kind(), read.type_node())
        };
        match kind.known() {
            Some(K::PropertyDeclaration | K::Parameter) => self.serialize_type_node(f, type_node),
            Some(K::GetAccessor | K::SetAccessor) => {
                let type_node = get_accessor_type_node(view(f)?, node, container.expect(NIL))?;
                self.serialize_type_node(f, type_node)
            }
            Some(K::ClassDeclaration | K::ClassExpression | K::MethodDeclaration) => {
                Ok(identifier(f, b"Function"))
            }
            _ => Ok(self.ec.new_void_zero_expression(f)),
        }
    }

    /// Serializes the parameter types of a node for use with decorator type
    /// metadata.
    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.serializeParameterTypesOfNode
    fn serialize_parameter_types_of_node(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
        container: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        let value_declaration = {
            let view = view(f)?;
            let read = view.node(node)?;
            if tsr_ast::utilities::is_class_like(&read) {
                tsr_ast::utilities_class::get_first_constructor_with_body(view, node)?
            } else if tsr_ast::utilities::is_function_like(Some(&read)) && {
                let body = read.body().map(|body| view.node(body)).transpose()?;
                tsr_ast::node_is_present(body.as_ref())
            } {
                Some(node)
            } else {
                None
            }
        };

        let Some(value_declaration) = value_declaration else {
            let elements = new_node_list(f, Vec::new());
            return Ok(f.new_array_literal_expression(Some(elements), false));
        };

        let mut expressions = Vec::new();
        let parameters = {
            let view = view(f)?;
            let list = get_parameters_of_decorated_declaration(view, value_declaration, container)?;
            list_nodes(view, list)?
        };
        for (i, parameter) in parameters.into_iter().enumerate() {
            let (is_this, dot_dot_dot, rest_type) = {
                let view = view(f)?;
                let read = view.node(parameter)?;
                let name = read.name().expect(NIL);
                let is_this = i == 0
                    && view.node(name)?.kind() == K::Identifier
                    && text(view, name)? == b"this";
                let dot_dot_dot = read
                    .data_source()
                    .as_parameter_declaration()
                    .expect("ParameterDeclaration payload")
                    .dot_dot_dot_token()
                    .is_some();
                let rest_type = if dot_dot_dot {
                    tsr_ast::utilities_tail::get_rest_parameter_element_type(
                        view,
                        read.type_node(),
                    )?
                } else {
                    None
                };
                (is_this, dot_dot_dot, rest_type)
            };
            if is_this {
                continue;
            }
            if dot_dot_dot {
                expressions.push(self.serialize_type_node(f, rest_type)?);
            } else {
                expressions.push(self.serialize_type_of_node(f, parameter, container)?);
            }
        }
        let elements = new_node_list(f, expressions);
        Ok(f.new_array_literal_expression(Some(elements), false))
    }

    /// Serializes the return type of a node for use with decorator type
    /// metadata.
    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.serializeReturnTypeOfNode
    fn serialize_return_type_of_node(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let (type_node, is_async) = {
            let view = view(f)?;
            let read = view.node(node)?;
            let type_node = if tsr_ast::utilities::is_function_like(Some(&read)) {
                read.type_node()
            } else {
                None
            };
            let is_async = type_node.is_none() && tsr_ast::is_async_function(view, node)?;
            (type_node, is_async)
        };
        if type_node.is_some() {
            return self.serialize_type_node(f, type_node);
        } else if is_async {
            return Ok(identifier(f, b"Promise"));
        }
        Ok(self.ec.new_void_zero_expression(f))
    }

    /// Serializes a type node for use with decorator type metadata.
    ///
    /// Types are serialized in the following fashion:
    /// - Void types point to "undefined" (e.g. "void 0")
    /// - Function and Constructor types point to the global "Function" constructor.
    /// - Interface types with a call or construct signature types point to the global
    ///   "Function" constructor.
    /// - Array and Tuple types point to the global "Array" constructor.
    /// - Type predicates and booleans point to the global "Boolean" constructor.
    /// - String literal types and strings point to the global "String" constructor.
    /// - Enum and number types point to the global "Number" constructor.
    /// - Symbol types point to the global "Symbol" constructor.
    /// - Type references to classes (or class-like variables) point to the constructor for the class.
    /// - Anything else points to the global "Object" constructor.
    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.serializeTypeNode
    fn serialize_type_node(
        &self,
        f: &mut dyn RuntimeFactory,
        node: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        let Some(node) = node else {
            return Ok(identifier(f, b"Object"));
        };

        let node = tsr_ast::utilities_positions::skip_type_parentheses(view(f)?, node)?;
        let (kind, type_node) = {
            let read = view(f)?.node(node)?;
            (read.kind(), read.type_node())
        };

        match kind.known() {
            Some(K::VoidKeyword | K::UndefinedKeyword | K::NeverKeyword) => {
                return Ok(self.ec.new_void_zero_expression(f));
            }
            Some(K::FunctionType | K::ConstructorType) => return Ok(identifier(f, b"Function")),
            Some(K::ArrayType | K::TupleType) => return Ok(identifier(f, b"Array")),
            Some(K::TypePredicate) => {
                let asserts = view(f)?
                    .node(node)?
                    .data_source()
                    .as_type_predicate_node()
                    .expect("TypePredicate payload")
                    .asserts_modifier();
                if asserts.is_some() {
                    return Ok(self.ec.new_void_zero_expression(f));
                }
                return Ok(identifier(f, b"Boolean"));
            }
            Some(K::BooleanKeyword) => return Ok(identifier(f, b"Boolean")),
            Some(K::TemplateLiteralType | K::StringKeyword) => {
                return Ok(identifier(f, b"String"));
            }
            Some(K::ObjectKeyword) => return Ok(identifier(f, b"Object")),
            Some(K::LiteralType) => {
                let literal = view(f)?
                    .node(node)?
                    .data_source()
                    .as_literal_type_node()
                    .expect("LiteralType payload")
                    .literal()
                    .expect(NIL);
                return self.serialize_literal_of_literal_type_node(f, literal);
            }
            Some(K::NumberKeyword) => return Ok(identifier(f, b"Number")),
            Some(K::BigIntKeyword) => return Ok(self.serialize_big_int_constructor(f)),
            Some(K::SymbolKeyword) => return Ok(identifier(f, b"Symbol")),
            Some(K::TypeReference) => return self.serialize_type_reference_node(f, node),
            Some(K::IntersectionType | K::UnionType) => {
                let types = {
                    let view = view(f)?;
                    let read = view.node(node)?;
                    let source = read.data_source();
                    let list = match source.as_union_type_node() {
                        Some(union) => union.types(),
                        None => source
                            .as_intersection_type_node()
                            .expect("IntersectionType payload")
                            .types(),
                    };
                    list_nodes(view, list)?
                };
                return self.serialize_union_or_intersection_constituents(
                    f,
                    &types,
                    kind == K::IntersectionType,
                );
            }
            Some(K::ConditionalType) => {
                let (true_type, false_type) = {
                    let read = view(f)?.node(node)?;
                    let source = read.data_source();
                    let data = source
                        .as_conditional_type_node()
                        .expect("ConditionalType payload");
                    (data.true_type().expect(NIL), data.false_type().expect(NIL))
                };
                let mut ctx = self.c.get();
                let old_state = ctx.serializing_conditional_type_branch;
                ctx.serializing_conditional_type_branch = true;
                self.c.set(ctx);
                let result = self.serialize_union_or_intersection_constituents(
                    f,
                    &[true_type, false_type],
                    false,
                );
                let mut ctx = self.c.get();
                ctx.serializing_conditional_type_branch = old_state;
                self.c.set(ctx);
                return result;
            }
            Some(K::TypeOperator) => {
                let operator = view(f)?
                    .node(node)?
                    .data_source()
                    .as_type_operator_node()
                    .expect("TypeOperator payload")
                    .operator();
                if operator == K::ReadonlyKeyword {
                    return self.serialize_type_node(f, type_node);
                }
                // TODO: why is `unique symbol` not handled as `Symbol`? This falls back to `Object`
            }
            Some(
                K::TypeQuery
                | K::IndexedAccessType
                | K::MappedType
                | K::TypeLiteral
                | K::AnyKeyword
                | K::UnknownKeyword
                | K::ThisType
                | K::ImportType
                // handle JSDoc types from an invalid parse
                | K::JSDocAllType
                | K::JSDocVariadicType,
            ) => {}
            Some(K::JSDocNullableType | K::JSDocNonNullableType | K::JSDocOptionalType) => {
                return self.serialize_type_node(f, type_node);
            }
            _ => debug::fail_bad_syntax_kind(&kind, &[]),
        }
        Ok(identifier(f, b"Object"))
    }

    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.serializeUnionOrIntersectionConstituents
    fn serialize_union_or_intersection_constituents(
        &self,
        f: &mut dyn RuntimeFactory,
        types: &[NodeId],
        is_intersection: bool,
    ) -> Result<NodeId, Error> {
        // Note when updating logic here also update `getEntityNameForDecoratorMetadata` in checker.ts so that aliases can be marked as referenced
        let mut serialized_type: Option<NodeId> = None;
        for &type_node in types {
            let (type_node, kind, literal_is_null) = {
                let view = view(f)?;
                let type_node =
                    tsr_ast::utilities_positions::skip_type_parentheses(view, type_node)?;
                let read = view.node(type_node)?;
                let literal_is_null = match read.data_source().as_literal_type_node() {
                    Some(literal) => {
                        view.node(literal.literal().expect(NIL))?.kind() == K::NullKeyword
                    }
                    None => false,
                };
                (type_node, read.kind(), literal_is_null)
            };
            if kind == K::NeverKeyword {
                if is_intersection {
                    return Ok(self.ec.new_void_zero_expression(f)); // Reduce to `never` in an intersection
                }
                continue; // Elide `never` in a union
            }

            if kind == K::UnknownKeyword {
                if !is_intersection {
                    return Ok(identifier(f, b"Object")); // Reduce to `unknown` in a union
                }
                continue; // Elide `unknown` in an intersection
            }

            if kind == K::AnyKeyword {
                return Ok(identifier(f, b"Object")); // Reduce to `any` in a union or intersection
            }

            if !self.strict_null_checks && (literal_is_null || kind == K::UndefinedKeyword) {
                continue; // Elide null and undefined from unions for metadata, just like what we did prior to the implementation of strict null checks
            }

            let serialized_constituent = self.serialize_type_node(f, Some(type_node))?;
            let is_object = {
                let view = view(f)?;
                view.node(serialized_constituent)?.kind() == K::Identifier
                    && text(view, serialized_constituent)? == b"Object"
            };
            if is_object {
                // One of the individual is global object, return immediately
                return Ok(serialized_constituent);
            }

            // If there exists union that is not `void 0` expression, check if the the common type is identifier.
            // anything more complex and we will just default to Object
            if let Some(serialized_type) = serialized_type {
                // Different types
                if !self.equate_serialized_type_nodes(
                    view(f)?,
                    serialized_type,
                    serialized_constituent,
                )? {
                    return Ok(identifier(f, b"Object"));
                }
            } else {
                // Initialize the union type
                serialized_type = Some(serialized_constituent);
            }
        }

        // If we were able to find common type, use it
        if let Some(serialized_type) = serialized_type {
            return Ok(serialized_type);
        }
        Ok(self.ec.new_void_zero_expression(f)) // Fallback is only hit if all union constituents are null/undefined/never
    }

    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.serializeLiteralOfLiteralTypeNode
    fn serialize_literal_of_literal_type_node(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let (kind, operand) = {
            let read = view(f)?.node(node)?;
            let operand = read
                .data_source()
                .as_prefix_unary_expression()
                .and_then(|prefix| prefix.operand());
            (read.kind(), operand)
        };
        match kind.known() {
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral) => {
                Ok(identifier(f, b"String"))
            }
            Some(K::PrefixUnaryExpression) => {
                let operand = operand.expect(NIL);
                let operand_kind = view(f)?.node(operand)?.kind();
                match operand_kind.known() {
                    Some(K::NumericLiteral | K::BigIntLiteral) => {
                        self.serialize_literal_of_literal_type_node(f, operand)
                    }
                    _ => debug::fail_bad_syntax_kind(&operand_kind, &[]),
                }
            }
            Some(K::NumericLiteral) => Ok(identifier(f, b"Number")),
            Some(K::BigIntLiteral) => Ok(self.serialize_big_int_constructor(f)),
            Some(K::TrueKeyword | K::FalseKeyword) => Ok(identifier(f, b"Boolean")),
            Some(K::NullKeyword) => Ok(self.ec.new_void_zero_expression(f)),
            _ => debug::fail_bad_syntax_kind(&kind, &[]),
        }
    }

    /// Serializes a TypeReferenceNode to an appropriate JS constructor value
    /// for use with decorator type metadata.
    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.serializeTypeReferenceNode
    fn serialize_type_reference_node(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let ctx = self.c.get();
        let serial_scope = ctx.current_name_scope.or(ctx.current_lexical_scope);
        let type_name = view(f)?
            .node(node)?
            .data_source()
            .as_type_reference_node()
            .expect("TypeReference payload")
            .type_name()
            .expect(NIL);
        let name = self.ec.parse_node(f, type_name);
        let scope = serial_scope.and_then(|scope| self.ec.parse_node(f, scope));
        let kind = self
            .resolver
            .borrow_mut()
            .get_type_reference_serialization_kind(name, scope)?;
        match kind {
            TypeReferenceSerializationKind::Unknown => {
                // From conditional type type reference that cannot be resolved is Similar to any or unknown
                if ctx.serializing_conditional_type_branch {
                    return Ok(identifier(f, b"Object"));
                }

                let serialized = self.serialize_entity_name_as_expression_fallback(f, type_name)?;
                let mut ec = self.ec.clone();
                let temp = ec.new_temp_variable(f);
                ec.add_variable_declaration(f, temp);
                let assignment = self.ec.new_assignment_expression(f, temp, serialized);
                let condition = self.ec.new_type_check(f, assignment, b"function");
                let question = f.new_token(K::QuestionToken.into());
                let colon = f.new_token(K::ColonToken.into());
                let object = identifier(f, b"Object");
                Ok(f.new_conditional_expression(
                    Some(condition),
                    Some(question),
                    Some(temp),
                    Some(colon),
                    Some(object),
                ))
            }

            TypeReferenceSerializationKind::TypeWithConstructSignatureAndValue => Ok(self
                .serialize_entity_name_as_expression(f, type_name)?
                .expect(NIL)),

            TypeReferenceSerializationKind::VoidNullableOrNeverType => {
                Ok(self.ec.new_void_zero_expression(f))
            }

            TypeReferenceSerializationKind::BigIntLikeType => {
                Ok(self.serialize_big_int_constructor(f))
            }

            TypeReferenceSerializationKind::BooleanType => Ok(identifier(f, b"Boolean")),

            TypeReferenceSerializationKind::NumberLikeType => Ok(identifier(f, b"Number")),

            TypeReferenceSerializationKind::StringLikeType => Ok(identifier(f, b"String")),

            TypeReferenceSerializationKind::ArrayLikeType => Ok(identifier(f, b"Array")),

            TypeReferenceSerializationKind::EsSymbolType => Ok(identifier(f, b"Symbol")),

            TypeReferenceSerializationKind::TypeWithCallSignature => Ok(identifier(f, b"Function")),

            TypeReferenceSerializationKind::Promise => Ok(identifier(f, b"Promise")),

            TypeReferenceSerializationKind::ObjectType => Ok(identifier(f, b"Object")),
        }
    }

    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.serializeBigIntConstructor
    fn serialize_big_int_constructor(&self, f: &mut dyn RuntimeFactory) -> NodeId {
        if self.language_version >= ScriptTarget::ES2020 {
            return identifier(f, b"BigInt");
        }
        let big_int = identifier(f, b"BigInt");
        let condition = self.ec.new_type_check(f, big_int, b"function");
        let question = f.new_token(K::QuestionToken.into());
        let when_true = identifier(f, b"BigInt");
        let colon = f.new_token(K::ColonToken.into());
        let when_false = identifier(f, b"Object");
        f.new_conditional_expression(
            Some(condition),
            Some(question),
            Some(when_true),
            Some(colon),
            Some(when_false),
        )
    }

    /// Serializes an entity name as an expression for decorator type
    /// metadata.
    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.serializeEntityNameAsExpression
    fn serialize_entity_name_as_expression(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let (kind, loc) = {
            let read = view(f)?.node(node)?;
            (read.kind(), read.range())
        };
        match kind.known() {
            Some(K::Identifier) => {
                // Create a clone of the name with a new parent, and treat it as if it were
                // a source tree node for the purposes of the checker.
                let name = tsr_ast::clone_node(f, node);
                f.set_node_range(name, loc);
                // make this identifier emulate a parse node, making it behave correctly when inspected by the module transforms
                self.ec.clone().unset_original(name);
                // ensure the parent is set to a parse tree node.
                let parent = self
                    .c
                    .get()
                    .current_lexical_scope
                    .and_then(|scope| self.ec.parse_node(f, scope));
                f.set_node_parent(name, parent);
                Ok(Some(name))
            }
            Some(K::QualifiedName) => self
                .serialize_qualified_name_as_expression(f, node)
                .map(Some),
            _ => Ok(None),
        }
    }

    /// Serializes a qualified name as an expression for decorator type
    /// metadata.
    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.serializeQualifiedNameAsExpression
    fn serialize_qualified_name_as_expression(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let (left, right) = qualified_name_parts(view(f)?, node)?;
        let expression = self.serialize_entity_name_as_expression(f, left)?;
        Ok(f.new_property_access_expression(expression, None, Some(right), node_flags::NONE))
    }

    /// Serializes an entity name which may not exist at runtime, but whose
    /// access shouldn't throw.
    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.serializeEntityNameAsExpressionFallback
    fn serialize_entity_name_as_expression_fallback(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let kind = view(f)?.node(node)?.kind();
        if kind == K::Identifier {
            // A -> typeof A !== "undefined" && A
            let copied = self
                .serialize_entity_name_as_expression(f, node)?
                .expect(NIL);
            return Ok(self.create_checked_value(f, copied, copied));
        }
        let (left, right) = qualified_name_parts(view(f)?, node)?;
        if view(f)?.node(left)?.kind() == K::Identifier {
            // A.B -> typeof A !== "undefined" && A.B
            let checked = self
                .serialize_entity_name_as_expression(f, left)?
                .expect(NIL);
            let value = self
                .serialize_entity_name_as_expression(f, node)?
                .expect(NIL);
            return Ok(self.create_checked_value(f, checked, value));
        }
        // A.B.C -> typeof A !== "undefined" && (_a = A.B) !== void 0 && _a.C
        let left = self.serialize_entity_name_as_expression_fallback(f, left)?;
        let mut ec = self.ec.clone();
        let temp = ec.new_temp_variable(f);
        ec.add_variable_declaration(f, temp);
        let (left_left, left_right) = {
            let read = view(f)?.node(left)?;
            let source = read.data_source();
            let binary = source
                .as_binary_expression()
                .expect("BinaryExpression payload");
            (binary.left().expect(NIL), binary.right().expect(NIL))
        };
        let assignment = self.ec.new_assignment_expression(f, temp, left_right);
        let void_zero = self.ec.new_void_zero_expression(f);
        let inequality = self
            .ec
            .new_strict_inequality_expression(f, assignment, void_zero);
        let inner = self.ec.new_logical_and_expression(f, left_left, inequality);
        let access =
            f.new_property_access_expression(Some(temp), None, Some(right), node_flags::NONE);
        Ok(self.ec.new_logical_and_expression(f, inner, access))
    }

    /// Produces an expression that results in `right` if `left` is not
    /// undefined at runtime:
    ///
    /// ```text
    /// typeof left !== "undefined" && right
    /// ```
    ///
    /// We use `typeof L !== "undefined"` (rather than `L !== undefined`) since `L` may not be declared.
    /// It's acceptable for this expression to result in `false` at runtime, as the result is intended to be
    /// further checked by any containing expression.
    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.createCheckedValue
    fn create_checked_value(
        &self,
        f: &mut dyn RuntimeFactory,
        left: NodeId,
        right: NodeId,
    ) -> NodeId {
        let type_of = f.new_type_of_expression(Some(left));
        let undefined =
            f.new_string_literal(JsString::from_bytes(&b"undefined"[..]), token_flags::NONE);
        let inequality = self
            .ec
            .new_strict_inequality_expression(f, type_of, undefined);
        self.ec.new_logical_and_expression(f, inequality, right)
    }

    // port: tsc/internal/transformers/tstransforms/typeserializer.go:metadataSerializer.equateSerializedTypeNodes
    fn equate_serialized_type_nodes(
        &self,
        view: AstView<'_>,
        left: NodeId,
        right: NodeId,
    ) -> Result<bool, Error> {
        let l = view.node(left)?;
        let r = view.node(right)?;
        let (lk, rk) = (l.kind(), r.kind());
        // temp vars used in fallback
        if is_generated_identifier(&self.ec, left) {
            return Ok(is_generated_identifier(&self.ec, right));
        }
        // entity names
        if lk == K::Identifier {
            return Ok(rk == K::Identifier && text(view, left)? == text(view, right)?);
        }
        if lk == K::PropertyAccessExpression {
            return Ok(rk == K::PropertyAccessExpression
                && self.equate_serialized_type_nodes(
                    view,
                    l.expression().expect(NIL),
                    r.expression().expect(NIL),
                )?
                && self.equate_serialized_type_nodes(
                    view,
                    l.name().expect(NIL),
                    r.name().expect(NIL),
                )?);
        }
        // `void 0`
        if lk == K::VoidExpression {
            if rk != K::VoidExpression {
                return Ok(false);
            }
            let le = l.expression().expect(NIL);
            let re = r.expression().expect(NIL);
            return Ok(view.node(le)?.kind() == K::NumericLiteral
                && view.node(re)?.kind() == K::NumericLiteral
                && text(view, le)? == b"0"
                && text(view, re)? == b"0");
        }
        // `"undefined"` or `"function"` in `typeof` checks
        if lk == K::StringLiteral {
            return Ok(rk == K::StringLiteral && text(view, left)? == text(view, right)?);
        }
        // used in `typeof` checks for fallback
        if lk == K::TypeOfExpression {
            return Ok(rk == K::TypeOfExpression
                && self.equate_serialized_type_nodes(
                    view,
                    l.expression().expect(NIL),
                    r.expression().expect(NIL),
                )?);
        }
        // parens in `typeof` checks with temps
        if lk == K::ParenthesizedExpression {
            return Ok(rk == K::ParenthesizedExpression
                && self.equate_serialized_type_nodes(
                    view,
                    l.expression().expect(NIL),
                    r.expression().expect(NIL),
                )?);
        }
        // conditionals used in fallback
        if lk == K::ConditionalExpression {
            if rk != K::ConditionalExpression {
                return Ok(false);
            }
            let (lc, rc) = (conditional_parts(&l), conditional_parts(&r));
            return Ok(self.equate_serialized_type_nodes(view, lc.0, rc.0)?
                && self.equate_serialized_type_nodes(view, lc.1, rc.1)?
                && self.equate_serialized_type_nodes(view, lc.2, rc.2)?);
        }
        // logical binary and assignments used in fallback
        if lk == K::BinaryExpression {
            if rk != K::BinaryExpression {
                return Ok(false);
            }
            let (lb, rb) = (binary_parts(&l), binary_parts(&r));
            return Ok(view.node(lb.1)?.kind() == view.node(rb.1)?.kind()
                && self.equate_serialized_type_nodes(view, lb.0, rb.0)?
                && self.equate_serialized_type_nodes(view, lb.2, rb.2)?);
        }
        Ok(false)
    }
}

/// The condition, `whenTrue` and `whenFalse` of a conditional expression.
fn conditional_parts(read: &tsr_ast::NodeRead<'_>) -> (NodeId, NodeId, NodeId) {
    let source = read.data_source();
    let data = source
        .as_conditional_expression()
        .expect("ConditionalExpression payload");
    (
        data.condition().expect(NIL),
        data.when_true().expect(NIL),
        data.when_false().expect(NIL),
    )
}

/// The left, operator token and right of a binary expression.
fn binary_parts(read: &tsr_ast::NodeRead<'_>) -> (NodeId, NodeId, NodeId) {
    let source = read.data_source();
    let data = source
        .as_binary_expression()
        .expect("BinaryExpression payload");
    (
        data.left().expect(NIL),
        data.operator_token().expect(NIL),
        data.right().expect(NIL),
    )
}

/// The left and right of a qualified name; any other node is Go's failed
/// `AsQualifiedName` conversion.
fn qualified_name_parts(view: AstView<'_>, node: NodeId) -> Result<(NodeId, NodeId), Error> {
    let read = view.node(node)?;
    let source = read.data_source();
    let Some(name) = source.as_qualified_name() else {
        panic!(
            "interface conversion: ast.nodeData is *ast.{}, not *ast.QualifiedName",
            source.name()
        );
    };
    Ok((name.left().expect(NIL), name.right().expect(NIL)))
}

// port: tsc/internal/transformers/tstransforms/typeserializer.go:GetSetAccessorValueParameter
pub fn get_set_accessor_value_parameter(
    view: AstView<'_>,
    node: Option<NodeId>,
) -> Result<Option<NodeId>, Error> {
    if let Some(node) = node {
        let parameters = list_nodes(view, view.node(node)?.parameter_list())?;
        if !parameters.is_empty() {
            if parameters.len() >= 2
                && tsr_ast::utilities_class::is_this_parameter(view, parameters[0])?
            {
                return Ok(Some(parameters[1]));
            }
            return Ok(Some(parameters[0]));
        }
    }
    Ok(None)
}

/// Get the type annotation for the value parameter.
// port: tsc/internal/transformers/tstransforms/typeserializer.go:getSetAccessorTypeAnnotationNode
fn get_set_accessor_type_annotation_node(
    view: AstView<'_>,
    node: Option<NodeId>,
) -> Result<Option<NodeId>, Error> {
    let p = get_set_accessor_value_parameter(view, node)?;
    if let Some(p) = p {
        if let Some(type_node) = view.node(p)?.type_node() {
            return Ok(Some(type_node));
        }
    }
    Ok(None)
}

// port: tsc/internal/transformers/tstransforms/typeserializer.go:getAccessorTypeNode
fn get_accessor_type_node(
    view: AstView<'_>,
    node: NodeId,
    container: NodeId,
) -> Result<Option<NodeId>, Error> {
    let accessors = tsr_ast::utilities_class::get_all_accessor_declarations(
        view,
        &members(view, container)?,
        node,
    )?;
    if accessors.set_accessor.is_some() {
        return get_set_accessor_type_annotation_node(view, accessors.set_accessor);
    }
    if let Some(get_accessor) = accessors.get_accessor {
        return Ok(view.node(get_accessor)?.type_node());
    }
    Ok(None)
}

// port: tsc/internal/transformers/tstransforms/typeserializer.go:getParametersOfDecoratedDeclaration
fn get_parameters_of_decorated_declaration(
    view: AstView<'_>,
    node: NodeId,
    container: Option<NodeId>,
) -> Result<Option<NodeListId>, Error> {
    if let Some(container) = container {
        if view.node(node)?.kind() == K::GetAccessor {
            let acc = tsr_ast::utilities_class::get_all_accessor_declarations(
                view,
                &members(view, container)?,
                node,
            )?;
            if let Some(set_accessor) = acc.set_accessor {
                return Ok(view.node(set_accessor)?.parameter_list());
            }
        }
    }
    Ok(view.node(node)?.parameter_list())
}
