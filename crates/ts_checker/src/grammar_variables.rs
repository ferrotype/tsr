//! Variable and ambient-source grammar, preserving the pin's early returns.
use crate::{type_flags as tf, CheckerState, Error};
use ts_arena::NodeId;
use ts_ast::{modifier_flags as mf, node_flags as nf, SyntaxKind as K};
use ts_diagnostics as d;
use ts_jsstring::JsString;

impl CheckerState {
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarVariableDeclaration
    pub(crate) fn check_grammar_variable(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_variable_declaration()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let name = data.name().ok_or(Error::MissingLink("variable name"))?;
        let initializer = data.initializer();
        let annotation = read.type_node();
        let exclamation = data.exclamation_token();
        let parent = read.parent().ok_or(Error::MissingLink("variable list"))?;
        let statement = self
            .ast(parent)?
            .node(parent)?
            .parent()
            .ok_or(Error::MissingLink("variable statement"))?;
        let flags = ts_ast::utilities::get_combined_node_flags(self.ast(node)?, node)?;
        let block = flags & nf::BLOCK_SCOPED;
        let pattern = self.is_binding_pattern(name)?;
        let keyword = match block {
            nf::AWAIT_USING => Some(b"await using".as_slice()),
            nf::USING => Some(b"using".as_slice()),
            nf::CONST => Some(b"const".as_slice()),
            _ => None,
        };
        if pattern && matches!(block, nf::AWAIT_USING | nf::USING) {
            return self.grammar_error_node(
                node,
                d::X_0_declarations_may_not_have_binding_patterns,
                vec![JsString::from_bytes(keyword.unwrap())],
            );
        }
        if !matches!(
            self.ast(statement)?.node(statement)?.kind().known(),
            Some(K::ForInStatement | K::ForOfStatement)
        ) {
            if flags & nf::AMBIENT != 0 {
                self.check_ambient_initializer(node)?;
            } else if initializer.is_none() {
                if pattern && !self.is_binding_pattern(parent)? {
                    return self.grammar_error_node(
                        node,
                        d::A_destructuring_declaration_must_have_an_initializer,
                        vec![],
                    );
                }
                if let Some(keyword) = keyword {
                    return self.grammar_error_node(
                        node,
                        d::X_0_declarations_must_be_initialized,
                        vec![JsString::from_bytes(keyword)],
                    );
                }
            }
        }
        if let Some(exclamation) = exclamation {
            if self.ast(statement)?.node(statement)?.kind() != K::VariableStatement
                || annotation.is_none()
                || initializer.is_some()
                || flags & nf::AMBIENT != 0
            {
                let message = if initializer.is_some() {
                    d::Declarations_with_initializers_cannot_also_have_definite_assignment_assertions
                } else if annotation.is_none() {
                    d::Declarations_with_definite_assignment_assertions_must_also_have_type_annotations
                } else {
                    d::A_definite_assignment_assertion_is_not_permitted_in_this_context
                };
                return self.grammar_error_node(exclamation, message, vec![]);
            }
        }
        let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
            .ok_or(Error::MissingLink("variable source"))?;
        let filename = self
            .ast(source)?
            .source_file(source)?
            .parse_options()
            .file_name
            .clone();
        if self
            .program()?
            .host
            .get_emit_module_format_of_file(filename.as_bytes())?
            < ts_core::ModuleKind::SYSTEM
            && self.ast(statement)?.node(statement)?.flags() & nf::AMBIENT == 0
            && self
                .ast(statement)?
                .node(statement)?
                .modifier_flags(self.ast(statement)?)?
                & mf::EXPORT
                != 0
        {
            self.check_binding_export_marker(name)?;
        }
        if block != 0 {
            self.check_let_binding_name(name)
        } else {
            Ok(false)
        }
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarNameInLetOrConstDeclarations
    fn check_let_binding_name(&mut self, name: NodeId) -> Result<bool, Error> {
        if self.ast(name)?.node(name)?.kind() == K::Identifier {
            if self.ast(name)?.node_text(name)?.as_bytes() == b"let" {
                return self.grammar_error_node(
                    name,
                    d::X_let_is_not_allowed_to_be_used_as_a_name_in_let_or_const_declarations,
                    vec![],
                );
            }
        } else {
            for element in self.source_list(name, self.ast(name)?.node(name)?.element_list())? {
                if let Some(name) = self.ast(element)?.node(element)?.name() {
                    self.check_let_binding_name(name)?;
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarForEsModuleMarkerInBindingName
    fn check_binding_export_marker(&mut self, name: NodeId) -> Result<bool, Error> {
        if self.ast(name)?.node(name)?.kind() == K::Identifier {
            if self.ast(name)?.node_text(name)?.as_bytes() == b"__esModule"
                && !self.program()?.host.options().no_emit.is_true()
            {
                return self.grammar_error_node(name,d::Identifier_expected_esModule_is_reserved_as_an_exported_marker_when_transforming_ECMAScript_modules,vec![]);
            }
        } else {
            for element in self.source_list(name, self.ast(name)?.node(name)?.element_list())? {
                if let Some(name) = self.ast(element)?.node(element)?.name() {
                    return self.check_binding_export_marker(name);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkAmbientInitializer
    pub(crate) fn check_ambient_initializer(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let Some(initializer) = read.initializer() else {
            return Ok(false);
        };
        let annotation = read.type_node();
        let constant = read.modifier_flags(self.ast(node)?)? & mf::READONLY != 0
            || read.kind() == K::VariableDeclaration
                && ts_ast::utilities::is_var_const_like(self.ast(node)?, node)?;
        // Even invalid non-const declarations resolve the initializer's enum
        // reference first; the native helper can emit a name error here.
        let literal = self.initializer_literal(initializer, true)?;
        let enum_reference = if literal {
            false
        } else {
            self.initializer_enum_reference(initializer)?
        };
        if constant && annotation.is_none() {
            if !literal && !enum_reference {
                return self.grammar_error_node(initializer,d::A_const_initializer_in_an_ambient_context_must_be_a_string_or_numeric_literal_or_literal_enum_reference,vec![]);
            }
        } else {
            return self.grammar_error_node(
                initializer,
                d::Initializers_are_not_allowed_in_ambient_contexts,
                vec![],
            );
        }
        Ok(false)
    }

    fn initializer_literal(&self, node: NodeId, other_literals: bool) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        match read.kind().known() {
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral | K::NumericLiteral) => {
                Ok(true)
            }
            Some(K::BigIntLiteral | K::TrueKeyword | K::FalseKeyword) => Ok(other_literals),
            Some(K::PrefixUnaryExpression) => {
                let data = read
                    .data_source()
                    .as_prefix_unary_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let kind = self
                    .ast(node)?
                    .node(
                        data.operand()
                            .ok_or(Error::MissingLink("ambient literal operand"))?,
                    )?
                    .kind();
                Ok(data.operator() == K::MinusToken
                    && (kind == K::NumericLiteral || other_literals && kind == K::BigIntLiteral))
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.isInitializerSimpleLiteralEnumReference
    fn initializer_enum_reference(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let eligible = if read.kind() == K::PropertyAccessExpression {
            true
        } else if let Some(data) = read.data_source().as_element_access_expression() {
            self.initializer_literal(
                data.argument_expression()
                    .ok_or(Error::MissingLink("ambient enum argument"))?,
                false,
            )? && ts_ast::is_entity_name_expression(
                self.ast(node)?,
                data.expression()
                    .ok_or(Error::MissingLink("ambient enum expression"))?,
            )?
        } else {
            false
        };
        if eligible {
            let ty = self.check_expression_cached(node)?;
            Ok(self.types.flags(ty)? & tf::ENUM_LIKE != 0)
        } else {
            Ok(false)
        }
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarSourceFile
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarTopLevelElementsForRequiredDeclareModifier
    pub(crate) fn check_grammar_source(&mut self, source: NodeId) -> Result<bool, Error> {
        if self.ast(source)?.node(source)?.flags() & nf::AMBIENT == 0 {
            return Ok(false);
        }
        let statements: Vec<_> = self
            .ast(source)?
            .node_slice(
                self.ast(source)?
                    .node(source)?
                    .statements(self.ast(source)?)?,
            )?
            .iter()
            .flatten()
            .collect();
        for node in statements {
            let read = self.ast(node)?.node(node)?;
            if !ts_ast::is_declaration_node(&read) && read.kind() != K::VariableStatement {
                continue;
            }
            if matches!(
                read.kind().known(),
                Some(
                    K::InterfaceDeclaration
                        | K::TypeAliasDeclaration
                        | K::ImportDeclaration
                        | K::JSImportDeclaration
                        | K::ImportEqualsDeclaration
                        | K::ExportDeclaration
                        | K::ExportAssignment
                        | K::NamespaceExportDeclaration
                )
            ) || read.modifier_flags(self.ast(node)?)? & (mf::AMBIENT | mf::EXPORT | mf::DEFAULT)
                != 0
            {
                continue;
            }
            if self.grammar_error_first_token(node,d::Top_level_declarations_in_d_ts_files_must_start_with_either_a_declare_or_export_modifier,vec![])? {return Ok(true);}
        }
        Ok(false)
    }
}
