//! Import attribute values participate in source checking and ambient-pattern
//! selection. Both consume the same cached anonymous type and retained loader
//! resolution; grammar errors never silently drop the attribute clause.
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    internal_symbol_names as names, symbol_flags as sf, JsString, SymbolTable, SyntaxKind as K,
};
use ts_core::ModuleKind;
use ts_diagnostics as d;

impl CheckerState {
    pub(crate) fn import_attributes(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        let data = read.data_source();
        Ok(match read.kind().known() {
            Some(K::ImportDeclaration | K::JSImportDeclaration) => {
                data.as_import_declaration().and_then(|n| n.attributes())
            }
            Some(K::ExportDeclaration) => data.as_export_declaration().and_then(|n| n.attributes()),
            Some(K::ImportType) => data.as_import_type_node().and_then(|n| n.attributes()),
            Some(K::ModuleDeclaration) => data.as_module_declaration().and_then(|n| n.attributes()),
            _ => None,
        })
    }
    fn import_attribute_nodes(&self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        let attributes = self
            .ast(node)?
            .node(node)?
            .as_import_attributes()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .attributes();
        self.source_list(node, attributes)
    }
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarImportAttributeValues
    pub(crate) fn check_grammar_import_attribute_values(
        &mut self,
        node: NodeId,
    ) -> Result<bool, Error> {
        let mut has_error = false;
        for attribute in self.import_attribute_nodes(node)? {
            let value = self
                .ast(attribute)?
                .node(attribute)?
                .as_import_attribute()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .value()
                .ok_or(ts_arena::Error::InvalidGraph)?;
            if self.ast(value)?.node(value)?.kind() != K::StringLiteral {
                has_error = true;
                self.error_at(
                    Some(value),
                    d::Import_attribute_values_must_be_string_literal_expressions,
                    vec![],
                )?;
            }
        }
        Ok(has_error)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkImportAttributesExpression
    pub(crate) fn import_attributes_expression_type(
        &mut self,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        if let Some(Some(ty)) = self.query.type_nodes.try_get(node) {
            return Ok(*ty);
        }
        let symbol = self.new_symbol(
            sf::OBJECT_LITERAL,
            JsString::from_bytes(names::IMPORT_ATTRIBUTES),
        )?;
        let mut members = SymbolTable::new();
        for attribute in self.import_attribute_nodes(node)? {
            let read = self.ast(attribute)?.node(attribute)?;
            let data = read
                .as_import_attribute()
                .ok_or(ts_arena::Error::InvalidGraph)?;
            let name = read.name().ok_or(ts_arena::Error::InvalidGraph)?;
            let value = data.value().ok_or(ts_arena::Error::InvalidGraph)?;
            let name = self.ast(name)?.node_text(name)?.into_js_string();
            let member = self.new_symbol(sf::PROPERTY, name.clone())?;
            let ty = self.check_expression_cached(value)?;
            let ty = self.get_regular_type_of_literal_type(ty)?;
            self.value_symbol_links.get_or_default(member).resolved_type = Some(ty);
            members.insert(name, Some(member));
        }
        let members = self.alloc_symbol_table(members);
        let ty = self.new_anonymous_type(Some(symbol), Some(members), &[], &[], &[])?;
        self.types.get_mut(ty)?.object_flags |= of::OBJECT_LITERAL | of::NON_INFERRABLE_TYPE;
        *self.query.type_nodes.get_or_default(node) = Some(ty);
        Ok(ty)
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeFromImportAttributes
    pub(crate) fn type_from_import_attributes(
        &mut self,
        node: Option<NodeId>,
    ) -> Result<Option<TypeId>, Error> {
        let Some(node) = node else { return Ok(None) };
        let ty = if self.ast(node)?.node(node)?.kind() == K::ImportAttributes {
            self.import_attributes_expression_type(node)?
        } else {
            self.check_expression_cached(node)?
        };
        Ok(Some(ty))
    }
    // port: tsc/internal/checker/checker.go:Checker.getImportAttributesTypeForModuleSpecifier
    pub(crate) fn import_attributes_type_for_specifier(
        &mut self,
        specifier: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        let Some(parent) = self.ast(specifier)?.node(specifier)?.parent() else {
            return Ok(None);
        };
        let read = self.ast(parent)?.node(parent)?;
        match read.kind().known() {
            Some(K::ImportDeclaration | K::JSImportDeclaration | K::ExportDeclaration) => {
                self.type_from_import_attributes(self.import_attributes(parent)?)
            }
            Some(K::LiteralType) => {
                let Some(import) = read.parent() else {
                    return Ok(None);
                };
                if self.ast(import)?.node(import)?.kind() == K::ImportType {
                    self.type_from_import_attributes(self.import_attributes(import)?)
                } else {
                    Ok(None)
                }
            }
            Some(K::CallExpression) => {
                let expression = read.expression();
                let is_import = if let Some(expr) = expression {
                    self.ast(expr)?.node(expr)?.kind() == K::ImportKeyword
                } else {
                    false
                };
                if is_import {
                    let arguments = self.source_list(parent, read.argument_list())?;
                    if let Some(&options) = arguments.get(1) {
                        let options = self.check_expression_cached(options)?;
                        return self.property_type(options, b"with");
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }
    pub(crate) fn global_import_attributes_type(&mut self) -> Result<TypeId, Error> {
        if let Some(ty) = self.module_aliases.global_import_attributes {
            return Ok(ty);
        }
        let ty = self.get_global_type("ImportAttributes", 0, true)?;
        self.module_aliases.global_import_attributes = Some(ty);
        Ok(ty)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkImportAttributes
    pub(crate) fn check_import_attributes(&mut self, declaration: NodeId) -> Result<(), Error> {
        let Some(node) = self.import_attributes(declaration)? else {
            return Ok(());
        };
        let global = self.global_import_attributes_type()?;
        if global != self.builtins.empty_object_type {
            let ty = self.import_attributes_expression_type(node)?;
            let target = self.nullable_type(global, tf::UNDEFINED)?;
            self.check_assignable_at(ty, target, node)?;
        }
        let read = self.ast(declaration)?.node(declaration)?;
        let type_only = match read.kind().known() {
            Some(K::ImportDeclaration | K::JSImportDeclaration) => {
                if let Some(clause) = read.as_import_declaration().and_then(|n| n.import_clause()) {
                    self.ast(clause)?
                        .node(clause)?
                        .as_import_clause()
                        .is_some_and(|n| n.phase_modifier() == K::TypeKeyword)
                } else {
                    false
                }
            }
            Some(K::ExportDeclaration) => read
                .as_export_declaration()
                .is_some_and(|n| n.is_type_only()),
            Some(K::ImportType) => true,
            _ => false,
        };
        let mode = self.import_resolution_mode_override(node, type_only)?;
        if type_only {
            return Ok(());
        }
        if !self
            .program()?
            .host
            .options()
            .emit_module_kind()
            .supports_import_attributes()
        {
            self.grammar_error_node(node, d::Import_attributes_are_only_supported_when_the_module_option_is_set_to_esnext_node18_node20_nodenext_or_preserve, vec![])?;
            return Ok(());
        }
        if let Some(specifier) = self.module_specifier(declaration)? {
            let (_, file) = self.module_source(specifier)?;
            if self
                .program()?
                .host
                .get_emit_syntax_for_usage_location(file.as_bytes(), specifier)?
                == ModuleKind::COMMON_JS
            {
                self.grammar_error_node(node, d::Import_attributes_are_not_allowed_on_statements_that_compile_to_CommonJS_require_calls, vec![])?;
                return Ok(());
            }
        }
        if mode != ModuleKind::NONE {
            self.grammar_error_node(
                node,
                d::X_resolution_mode_can_only_be_set_for_type_only_imports,
                vec![],
            )?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.getResolutionModeOverride
    pub(crate) fn import_resolution_mode_override(
        &mut self,
        node: NodeId,
        report: bool,
    ) -> Result<ModuleKind, Error> {
        let (mode, invalid) =
            ts_ast::utilities_middle::import_attributes_resolution_mode_with_invalid_value(
                self.ast(node)?,
                Some(node),
            )?;
        if report {
            if let Some(value) = invalid {
                self.grammar_error_node(
                    value,
                    d::X_resolution_mode_should_be_either_require_or_import,
                    vec![],
                )?;
            }
        }
        Ok(mode.unwrap_or(ModuleKind::NONE))
    }
    // port: tsc/internal/checker/checker.go:hasTypeJsonImportAttribute
    pub(crate) fn has_type_json_import_attribute(
        &self,
        declaration: NodeId,
    ) -> Result<bool, Error> {
        let Some(attributes) = self.import_attributes(declaration)? else {
            return Ok(false);
        };
        for attribute in self.import_attribute_nodes(attributes)? {
            let read = self.ast(attribute)?.node(attribute)?;
            let name = read.name().ok_or(ts_arena::Error::InvalidGraph)?;
            let value = read
                .as_import_attribute()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .value()
                .ok_or(ts_arena::Error::InvalidGraph)?;
            if self.ast(name)?.node_text(name)?.as_bytes() == b"type"
                && matches!(
                    self.ast(value)?.node(value)?.kind().known(),
                    Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
                )
                && self.ast(value)?.node_text(value)?.as_bytes() == b"json"
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeOfModuleImportAttributes
    pub(crate) fn module_import_attributes_type(
        &mut self,
        symbol: SymbolId,
    ) -> Result<TypeId, Error> {
        if let Some(&ty) = self.module_aliases.attributes_types.get(&symbol) {
            return Ok(ty);
        }
        let declarations = self
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect::<Vec<_>>();
        let mut ty = self.builtins.empty_object_type;
        for declaration in declarations {
            let read = self.ast(declaration)?.node(declaration)?;
            if read.kind() == K::ModuleDeclaration {
                if let Some(name) = read.name() {
                    if self.ast(name)?.node(name)?.kind() == K::StringLiteral {
                        if let Some(attributes) = self.import_attributes(declaration)? {
                            ty = self.get_type_from_type_node(attributes)?;
                        }
                        break;
                    }
                }
            }
        }
        self.module_aliases.attributes_types.insert(symbol, ty);
        Ok(ty)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkImportAttributesType
    pub(crate) fn check_import_attributes_type(&mut self, attributes: NodeId) -> Result<(), Error> {
        self.check_grammar_import_attributes_type(attributes)?;
        self.check_source_element(attributes)?;
        let global = self.global_import_attributes_type()?;
        let ty = self.get_type_from_type_node(attributes)?;
        if global != self.builtins.empty_object_type {
            self.check_assignable_at(ty, global, attributes)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarImportAttributesType
    fn check_grammar_import_attributes_type(&mut self, attributes: NodeId) -> Result<bool, Error> {
        for member in self.source_list(
            attributes,
            self.ast(attributes)?.node(attributes)?.member_list(),
        )? {
            let read = self.ast(member)?.node(member)?;
            if read.kind() != K::PropertySignature {
                return self.grammar_error_node(
                    member,
                    d::An_import_attributes_type_may_only_contain_property_signatures,
                    vec![],
                );
            }
            let Some(ty) = read.type_node() else {
                return self.grammar_error_node(
                    member,
                    d::An_import_attributes_property_must_have_a_type_annotation,
                    vec![],
                );
            };
            if read.postfix_token().is_some() {
                return self.grammar_error_node(
                    member,
                    d::An_import_attributes_property_cannot_be_optional,
                    vec![],
                );
            }
            let name = read.name().ok_or(ts_arena::Error::InvalidGraph)?;
            if !matches!(
                self.ast(name)?.node(name)?.kind().known(),
                Some(K::Identifier | K::StringLiteral | K::NoSubstitutionTemplateLiteral)
            ) {
                return self.grammar_error_node(
                    name,
                    d::An_import_attributes_property_must_have_a_string_literal_or_identifier_name,
                    vec![],
                );
            }
            let text = self.ast(name)?.node_text(name)?.into_js_string();
            if text.as_bytes() == b"resolution-mode" {
                return self.grammar_error_node(
                    name,
                    d::X_0_is_not_a_valid_key_for_an_import_attributes_type,
                    vec![text],
                );
            }
            let literal = self
                .ast(ty)?
                .node(ty)?
                .as_literal_type_node()
                .and_then(|n| n.literal());
            let valid = if let Some(literal) = literal {
                matches!(
                    self.ast(literal)?.node(literal)?.kind().known(),
                    Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
                )
            } else {
                false
            };
            if !valid {
                return self.grammar_error_node(
                    ty,
                    d::An_import_attributes_property_must_have_a_string_literal_type_annotation,
                    vec![],
                );
            }
        }
        Ok(false)
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.mergePatternAmbientModules
    pub(crate) fn merge_attributed_pattern_modules(&mut self) -> Result<(), Error> {
        let source = std::mem::take(&mut self.module_aliases.patterns);
        let mut grouped: Vec<ts_ast::PatternAmbientModule> = Vec::new();
        let mut groups: crate::types::Map<JsString, Vec<usize>> = crate::types::Map::default();
        for module in &source {
            let symbol = module
                .symbol
                .ok_or(Error::MissingLink("pattern module symbol"))?;
            let attributes = self.module_import_attributes_type(symbol)?;
            let key = JsString::from_bytes(module.pattern.text.as_ref());
            let mut matched = None;
            if let Some(indices) = groups.get(&key) {
                for &index in indices {
                    let existing = grouped[index]
                        .symbol
                        .ok_or(Error::MissingLink("grouped pattern symbol"))?;
                    let other = self.module_import_attributes_type(existing)?;
                    if self.is_type_related_to(attributes, other, crate::RelationKind::Identity)? {
                        matched = Some(index);
                        break;
                    }
                }
            }
            if let Some(index) = matched {
                let target = grouped[index]
                    .symbol
                    .ok_or(Error::MissingLink("grouped pattern symbol"))?;
                grouped[index].symbol = Some(self.merge_symbol(target, symbol, false)?);
            } else {
                groups.entry(key).or_default().push(grouped.len());
                grouped.push(module.clone());
            }
        }
        let globals = self
            .builtins
            .globals
            .ok_or(Error::MissingLink("pattern globals"))?;
        for module in source {
            let symbol = module
                .symbol
                .ok_or(Error::MissingLink("pattern module symbol"))?;
            let name = self.symbol(symbol)?.name_to_owned();
            if self.table(globals)?.get(name.as_bytes()).is_some() {
                let merged = self.get_merged_symbol(symbol);
                self.tables.get_mut(globals)?.insert(name, Some(merged));
            }
        }
        self.module_aliases.patterns = grouped;
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.tryResolvePatternAmbientModule
    pub(crate) fn resolve_attributed_pattern_module(
        &mut self,
        resolved: Option<SymbolId>,
        name: &[u8],
        attributes: TypeId,
    ) -> Result<Option<SymbolId>, Error> {
        if resolved.is_some() && self.empty_object_type(attributes)? {
            return Ok(resolved);
        }
        let patterns = self.module_aliases.patterns.clone();
        let mut candidates = Vec::new();
        for module in patterns {
            // Native fetches the declaration type before testing pattern text.
            let symbol = module
                .symbol
                .ok_or(Error::MissingLink("pattern module symbol"))?;
            let target = self.module_import_attributes_type(symbol)?;
            if module.pattern.matches(name)
                && self.is_type_related_to(attributes, target, crate::RelationKind::Assignable)?
            {
                candidates.push(module);
            }
        }
        if candidates.is_empty() {
            return Ok(resolved);
        }
        let candidate = if candidates.len() == 1 {
            candidates[0].symbol
        } else {
            let mut best = Vec::new();
            'candidate: for (index, module) in candidates.iter().enumerate() {
                let symbol = module
                    .symbol
                    .ok_or(Error::MissingLink("pattern module symbol"))?;
                let target = self.module_import_attributes_type(symbol)?;
                for (other_index, other) in candidates.iter().enumerate() {
                    let symbol = other
                        .symbol
                        .ok_or(Error::MissingLink("pattern module symbol"))?;
                    let other = self.module_import_attributes_type(symbol)?;
                    if index != other_index
                        && self.is_type_related_to(
                            other,
                            target,
                            crate::RelationKind::StrictSubtype,
                        )?
                        && !self.is_type_related_to(other, target, crate::RelationKind::Identity)?
                    {
                        continue 'candidate;
                    }
                }
                best.push(module.clone());
            }
            if best.len() == 1 {
                best[0].symbol
            } else {
                ts_core::pattern::find_best_pattern_match(
                    &best,
                    |module| module.pattern.clone(),
                    name,
                )
                .symbol
            }
        };
        let candidate =
            self.get_merged_symbol(candidate.ok_or(Error::MissingLink("best pattern module"))?);
        if self.module_aliases.pattern_targets.get(name).copied() == Some(candidate) {
            if let Some(&augmentation) = self.module_aliases.pattern_augmentations.get(name) {
                return Ok(Some(self.get_merged_symbol(augmentation)));
            }
        }
        Ok(Some(candidate))
    }
}
