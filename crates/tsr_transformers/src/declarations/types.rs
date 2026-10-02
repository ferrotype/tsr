//! The type-node transforms of `transform.go`, including the JSDoc type
//! rewrites.
use super::{
    transform::{Transformer, NIL},
    util,
};
use tsr_ast::{FactoryMethods, NodeId, RuntimeFactory, SyntaxKind as K};
use tsr_printer::emit_resolver::DeclarationEmitResolver;

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformMappedTypeNode
    pub fn transform_mapped_type_node(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let (readonly_token, type_parameter, name_type, question_token, ty) = {
            let read = self.node(input);
            let data = read.as_mapped_type_node().expect("mapped type payload");
            (
                data.readonly_token(),
                data.type_parameter(),
                data.name_type(),
                data.question_token(),
                data.r#type(),
            )
        };
        // handle missing template type nodes, since the printer does not
        let type_node = match ty {
            None => Some(self.output.new_keyword_type_node(K::AnyKeyword.into())),
            Some(ty) => self.visit(Some(ty))?,
        };
        let type_parameter = self.visit(type_parameter)?;
        let name_type = self.visit(name_type)?;
        Ok(self.output.update_mapped_type_node(
            input,
            readonly_token,
            type_parameter,
            name_type,
            question_token,
            type_node,
            None,
        ))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformHeritageClause
    pub fn transform_heritage_clause(
        &mut self,
        clause: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let (token, types) = {
            let read = self.node(clause);
            let data = read.as_heritage_clause().expect("heritage clause payload");
            (data.token(), data.types())
        };
        let nodes = self.list_nodes(types);
        let mut retained_clauses = Vec::new();
        for &t in &nodes {
            let name = tsr_ast::utilities_class::get_heritage_clause_element_name(self.view(), t)?
                .expect(NIL);
            let retained = tsr_ast::utilities::is_entity_name(&self.node(name))
                || tsr_ast::is_entity_name_expression(self.view(), name)?
                || (token == K::ExtendsKeyword
                    && self.kind(t) == K::ExpressionWithTypeArguments
                    && self.kind(self.node(t).expression().expect(NIL)) == K::NullKeyword);
            if retained {
                retained_clauses.push(t);
            }
        }
        if retained_clauses.is_empty() {
            return Ok(None); // elide empty clause
        }
        if retained_clauses.len() == nodes.len() {
            return self.visit_each_child(clause).map(Some);
        }
        let list = self.new_node_list(retained_clauses);
        let types = self.visit_nodes(Some(list))?;
        Ok(Some(
            self.output.update_heritage_clause(clause, token, types),
        ))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformImportTypeNode
    pub fn transform_import_type_node(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        if !self.is_literal_import_type_node(input) {
            return Ok(input);
        }
        let (is_type_of, argument, attributes, qualifier, type_arguments) = {
            let read = self.node(input);
            let data = read.as_import_type_node().expect("import type payload");
            (
                data.is_type_of(),
                data.argument().expect(NIL),
                data.attributes(),
                data.qualifier(),
                data.type_arguments(),
            )
        };
        let literal = self
            .node(argument)
            .as_literal_type_node()
            .expect("literal type payload")
            .literal();
        let literal = self.rewrite_module_specifier(input, literal);
        let argument = self.output.update_literal_type_node(argument, literal);
        let type_arguments = self.visit_nodes(type_arguments)?;
        Ok(self.output.update_import_type_node(
            input,
            is_type_of,
            Some(argument),
            attributes,
            qualifier,
            type_arguments,
        ))
    }
    /// `ast.IsLiteralImportTypeNode`.
    fn is_literal_import_type_node(&self, node: NodeId) -> bool {
        if self.kind(node) != K::ImportType {
            return false;
        }
        let argument = self
            .node(node)
            .as_import_type_node()
            .expect("import type payload")
            .argument()
            .expect(NIL);
        if self.kind(argument) != K::LiteralType {
            return false;
        }
        let literal = self
            .node(argument)
            .as_literal_type_node()
            .expect("literal type payload")
            .literal()
            .expect(NIL);
        self.kind(literal) == K::StringLiteral
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformConstructorTypeNode
    pub fn transform_constructor_type_node(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let (type_parameters, parameters, ty) = {
            let read = self.node(input);
            (
                read.type_parameter_list(),
                read.parameter_list(),
                read.type_node(),
            )
        };
        let modifiers = self.ensure_modifiers(input)?;
        let type_parameters = self.visit_nodes(type_parameters)?;
        let parameters = self.update_param_list(input, parameters)?;
        let ty = self.visit(ty)?;
        Ok(self.output.update_constructor_type_node(
            input,
            modifiers,
            type_parameters,
            Some(parameters),
            ty,
        ))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformFunctionTypeNode
    pub fn transform_function_type_node(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let (type_parameters, parameters, ty) = {
            let read = self.node(input);
            (
                read.type_parameter_list(),
                read.parameter_list(),
                read.type_node(),
            )
        };
        let type_parameters = self.visit_nodes(type_parameters)?;
        let parameters = self.update_param_list(input, parameters)?;
        let ty = self.visit(ty)?;
        Ok(self
            .output
            .update_function_type_node(input, type_parameters, Some(parameters), ty))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformConditionalTypeNode
    pub fn transform_conditional_type_node(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let (check_type, extends_type, true_type, false_type) = {
            let read = self.node(input);
            let data = read
                .as_conditional_type_node()
                .expect("conditional type payload");
            (
                data.check_type(),
                data.extends_type(),
                data.true_type(),
                data.false_type(),
            )
        };
        let check_type = self.visit(check_type)?;
        let extends_type = self.visit(extends_type)?;
        let old_enclosing_decl = self.enclosing_declaration;
        self.enclosing_declaration = true_type.expect(NIL);
        let true_type = self.visit(true_type)?;
        self.enclosing_declaration = old_enclosing_decl;
        let false_type = self.visit(false_type)?;
        Ok(self.output.update_conditional_type_node(
            input,
            check_type,
            extends_type,
            true_type,
            false_type,
        ))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformTypeReference
    pub fn transform_type_reference(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let type_name = self
            .node(input)
            .as_type_reference_node()
            .expect("type reference payload")
            .type_name()
            .expect(NIL);
        self.check_entity_name_visibility(type_name, self.enclosing_declaration)?;
        self.visit_each_child(input)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformExpressionWithTypeArguments
    pub fn transform_expression_with_type_arguments(
        &mut self,
        input: NodeId,
    ) -> Result<NodeId, R::Error> {
        let expression = self.node(input).expression().expect(NIL);
        if tsr_ast::utilities::is_entity_name(&self.node(expression))
            || tsr_ast::is_entity_name_expression(self.view(), expression)?
        {
            self.check_entity_name_visibility(expression, self.enclosing_declaration)?;
        }
        self.visit_each_child(input)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformTypeParameterDeclaration
    pub fn transform_type_parameter_declaration(
        &mut self,
        input: NodeId,
    ) -> Result<NodeId, R::Error> {
        let (modifiers, name, constraint, expression, default_type) = {
            let read = self.node(input);
            let data = read
                .as_type_parameter_declaration()
                .expect("type parameter payload");
            (
                read.modifiers(),
                read.name(),
                data.constraint(),
                data.expression(),
                data.default_type(),
            )
        };
        if util::is_private_method_type_parameter(self, input)?
            && (default_type.is_some() || constraint.is_some())
        {
            return Ok(self.output.update_type_parameter_declaration(
                input, modifiers, name, None, expression, None,
            ));
        }
        self.visit_each_child(input)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocTypeExpression
    pub fn transform_js_doc_type_expression(
        &mut self,
        input: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let ty = self.node(input).type_node();
        self.visit(ty)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocTypeLiteral
    pub fn transform_js_doc_type_literal(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let tags = {
            let read = self.node(input);
            let slice = read
                .as_js_doc_type_literal()
                .expect("JSDoc type literal payload")
                .js_doc_property_tags();
            self.output
                .read_nodes(slice)
                .iter()
                .flatten()
                .collect::<Vec<_>>()
        };
        let (members, _) = self.visit_slice(&tags)?;
        let list = self.new_node_list(members);
        let replacement = self.output.new_type_literal_node(Some(list));
        self.emit.set_original(replacement, input);
        Ok(replacement)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocPropertyTag
    pub fn transform_js_doc_property_tag(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let (tag_name, type_expression) = {
            let read = self.node(input);
            let data = read
                .as_js_doc_parameter_or_property_tag()
                .expect("JSDoc property tag payload");
            (data.tag_name(), data.type_expression())
        };
        let name = self.visit(tag_name)?;
        let ty = self.visit(type_expression)?;
        let replacement = self
            .output
            .new_property_signature_declaration(None, name, None, ty, None);
        self.emit.set_original(replacement, input);
        Ok(replacement)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocAllType
    pub fn transform_js_doc_all_type(&mut self, input: NodeId) -> NodeId {
        let replacement = self.output.new_keyword_type_node(K::AnyKeyword.into());
        self.emit.set_original(replacement, input);
        replacement
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocNullableType
    pub fn transform_js_doc_nullable_type(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let ty = self.node(input).type_node();
        let ty = self.visit(ty)?;
        let null = self.output.new_keyword_expression(K::NullKeyword.into());
        let null = self.output.new_literal_type_node(Some(null));
        let nodes = self.output.alloc_nodes(vec![ty, Some(null)]);
        let list = self
            .output
            .alloc_list(tsr_core::TextRange::new(-1, -1), nodes);
        let replacement = self.output.new_union_type_node(Some(list));
        self.emit.set_original(replacement, input);
        Ok(replacement)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocNonNullableType
    pub fn transform_js_doc_non_nullable_type(
        &mut self,
        input: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let ty = self.node(input).type_node();
        self.visit(ty)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocVariadicType
    pub fn transform_js_doc_variadic_type(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let operand = self
            .node(input)
            .as_js_doc_variadic_type()
            .expect("JSDoc variadic payload")
            .r#type();
        let ty = self.visit(operand)?;
        let replacement = self.output.new_array_type_node(ty);
        self.emit.set_original(replacement, input);
        Ok(replacement)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocOptionalType
    pub fn transform_js_doc_optional_type(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let ty = self.node(input).type_node();
        let ty = self.visit(ty)?;
        let undefined = self
            .output
            .new_keyword_type_node(K::UndefinedKeyword.into());
        let nodes = self.output.alloc_nodes(vec![ty, Some(undefined)]);
        let list = self
            .output
            .alloc_list(tsr_core::TextRange::new(-1, -1), nodes);
        let replacement = self.output.new_union_type_node(Some(list));
        self.emit.set_original(replacement, input);
        Ok(replacement)
    }
}
