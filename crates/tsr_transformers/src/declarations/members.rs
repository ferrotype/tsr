//! The variable-declaration, signature and class/interface member transforms
//! of `transform.go`.
use super::{
    transform::{Transformer, NIL},
    util,
};
use tsr_ast::{
    modifier_flags as mf, Factory, FactoryMethods, JsString, NodeId, NodeListId, SyntaxKind as K,
};
use tsr_printer::emit_resolver::DeclarationEmitResolver;

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformVariableDeclaration
    pub fn transform_variable_declaration(
        &mut self,
        input: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        if self
            .output
            .read_source_file(self.current_source_file)?
            .common_js_module_indicator()
            .is_some()
            && tsr_ast::is_variable_declaration_initialized_to_require(self.view(), input)?
        {
            return Ok(self.transform_cjs_require_variable_declaration(input));
        }
        let name = self.node(input).name();
        if let Some(pattern) = name.filter(|name| self.is_binding_pattern(*name)) {
            if self.has_any_binding_initializers(pattern) {
                return self.recreate_binding_pattern(pattern);
            }
        }
        // Variable declaration types also suppress new diagnostic contexts, provided the contexts wouldn't be made for binding pattern types
        self.suppress_new_diagnostic_contexts = true;
        let name = self.visit_binding_name_node(name)?;
        let ty = self.ensure_type(input, false)?;
        let initializer = self.ensure_no_initializer(input)?;
        Ok(Some(self.output.update_variable_declaration(
            input,
            name,
            None,
            ty,
            initializer,
        )))
    }

    pub fn is_binding_pattern(&self, node: NodeId) -> bool {
        matches!(
            self.kind(node),
            K::ArrayBindingPattern | K::ObjectBindingPattern
        )
    }

    // port: tsc/internal/transformers/declarations/transform.go:hasAnyBindingInitializers
    fn has_any_binding_initializers(&self, binding_pattern: NodeId) -> bool {
        for elem in self.list_nodes(self.node(binding_pattern).element_list()) {
            if self.kind(elem) != K::BindingElement {
                continue;
            }
            if self.node(elem).initializer().is_some() {
                return true;
            }
            if let Some(name) = self.node(elem).name() {
                if self.is_binding_pattern(name) && self.has_any_binding_initializers(name) {
                    return true;
                }
            }
        }
        false
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformCjsRequireVariableDeclaration
    fn transform_cjs_require_variable_declaration(&mut self, input: NodeId) -> Option<NodeId> {
        let initializer = self.node(input).initializer().expect(NIL);
        let argument = *self
            .list_nodes(
                self.node(initializer)
                    .as_call_expression()
                    .expect("call expression payload")
                    .arguments(),
            )
            .first()
            .expect("runtime error: index out of range [0] with length 0");
        let specifier = self.rewrite_module_specifier(input, Some(argument));
        let name = self.node(input).name().expect(NIL);
        if self.kind(name) == K::Identifier {
            // `const x = require("something")` -> `import x = require("something")`
            let reference = self.output.new_external_module_reference(specifier);
            return Some(self.output.new_import_equals_declaration(
                None,
                false,
                Some(name),
                Some(reference),
            ));
        } else if self.kind(name) == K::ArrayBindingPattern {
            // TODO: Is this actually reachable? should we error on this?
            return None;
        }
        // object binding pattern

        // `const {x, y: z} = require("something")` -> `import {x, y as z} from "something"`
        let mut import_specifiers = Vec::new();
        for elem in self.list_nodes(self.node(name).element_list()) {
            let elem_name = self.node(elem).name().expect(NIL);
            if self.kind(elem_name) != K::Identifier {
                continue; // nested destructuring, bail
            }
            let property_name = self.node(elem).property_name();
            import_specifiers.push(self.output.new_import_specifier(
                false,
                property_name,
                Some(elem_name),
            ));
        }
        let list = self.new_node_list(import_specifiers);
        let named_imports = self.output.new_named_imports(Some(list));
        let clause = self
            .output
            .new_import_clause(K::Unknown.into(), None, Some(named_imports));
        Some(
            self.output
                .new_import_declaration(None, Some(clause), specifier, None),
        )
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.recreateBindingPattern
    fn recreate_binding_pattern(&mut self, input: NodeId) -> Result<Option<NodeId>, R::Error> {
        let mut results = Vec::new();
        for elem in self.list_nodes(self.node(input).element_list()) {
            let Some(result) = self.recreate_binding_element(elem)? else {
                continue;
            };
            if self.kind(result) == K::SyntaxList {
                results.extend(self.node_or_syntax_list_children(result));
            } else {
                results.push(result);
            }
        }
        if results.is_empty() {
            return Ok(None);
        }
        if results.len() == 1 {
            return Ok(Some(results[0]));
        }
        Ok(Some(self.new_syntax_list(results)))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.recreateBindingElement
    fn recreate_binding_element(&mut self, e: NodeId) -> Result<Option<NodeId>, R::Error> {
        let Some(name) = self.node(e).name() else {
            return Ok(None);
        };
        if !util::get_binding_name_visible(self, e)? {
            return Ok(None);
        }
        if self.is_binding_pattern(name) {
            return self.recreate_binding_pattern(name);
        }
        let ty = self.ensure_type(e, false)?;
        Ok(Some(self.output.new_variable_declaration(
            Some(name),
            None,
            ty,
            None, // TODO: possible strada bug - not emitting const initialized binding pattern elements?
        )))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformIndexSignatureDeclaration
    pub fn transform_index_signature_declaration(
        &mut self,
        input: NodeId,
    ) -> Result<NodeId, R::Error> {
        let (ty, parameters) = {
            let read = self.node(input);
            (read.type_node(), read.parameter_list())
        };
        let t = match self.visit(ty)? {
            Some(t) => t,
            None => self.output.new_keyword_type_node(K::AnyKeyword.into()),
        };
        let modifiers = self.ensure_modifiers(input)?;
        let parameters = self.update_param_list(input, parameters)?;
        Ok(self.output.update_index_signature_declaration(
            input,
            modifiers,
            Some(parameters),
            Some(t),
        ))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformCallSignatureDeclaration
    pub fn transform_call_signature_declaration(
        &mut self,
        input: NodeId,
    ) -> Result<NodeId, R::Error> {
        let (type_parameters, parameters) = {
            let read = self.node(input);
            (read.type_parameter_list(), read.parameter_list())
        };
        let type_parameters = self.ensure_type_params(input, type_parameters)?;
        let parameters = self.update_param_list(input, parameters)?;
        let ty = self.ensure_type(input, false)?;
        Ok(self.output.update_call_signature_declaration(
            input,
            type_parameters,
            Some(parameters),
            ty,
        ))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformPropertySignatureDeclaration
    pub fn transform_property_signature_declaration(
        &mut self,
        input: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let (name, postfix_token) = {
            let read = self.node(input);
            (read.name(), read.postfix_token())
        };
        if self.kind(name.expect(NIL)) == K::PrivateIdentifier {
            return Ok(None);
        }
        let modifiers = self.ensure_modifiers(input)?;
        let ty = self.ensure_type(input, false)?;
        let initializer = self.ensure_no_initializer(input)?; // TODO: possible strada bug (fixed here) - const property signatures never initialized
        let result = self.output.update_property_signature_declaration(
            input,
            modifiers,
            name,
            postfix_token,
            ty,
            initializer,
        );
        self.preserve_partial_js_doc(result, input)?;
        Ok(Some(result))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformPropertyDeclaration
    pub fn transform_property_declaration(
        &mut self,
        input: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let (name, postfix_token) = {
            let read = self.node(input);
            (read.name(), read.postfix_token())
        };
        if self.kind(name.expect(NIL)) == K::PrivateIdentifier {
            return Ok(None);
        }
        // Remove definite assignment assertion (!) from declaration files
        let postfix_token = postfix_token.filter(|token| self.kind(*token) != K::ExclamationToken);
        let modifiers = self.ensure_modifiers(input)?;
        let ty = self.ensure_type(input, false)?;
        let initializer = self.ensure_no_initializer(input)?;
        Ok(Some(self.output.update_property_declaration(
            input,
            modifiers,
            name,
            postfix_token,
            ty,
            initializer,
        )))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformSetAccessorDeclaration
    pub fn transform_set_accessor_declaration(
        &mut self,
        input: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let name = self.node(input).name();
        if self.kind(name.expect(NIL)) == K::PrivateIdentifier {
            return Ok(None);
        }
        let modifiers = self.ensure_modifiers(input)?;
        let is_private = self.effective_declaration_flags(input, mf::PRIVATE)? != 0;
        let parameters = self.update_accessor_param_list(input, is_private)?;
        Ok(Some(self.output.update_set_accessor_declaration(
            input,
            modifiers,
            name,
            None, // accessors shouldn't have type params
            Some(parameters),
            None,
            None,
            None,
        )))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformGetAccesorDeclaration
    pub fn transform_get_accesor_declaration(
        &mut self,
        input: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let name = self.node(input).name();
        if self.kind(name.expect(NIL)) == K::PrivateIdentifier {
            return Ok(None);
        }
        let modifiers = self.ensure_modifiers(input)?;
        let is_private = self.effective_declaration_flags(input, mf::PRIVATE)? != 0;
        let parameters = self.update_accessor_param_list(input, is_private)?;
        let ty = self.ensure_type(input, false)?;
        Ok(Some(self.output.update_get_accessor_declaration(
            input,
            modifiers,
            name,
            None, // accessors shouldn't have type params
            Some(parameters),
            ty,
            None,
            None,
        )))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.updateAccessorParamList
    fn update_accessor_param_list(
        &mut self,
        input: NodeId,
        is_private: bool,
    ) -> Result<NodeListId, R::Error> {
        let mut new_params = Vec::new();
        if !is_private {
            let this_param = tsr_ast::utilities_class::get_this_parameter(self.view(), input)?;
            if let Some(this_param) = this_param {
                new_params.push(self.ensure_parameter(this_param)?);
            }
        }
        if self.kind(input) == K::SetAccessor {
            let mut value_param = None;
            if !is_private {
                let parameters = self.list_nodes(self.node(input).parameter_list());
                if new_params.len() == 1 && parameters.len() >= 2 {
                    value_param = Some(self.ensure_parameter(parameters[1])?);
                } else if new_params.is_empty() && !parameters.is_empty() {
                    value_param = Some(self.ensure_parameter(parameters[0])?);
                }
            }
            let value_param = if let Some(value_param) = value_param {
                value_param
            } else {
                // When synthesizing a missing value parameter, emit `value: any` for non-private accessors to match TypeScript's declaration emit behavior.
                let t =
                    (!is_private).then(|| self.output.new_keyword_type_node(K::AnyKeyword.into()));
                let name = self
                    .output
                    .new_identifier(JsString::from_bytes(b"value".as_slice()));
                self.output
                    .new_parameter_declaration(None, None, Some(name), None, t, None)
            };
            new_params.push(value_param);
        }
        Ok(self.new_node_list(new_params))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformConstructorDeclaration
    pub fn transform_constructor_declaration(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        // A constructor declaration may not have a type annotation
        let parameters = self.node(input).parameter_list();
        let modifiers = self.ensure_modifiers(input)?;
        let parameters = self.update_param_list(input, parameters)?;
        Ok(self.output.update_constructor_declaration(
            input,
            modifiers,
            None, // no type params
            Some(parameters),
            None, // no return type
            None,
            None,
        ))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformConstructSignatureDeclaration
    pub fn transform_construct_signature_declaration(
        &mut self,
        input: NodeId,
    ) -> Result<NodeId, R::Error> {
        let (type_parameters, parameters) = {
            let read = self.node(input);
            (read.type_parameter_list(), read.parameter_list())
        };
        let type_parameters = self.ensure_type_params(input, type_parameters)?;
        let parameters = self.update_param_list(input, parameters)?;
        let ty = self.ensure_type(input, false)?;
        Ok(self.output.update_construct_signature_declaration(
            input,
            type_parameters,
            Some(parameters),
            ty,
        ))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.omitPrivateMethodType
    fn omit_private_method_type(&mut self, input: NodeId) -> Result<Option<NodeId>, R::Error> {
        if let Some(symbol) = self.resolver.bound_symbol_of_declaration(input)? {
            let declarations = self.resolver.symbol_declarations(symbol)?;
            if !declarations.is_empty() && declarations[0] != input {
                return Ok(None);
            }
        }
        let modifiers = self.ensure_modifiers(input)?;
        let name = self.node(input).name();
        let result = self
            .output
            .new_property_declaration(modifiers, name, None, None, None);
        self.preserve_js_doc(result, input);
        Ok(Some(result))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformMethodSignatureDeclaration
    pub fn transform_method_signature_declaration(
        &mut self,
        input: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        if self.effective_declaration_flags(input, mf::PRIVATE)? != 0 {
            return self.omit_private_method_type(input);
        }
        let (name, postfix_token, type_parameters, parameters) = {
            let read = self.node(input);
            (
                read.name(),
                read.postfix_token(),
                read.type_parameter_list(),
                read.parameter_list(),
            )
        };
        if self.kind(name.expect(NIL)) == K::PrivateIdentifier {
            return Ok(None);
        }
        let modifiers = self.ensure_modifiers(input)?;
        let type_parameters = self.ensure_type_params(input, type_parameters)?;
        let parameters = self.update_param_list(input, parameters)?;
        let ty = self.ensure_type(input, false)?;
        Ok(Some(self.output.update_method_signature_declaration(
            input,
            modifiers,
            name,
            postfix_token,
            type_parameters,
            Some(parameters),
            ty,
        )))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformMethodDeclaration
    pub fn transform_method_declaration(
        &mut self,
        input: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        if self.effective_declaration_flags(input, mf::PRIVATE)? != 0 {
            return self.omit_private_method_type(input);
        }
        let (name, postfix_token, type_parameters, parameters) = {
            let read = self.node(input);
            (
                read.name(),
                read.postfix_token(),
                read.type_parameter_list(),
                read.parameter_list(),
            )
        };
        if self.kind(name.expect(NIL)) == K::PrivateIdentifier {
            return Ok(None);
        }
        let modifiers = self.ensure_modifiers(input)?;
        let type_parameters = self.ensure_type_params(input, type_parameters)?;
        let parameters = self.update_param_list(input, parameters)?;
        let ty = self.ensure_type(input, false)?;
        Ok(Some(self.output.update_method_declaration(
            input,
            modifiers,
            None,
            name,
            postfix_token,
            type_parameters,
            Some(parameters),
            ty,
            None,
            None,
        )))
    }
}
