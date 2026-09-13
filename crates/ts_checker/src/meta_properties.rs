//! `new.target` and `import.meta` (`checkMetaProperty`, `checkNewTargetMetaProperty`
//! and `checkImportMetaProperty` in `tsc/internal/checker/checker.go`;
//! `checkGrammarMetaProperty` in `grammarchecks.go`).

use crate::{CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{NodeKind, SyntaxKind as K};
use ts_core::ModuleKind;
use ts_diagnostics as d;
use ts_jsstring::JsString;

impl CheckerState {
    /// The keyword and the name of a meta property.
    fn meta_property_parts(&self, node: NodeId) -> Result<(NodeKind, NodeId), Error> {
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_meta_property()
            .ok_or(Error::MissingLink("meta property payload"))?;
        let name = data
            .name()
            .ok_or(Error::MissingLink("meta property name"))?;
        Ok((data.keyword_token(), name))
    }

    // port: tsc/internal/checker/checker.go:Checker.checkMetaProperty
    pub(crate) fn check_meta_property(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.check_grammar_meta_property(node)?;
        let (keyword, name) = self.meta_property_parts(node)?;
        match keyword.known() {
            Some(K::NewKeyword) => self.check_new_target_meta_property(node),
            Some(K::ImportKeyword) => {
                if self.ast(name)?.node_text(name)?.as_bytes() == b"defer" {
                    // `import.defer` is only meaningful as a callee, whose type
                    // the call check computes; the bare property is an error.
                    return Ok(self.builtins.error_type);
                }
                self.check_import_meta_property(node)
            }
            _ => Err(Error::Unsupported("checkMetaProperty: unhandled keyword")),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkNewTargetMetaProperty
    fn check_new_target_meta_property(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let Some(container) = self.new_target_container(node)? else {
            self.error_at(
                Some(node),
                d::Meta_property_0_is_only_allowed_in_the_body_of_a_function_declaration_function_expression_or_constructor,
                vec![JsString::from_bytes(&b"new.target"[..])],
            )?;
            return Ok(self.builtins.error_type);
        };
        let read = self.ast(container)?.node(container)?;
        let declaration = if read.kind() == K::Constructor {
            read.parent()
                .ok_or(Error::MissingLink("constructor parent"))?
        } else {
            container
        };
        let symbol = self
            .get_symbol_of_declaration(declaration)?
            .ok_or(Error::MissingLink("new.target container symbol"))?;
        self.get_type_of_symbol(symbol)
    }

    /// The function declaration, function expression or constructor whose
    /// `new.target` the node reads; arrow functions are transparent.
    // port: tsc/internal/ast/utilities.go:GetNewTargetContainer
    fn new_target_container(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let view = self.ast(node)?;
        let container = ts_ast::get_this_container(view, node, false, false)?;
        Ok(matches!(
            view.node(container)?.kind().known(),
            Some(K::Constructor | K::FunctionDeclaration | K::FunctionExpression)
        )
        .then_some(container))
    }

    // port: tsc/internal/checker/checker.go:Checker.checkImportMetaProperty
    fn check_import_meta_property(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let module_kind = self.program()?.host.options().emit_module_kind();
        if (ModuleKind::NODE16..=ModuleKind::NODE_NEXT).contains(&module_kind) {
            let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
                .ok_or(Error::MissingLink("import.meta source file"))?;
            let path = self
                .ast(source)?
                .source_file(source)?
                .parse_options()
                .path
                .clone();
            let implied = self
                .program()?
                .host
                .get_source_file_meta_data(path.as_bytes())?
                .implied_node_format;
            if implied != ModuleKind::ESNEXT {
                self.error_at(
                    Some(node),
                    d::The_import_meta_meta_property_is_not_allowed_in_files_which_will_build_into_CommonJS_output,
                    vec![],
                )?;
            }
        } else if module_kind < ModuleKind::ES2020 && module_kind != ModuleKind::SYSTEM {
            self.error_at(
                Some(node),
                d::The_import_meta_meta_property_is_only_allowed_when_the_module_option_is_es2020_es2022_esnext_system_node16_node18_node20_or_nodenext,
                vec![],
            )?;
        }
        let (_, name) = self.meta_property_parts(node)?;
        if self.ast(name)?.node_text(name)?.as_bytes() == b"meta" {
            return self.global_import_meta_type();
        }
        Ok(self.builtins.error_type)
    }

    /// `getGlobalImportMetaType`: the `ImportMeta` global, resolved once and
    /// reported once, like upstream's memoized resolver.
    // port: tsc/internal/checker/checker.go:Checker.getGlobalTypeResolver
    fn global_import_meta_type(&mut self) -> Result<TypeId, Error> {
        if let Some(ty) = self.query.global_types.get("ImportMeta") {
            return Ok(*ty);
        }
        let ty = self.get_global_type("ImportMeta", 0, true)?;
        self.query.global_types.insert("ImportMeta", ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarMetaProperty
    fn check_grammar_meta_property(&mut self, node: NodeId) -> Result<bool, Error> {
        let (keyword, name) = self.meta_property_parts(node)?;
        let text = self.ast(name)?.node_text(name)?.into_js_string();
        let keyword_text = |keyword: NodeKind| {
            JsString::from_bytes(
                ts_scanner::token_to_string(keyword.known().expect("keyword token")).as_bytes(),
            )
        };
        match keyword.known() {
            Some(K::NewKeyword) if text.as_bytes() != b"target" => self.grammar_error_node(
                name,
                d::X_0_is_not_a_valid_meta_property_for_keyword_1_Did_you_mean_2,
                vec![
                    text,
                    keyword_text(keyword),
                    JsString::from_bytes(&b"target"[..]),
                ],
            ),
            Some(K::ImportKeyword) if text.as_bytes() != b"meta" => {
                let read = self.ast(node)?.node(node)?;
                let is_callee = match read.parent() {
                    Some(parent) => {
                        let parent_read = self.ast(parent)?.node(parent)?;
                        parent_read.kind() == K::CallExpression
                            && parent_read.expression() == Some(node)
                    }
                    None => false,
                };
                if text.as_bytes() == b"defer" {
                    if !is_callee {
                        let end = i64::from(read.end());
                        return self.grammar_error_range_with_args(
                            node,
                            end,
                            end,
                            d::X_0_expected,
                            vec![JsString::from_bytes(&b"("[..])],
                        );
                    }
                    Ok(false)
                } else if is_callee {
                    self.grammar_error_node(
                        name,
                        d::X_0_is_not_a_valid_meta_property_for_keyword_import_Did_you_mean_meta_or_defer,
                        vec![text],
                    )
                } else {
                    self.grammar_error_node(
                        name,
                        d::X_0_is_not_a_valid_meta_property_for_keyword_1_Did_you_mean_2,
                        vec![
                            text,
                            keyword_text(keyword),
                            JsString::from_bytes(&b"meta"[..]),
                        ],
                    )
                }
            }
            _ => Ok(false),
        }
    }
}
