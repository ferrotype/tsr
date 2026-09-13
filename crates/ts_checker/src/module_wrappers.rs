//! Namespace imports retain their original symbol while their anonymous module
//! type omits call/construct signatures and may expose a synthetic default.
use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    internal_symbol_names as names, symbol_flags as sf, Diagnostic, JsString, SymbolTable,
};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getTypeWithSyntheticDefaultOnly
    pub(crate) fn module_default_only_type(
        &mut self,
        ty: TypeId,
        symbol: SymbolId,
        original: SymbolId,
        specifier: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        if !self.module_only_importable_as_default(specifier, None)? || self.is_error_type(ty)? {
            return Ok(None);
        }
        if let Some(&cached) = self.module_aliases.default_only_types.get(&ty) {
            return Ok(Some(cached));
        }
        let result = self.default_property_wrapper_for_module(symbol, Some(original), None)?;
        self.module_aliases.default_only_types.insert(ty, result);
        Ok(Some(result))
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeWithSyntheticDefaultImportType
    pub(crate) fn module_synthetic_default_type(
        &mut self,
        ty: TypeId,
        symbol: SymbolId,
        original: SymbolId,
        specifier: NodeId,
    ) -> Result<TypeId, Error> {
        if self.is_error_type(ty)? {
            return Ok(ty);
        }
        if let Some(&cached) = self.module_aliases.synthetic_types.get(&ty) {
            return Ok(cached);
        }
        let result = if self.module_can_have_synthetic_default(original, specifier, false)? {
            let anonymous = self.new_symbol(sf::TYPE_LITERAL, JsString::from_bytes(names::TYPE))?;
            let declarations = self.symbol(original)?.declarations();
            self.symbol_mut(anonymous)?.declarations = declarations;
            let defaults =
                self.default_property_wrapper_for_module(symbol, Some(original), Some(anonymous))?;
            self.value_symbol_links
                .get_or_default(anonymous)
                .resolved_type = Some(defaults);
            if self.valid_spread_type(ty)? {
                self.object_spread_type(ty, defaults, Some(anonymous), 0, false)?
            } else {
                defaults
            }
        } else {
            ty
        };
        self.module_aliases.synthetic_types.insert(ty, result);
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.createDefaultPropertyWrapperForModule
    pub(crate) fn default_property_wrapper_for_module(
        &mut self,
        symbol: SymbolId,
        original: Option<SymbolId>,
        mut anonymous: Option<SymbolId>,
    ) -> Result<TypeId, Error> {
        let default = self.new_symbol(sf::ALIAS, JsString::from_bytes(names::DEFAULT))?;
        self.symbol_mut(default)?.parent = original;
        let name_type = self.get_string_literal_type(JsString::from_bytes(names::DEFAULT))?;
        self.value_symbol_links.get_or_default(default).name_type = Some(name_type);
        let target = self
            .resolve_module_symbol(Some(symbol), false)?
            .ok_or(Error::MissingLink("module default alias target"))?;
        self.module_aliases.targets.insert(default, Ok(target));
        let mut members = SymbolTable::new();
        members.insert(JsString::from_bytes(names::DEFAULT), Some(default));
        if anonymous.is_none() {
            if let Some(original) = original {
                let created =
                    self.new_symbol(sf::OBJECT_LITERAL, JsString::from_bytes(names::OBJECT))?;
                let declarations = self.symbol(original)?.declarations();
                self.symbol_mut(created)?.declarations = declarations;
                anonymous = Some(created);
            }
        }
        let table = self.alloc_symbol_table(members);
        self.new_anonymous_type(anonymous, Some(table), &[], &[], &[])
    }
    // port: tsc/internal/checker/checker.go:Checker.cloneTypeAsModuleType
    pub(crate) fn clone_type_as_module_type(
        &mut self,
        symbol: SymbolId,
        module_type: TypeId,
        import: NodeId,
    ) -> Result<SymbolId, Error> {
        let read = self.symbol(symbol)?;
        let (flags, name, declarations, value, members, exports, parent) = (
            read.flags(),
            read.name_to_owned(),
            read.declarations(),
            read.value_declaration(),
            read.members(),
            read.exports(),
            read.parent(),
        );
        let declarations =
            declarations.slice_with_capacity(0..declarations.len(), declarations.len())?;
        let members = self.clone_module_wrapper_table(members)?;
        let exports = self.clone_module_wrapper_table(exports)?;
        let result = self.new_symbol(flags, name)?;
        let record = self.symbol_mut(result)?;
        record.declarations = declarations;
        record.value_declaration = value;
        record.members = members;
        record.exports = exports;
        record.parent = parent;
        // This is not a merge: the original symbol must keep its own identity.
        self.module_aliases
            .export_types
            .insert(result, (symbol, import));
        self.resolve_type_members(module_type)?;
        let resolved = self.types.structured(module_type)?;
        let members = resolved.members;
        let indexes = resolved.index_infos.as_deref().unwrap_or_default().to_vec();
        let ty = self.new_anonymous_type(Some(result), members, &[], &[], &indexes)?;
        self.value_symbol_links.get_or_default(result).resolved_type = Some(ty);
        Ok(result)
    }
    fn clone_module_wrapper_table(
        &mut self,
        table: Option<ts_ast::SymbolTableId>,
    ) -> Result<Option<ts_ast::SymbolTableId>, Error> {
        table
            .map(|table| {
                let copied = self
                    .table(table)?
                    .iter()
                    .map(|(name, symbol)| (JsString::from_bytes(name), symbol))
                    .collect();
                Ok(self.alloc_symbol_table(copied))
            })
            .transpose()
    }
    // port: tsc/internal/checker/checker.go:Checker.invocationErrorRecovery
    pub(crate) fn module_invocation_error_related(
        &mut self,
        apparent: TypeId,
        construct: bool,
    ) -> Result<Option<Diagnostic>, Error> {
        let Some(symbol) = self.types.get(apparent)?.symbol else {
            return Ok(None);
        };
        let Some(&(target, import)) = self.module_aliases.export_types.get(&symbol) else {
            return Ok(None);
        };
        if self.ast(import)?.node(import)?.kind() == ts_ast::SyntaxKind::CallExpression {
            return Ok(None);
        }
        let ty = self.get_type_of_symbol(target)?;
        if self.signatures_of_type(ty, construct)?.is_empty() {
            return Ok(None);
        }
        self.diagnostic_for_node(Some(import), ts_diagnostics::Type_originates_at_this_import_A_namespace_style_import_cannot_be_called_or_constructed_and_will_cause_a_failure_at_runtime_Consider_using_a_default_import_or_import_require_here_instead, vec![]).map(Some)
    }
    pub(crate) fn module_has_signatures(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::STRUCTURED_TYPE == 0 {
            return Ok(false);
        }
        Ok(!self.signatures_of_type(ty, false)?.is_empty()
            || !self.signatures_of_type(ty, true)?.is_empty())
    }
}
