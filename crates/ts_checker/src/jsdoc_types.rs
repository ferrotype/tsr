//! JSDoc annotations use the reparsed AST type edges and the ordinary type stores.
use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, symbol_flags as sf, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getIntendedTypeFromJSDocTypeReference
    pub(crate) fn intended_jsdoc_type(&mut self, node: NodeId) -> Result<Option<TypeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.flags() & nf::JS_DOC == 0 || read.kind() != K::TypeReference {
            return Ok(None);
        }
        let name = read
            .data_source()
            .as_type_reference_node()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .type_name()
            .ok_or(Error::MissingLink("documentation type name"))?;
        if self.ast(name)?.node(name)?.kind() != K::Identifier {
            return Ok(None);
        }
        let text = self.ast(name)?.node_text(name)?.into_js_string();
        let arguments = self.source_list(node, read.type_argument_list())?;
        let builtin = match text.as_bytes() {
            b"String" => Some(self.builtins.string_type),
            b"Number" => Some(self.builtins.number_type),
            b"BigInt" => Some(self.builtins.bigint_type),
            b"Boolean" => Some(self.builtins.boolean_type),
            b"Void" => Some(self.builtins.void_type),
            b"Undefined" => Some(self.builtins.undefined_type),
            b"Null" => Some(self.builtins.null_type),
            b"Function" | b"function" => Some(
                *self
                    .query
                    .global_types
                    .get("Function")
                    .ok_or(Error::MissingLink("Function global"))?,
            ),
            _ => None,
        };
        let no_implicit = self
            .program()?
            .host
            .options()
            .strict_option_value(self.program()?.host.options().no_implicit_any);
        if builtin.is_some() || text.as_bytes() == b"Object" && arguments.len() != 2 && !no_implicit
        {
            if !arguments.is_empty() {
                let name = ts_scanner::declaration_name_to_string(self.ast(name)?, Some(name))?;
                self.error_at(
                    Some(node),
                    ts_diagnostics::Type_0_is_not_generic,
                    vec![name],
                )?;
            }
            return Ok(Some(builtin.unwrap_or(self.builtins.any_type)));
        }
        if arguments.is_empty() && !no_implicit {
            if text.as_bytes() == b"array" {
                return Ok(self.query.global_types.get("anyArrayType").copied());
            }
            if text.as_bytes() == b"promise" {
                return self.create_promise_type(self.builtins.any_type).map(Some);
            }
        }
        if text.as_bytes() == b"Object" && arguments.len() == 2 {
            if let Some(symbol) = self.global_type_alias_symbol("Record", 2, true)? {
                let key = self.get_type_from_type_node(arguments[0])?;
                if self.valid_index_key_type(key)? {
                    let value = self.get_type_from_type_node(arguments[1])?;
                    let declared = self.get_declared_type_of_symbol(symbol)?;
                    let parameters = self
                        .query
                        .type_aliases
                        .try_get(symbol)
                        .and_then(|links| links.parameters.clone())
                        .unwrap_or_default();
                    return self
                        .type_alias_instantiation(
                            symbol,
                            declared,
                            &parameters,
                            &[key, value],
                            None,
                        )
                        .map(Some);
                }
            }
            return Ok(Some(self.builtins.any_type));
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getGlobalTypeAliasResolver
    // port: tsc/internal/checker/checker.go:Checker.getGlobalTypeAliasSymbol
    pub(crate) fn global_type_alias_symbol(
        &mut self,
        name: &'static str,
        arity: usize,
        report: bool,
    ) -> Result<Option<ts_arena::SymbolId>, Error> {
        let key = (name, arity, report);
        if let Some(&symbol) = self.query.global_type_aliases.get(&key) {
            return Ok(symbol);
        }
        let symbol = self.resolve_name(
            None,
            name.as_bytes(),
            sf::TYPE_ALIAS,
            report.then_some(ts_diagnostics::Cannot_find_global_type_0),
            false,
        )?;
        let symbol = if let Some(symbol) = symbol {
            self.get_declared_type_of_symbol(symbol)?;
            let parameters = self
                .query
                .type_aliases
                .try_get(symbol)
                .and_then(|links| links.parameters.as_ref());
            if parameters.map_or(0, |parameters| parameters.len()) == arity {
                Some(symbol)
            } else {
                if report {
                    let declaration = self.declaration_of_kind(symbol, K::TypeAliasDeclaration)?;
                    let name = self.symbol(symbol)?.name_to_owned();
                    self.error_at(
                        declaration,
                        ts_diagnostics::Global_type_0_must_have_1_type_parameter_s,
                        vec![
                            name,
                            ts_ast::JsString::from_bytes(arity.to_string().into_bytes()),
                        ],
                    )?;
                }
                None
            }
        } else {
            None
        };
        self.query.global_type_aliases.insert(key, symbol);
        Ok(symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.getNullableType
    pub(crate) fn nullable_type(&mut self, ty: TypeId, flags: u32) -> Result<TypeId, Error> {
        match flags & !self.types.flags(ty)? & tf::NULLABLE {
            0 => Ok(ty),
            tf::UNDEFINED => self.get_union_type(&[ty, self.builtins.undefined_type]),
            tf::NULL => self.get_union_type(&[ty, self.builtins.null_type]),
            _ => self.get_union_type(&[ty, self.builtins.undefined_type, self.builtins.null_type]),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkJSDocType
    pub(crate) fn check_jsdoc_type(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        if read.flags() & nf::JAVA_SCRIPT_FILE == 0 {
            if matches!(
                read.kind().known(),
                Some(K::JSDocNonNullableType | K::JSDocNullableType)
            ) {
                let nullable = read.kind() == K::JSDocNullableType;
                let annotation = read
                    .type_node()
                    .ok_or(Error::MissingLink("JSDoc nullable type"))?;
                let postfix = read.pos() == self.ast(annotation)?.node(annotation)?.pos();
                let mut ty = self.get_type_from_type_node(annotation)?;
                if nullable && ty != self.builtins.never_type && ty != self.builtins.void_type {
                    ty =
                        self.nullable_type(ty, if postfix { tf::UNDEFINED } else { tf::NULLABLE })?;
                }
                let display = self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)?;
                let token = ts_ast::JsString::from_bytes(if nullable {
                    b"?".as_slice()
                } else {
                    b"!".as_slice()
                });
                let message = if postfix {
                    ts_diagnostics::X_0_at_the_end_of_a_type_is_not_valid_TypeScript_syntax_Did_you_mean_to_write_1
                } else {
                    ts_diagnostics::X_0_at_the_start_of_a_type_is_not_valid_TypeScript_syntax_Did_you_mean_to_write_1
                };
                self.grammar_error_node(node, message, vec![token, display])?;
            } else {
                self.grammar_error_node(
                    node,
                    ts_diagnostics::JSDoc_types_can_only_be_used_inside_documentation_comments,
                    vec![],
                )?;
            }
        }
        for child in self.source_children(node)? {
            self.check_source_element(child)?;
        }
        Ok(())
    }
}
