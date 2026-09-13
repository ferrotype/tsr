//! Pattern modules merge in source order, then concrete augmentations apply to
//! their resolved target. Pattern augmentation copies never change the wildcard.
use crate::{CheckerState, Error};
use ts_arena::NodeId;
use ts_ast::{
    internal_symbol_names as names, node_flags as nf, symbol_flags as sf, SyntaxKind as K,
};
use ts_diagnostics as d;
impl CheckerState {
    pub(crate) fn collect_pattern_ambient_modules(&mut self, source: NodeId) -> Result<(), Error> {
        let patterns = self
            .program()?
            .bound(source)?
            .result()
            .pattern_ambient_modules()
            .to_vec();
        self.module_aliases.patterns.extend(patterns);
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.mergePatternAmbientModules
    pub(crate) fn merge_pattern_ambient_modules(&mut self) -> Result<(), Error> {
        self.merge_attributed_pattern_modules()
    }
    // port: tsc/internal/checker/checker.go:Checker.mergeModuleAugmentation
    pub(crate) fn merge_external_module_augmentation(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let name = read
            .name()
            .ok_or(Error::MissingLink("augmentation module name"))?;
        let parent = read
            .parent()
            .ok_or(Error::MissingLink("augmentation parent"))?;
        let symbol = self
            .program()?
            .bound(node)?
            .node_binding(node)?
            .and_then(|binding| binding.symbol)
            .ok_or(Error::MissingLink("augmentation symbol"))?;
        if self.symbol_declarations(symbol)?.first().flatten() != Some(node) {
            return Ok(());
        }
        let message = (self.ast(parent)?.node(parent)?.flags() & nf::AMBIENT == 0)
            .then_some(d::Invalid_module_name_in_augmentation_module_0_cannot_be_found);
        let main =
            self.resolve_external_module_name_with_error(name, name, false, message, true)?;
        let Some(main) = self.resolve_external_module_symbol(main, false)? else {
            return Ok(());
        };
        let text = self.ast(name)?.node_text(name)?.into_js_string();
        if self.symbol(main)?.flags() & sf::NAMESPACE == 0 {
            self.error_at(
                Some(name),
                d::Cannot_augment_module_0_because_it_resolves_to_a_non_module_entity,
                vec![text],
            )?;
            return Ok(());
        }
        let is_pattern = self
            .module_aliases
            .patterns
            .iter()
            .filter_map(|p| p.symbol)
            .any(|symbol| self.get_merged_symbol(symbol) == main);
        if is_pattern {
            let merged = self.merge_symbol(symbol, main, true)?;
            self.module_aliases
                .pattern_augmentations
                .insert(text.clone(), merged);
            self.module_aliases.pattern_targets.insert(text, main);
        } else {
            let main_exports = self.symbol(main)?.exports();
            if self
                .member_symbol(main_exports, names::EXPORT_STAR)?
                .is_some()
            {
                if let Some(augmentation_exports) = self.symbol(symbol)?.exports() {
                    let resolved = self.module_exports(main)?;
                    for (key, value) in self.module_table_entries(augmentation_exports)? {
                        if let (Some(value), Some(target)) =
                            (value, self.member_symbol(Some(resolved), key.as_bytes())?)
                        {
                            if self.member_symbol(main_exports, key.as_bytes())?.is_none() {
                                self.merge_symbol(target, value, false)?;
                            }
                        }
                    }
                }
            }
            self.merge_symbol(main, symbol, false)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkModuleAugmentationElement
    pub(crate) fn check_module_augmentation_element(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        match read.kind().known() {
            Some(K::VariableStatement) => {
                let list = read
                    .data_source()
                    .as_variable_statement()
                    .and_then(|d| d.declaration_list())
                    .ok_or(Error::MissingLink("augmentation declarations"))?;
                for declaration in self.source_list(
                    list,
                    self.ast(list)?
                        .node(list)?
                        .data_source()
                        .as_variable_declaration_list()
                        .and_then(|d| d.declarations()),
                )? {
                    self.check_module_augmentation_element(declaration)?;
                }
            }
            Some(K::ExportAssignment | K::ExportDeclaration) => {
                self.grammar_error_first_token(
                    node,
                    d::Exports_and_export_assignments_are_not_permitted_in_module_augmentations,
                    vec![],
                )?;
            }
            Some(K::ImportDeclaration | K::JSImportDeclaration) => {
                self.grammar_error_first_token(node,d::Imports_are_not_permitted_in_module_augmentations_Consider_moving_them_to_the_enclosing_external_module,vec![])?;
            }
            Some(K::ImportEqualsDeclaration) => {
                let reference = read
                    .data_source()
                    .as_import_equals_declaration()
                    .and_then(|d| d.module_reference())
                    .ok_or(Error::MissingLink("augmentation import reference"))?;
                if self.ast(reference)?.node(reference)?.kind() == K::ExternalModuleReference {
                    self.grammar_error_first_token(node,d::Imports_are_not_permitted_in_module_augmentations_Consider_moving_them_to_the_enclosing_external_module,vec![])?;
                }
            }
            Some(K::BindingElement | K::VariableDeclaration) => {
                if let Some(name) = read.name() {
                    if self.is_binding_pattern(name)? {
                        for element in
                            self.source_list(name, self.ast(name)?.node(name)?.element_list())?
                        {
                            self.check_module_augmentation_element(element)?;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
