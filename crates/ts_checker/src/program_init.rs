//! Program-dependent initialization after NewChecker's intrinsic prefix.

use crate::{object_flags, CheckerState, Error, TypeId};
use ts_ast::{symbol_flags as sf, utilities as ast, JsString, SymbolId, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.initializeChecker
    pub(crate) fn initialize_program(&mut self) -> Result<(), Error> {
        let globals = self.builtins.globals.ok_or(Error::MissingLink("globals"))?;
        let mut augmentations = Vec::new();
        let mut ambient_modules = Vec::new();
        for index in 0..self.program()?.host.source_file_count() {
            let file = self.program()?.host.source_file(index);
            let view = file.view();
            let source = view.source_file()?;
            augmentations.extend(source.module_augmentations()?.iter().flatten().copied());
            let source_id = file.source();
            let locals = view
                .node_binding(file.source())?
                .and_then(|binding| binding.locals);
            let external = ast::is_external_or_common_js_module(&source);
            let exports = view.result().global_exports();
            if !external {
                let symbols = match locals {
                    Some(locals) => self
                        .table(locals)?
                        .into_iter()
                        .filter_map(|(_, symbol)| symbol)
                        .collect::<Vec<_>>(),
                    None => Vec::new(),
                };
                for symbol in symbols {
                    let read = self.symbol(symbol)?;
                    if read.flags() & sf::MODULE != 0
                        && ts_ast::is_ambient_module_symbol_name(read.name_bytes())
                    {
                        ambient_modules.push(symbol);
                        continue;
                    }
                    if read.name_bytes() == b"globalThis" {
                        for declaration in self
                            .symbol_declarations(symbol)?
                            .to_vec()
                            .into_iter()
                            .flatten()
                        {
                            self.error_at(Some(declaration), ts_diagnostics::Declaration_name_conflicts_with_built_in_global_identifier_0, vec![JsString::from_bytes(&b"globalThis"[..])])?;
                        }
                    }
                    self.merge_global_symbol(symbol)?;
                }
            }
            if let Some(exports) = exports {
                let values = self
                    .table(exports)?
                    .into_iter()
                    .map(|(name, value)| (JsString::from_bytes(name), value))
                    .collect::<Vec<_>>();
                for (name, value) in values {
                    if self.tables.get(globals)?.get(name.as_bytes()).is_none() {
                        self.tables.get_mut(globals)?.insert(name, value);
                    }
                }
            }
            self.collect_pattern_ambient_modules(source_id)?;
        }
        for &name in &augmentations {
            let declaration = self
                .ast(name)?
                .node(name)?
                .parent()
                .ok_or(Error::MissingLink("augmentation parent"))?;
            if ast::is_global_scope_augmentation(&self.ast(declaration)?.node(declaration)?) {
                self.merge_global_augmentation(declaration)?;
            }
        }
        self.add_undefined_to_globals()?;
        self.value_symbol_links
            .get_or_default(self.builtins.undefined_symbol)
            .resolved_type = Some(self.builtins.undefined_widening_type);
        let arguments = self.get_global_type("IArguments", 0, true)?;
        self.value_symbol_links
            .get_or_default(self.builtins.arguments_symbol)
            .resolved_type = Some(arguments);
        self.value_symbol_links
            .get_or_default(self.builtins.unknown_symbol)
            .resolved_type = Some(self.builtins.error_type);
        let global_this = self.new_object_type(
            object_flags::ANONYMOUS,
            Some(self.builtins.global_this_symbol),
        )?;
        self.value_symbol_links
            .get_or_default(self.builtins.global_this_symbol)
            .resolved_type = Some(global_this);
        for (name, arity) in [
            ("Array", 1),
            ("Object", 0),
            ("Function", 0),
            ("CallableFunction", 0),
            ("NewableFunction", 0),
            ("String", 0),
            ("Number", 0),
            ("Boolean", 0),
            ("RegExp", 0),
        ] {
            let strict_bind_call_apply = {
                let options = self.program()?.host.options();
                options.strict_option_value(options.strict_bind_call_apply)
            };
            let ty = if matches!(name, "CallableFunction" | "NewableFunction")
                && !strict_bind_call_apply
            {
                self.query.global_types["Function"]
            } else {
                self.get_global_type(name, arity, true)?
            };
            self.query.global_types.insert(name, ty);
        }
        let array = self.query.global_types["Array"];
        let any_array = self.type_from_generic_global(array, self.builtins.any_type)?;
        let mut auto_array = self.type_from_generic_global(array, self.builtins.auto_type)?;
        if auto_array == self.builtins.empty_object_type {
            auto_array = self.new_anonymous_type(None, None, &[], &[], &[])?;
        }
        self.query.global_types.insert("anyArrayType", any_array);
        self.query.global_types.insert("autoArrayType", auto_array);
        let mut readonly = self.get_global_type("ReadonlyArray", 1, false)?;
        if readonly == self.builtins.empty_generic_type {
            readonly = array;
        }
        self.query.global_types.insert("ReadonlyArray", readonly);
        let any_readonly = self.type_from_generic_global(readonly, self.builtins.any_type)?;
        self.query
            .global_types
            .insert("anyReadonlyArrayType", any_readonly);
        let this = self.get_global_type("ThisType", 1, false)?;
        self.query.global_types.insert("ThisType", this);
        for symbol in ambient_modules {
            self.merge_global_symbol(symbol)?;
        }
        self.merge_pattern_ambient_modules()?;
        for name in augmentations {
            let declaration = self
                .ast(name)?
                .node(name)?
                .parent()
                .ok_or(Error::MissingLink("augmentation parent"))?;
            if !ast::is_global_scope_augmentation(&self.ast(declaration)?.node(declaration)?) {
                self.merge_external_module_augmentation(declaration)?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.mergeModuleAugmentation
    fn merge_global_augmentation(&mut self, declaration: ts_arena::NodeId) -> Result<(), Error> {
        // The raw bound symbol accumulates all augmentations in this file.
        // Process only its first declaration, before resolving global types.
        let symbol = self
            .program()?
            .bound(declaration)?
            .node_binding(declaration)?
            .and_then(|binding| binding.symbol)
            .ok_or(Error::MissingLink("augmentation symbol"))?;
        if self.symbol_declarations(symbol)?.first().flatten() != Some(declaration) {
            return Ok(());
        }
        if let Some(exports) = self.symbol(symbol)?.exports() {
            self.merge_symbol_table(
                self.builtins.globals.ok_or(Error::MissingLink("globals"))?,
                exports,
                false,
                None,
            )?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.addUndefinedToGlobalsOrErrorOnRedeclaration
    fn add_undefined_to_globals(&mut self) -> Result<(), Error> {
        let globals = self.builtins.globals.ok_or(Error::MissingLink("globals"))?;
        if let Some(symbol) = self.table(globals)?.get(b"undefined").flatten() {
            for declaration in self
                .symbol_declarations(symbol)?
                .to_vec()
                .into_iter()
                .flatten()
            {
                if !self.is_type_declaration(declaration)? {
                    self.error_at(Some(declaration), ts_diagnostics::Declaration_name_conflicts_with_built_in_global_identifier_0, vec![JsString::from_bytes(&b"undefined"[..])])?;
                }
            }
        } else {
            self.tables.get_mut(globals)?.insert(
                JsString::from_bytes(&b"undefined"[..]),
                Some(self.builtins.undefined_symbol),
            );
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getGlobalType
    pub(crate) fn get_global_type(
        &mut self,
        name: &'static str,
        arity: usize,
        report: bool,
    ) -> Result<TypeId, Error> {
        let symbol = self.resolve_name(
            None,
            name.as_bytes(),
            sf::TYPE,
            report.then_some(ts_diagnostics::Cannot_find_global_type_0),
            false,
        )?;
        if let Some(symbol) = symbol {
            if self.symbol(symbol)?.flags() & (sf::CLASS | sf::INTERFACE) != 0 {
                let ty = self.get_declared_type_of_symbol(symbol)?;
                if self.types.interface(ty)?.type_parameters().len() == arity {
                    return Ok(ty);
                }
                if report {
                    let declaration = self.global_type_declaration(symbol)?;
                    let name = self.symbol_to_string(symbol)?;
                    self.error_at(
                        declaration,
                        ts_diagnostics::Global_type_0_must_have_1_type_parameter_s,
                        vec![name, JsString::from_bytes(arity.to_string().as_bytes())],
                    )?;
                }
            } else if report {
                let declaration = self.global_type_declaration(symbol)?;
                let name = self.symbol_to_string(symbol)?;
                self.error_at(
                    declaration,
                    ts_diagnostics::Global_type_0_must_be_a_class_or_interface_type,
                    vec![name],
                )?;
            }
        }
        Ok(if arity != 0 {
            self.builtins.empty_generic_type
        } else {
            self.builtins.empty_object_type
        })
    }

    // port: tsc/internal/checker/checker.go:getGlobalTypeDeclaration
    fn global_type_declaration(&self, symbol: SymbolId) -> Result<Option<ts_arena::NodeId>, Error> {
        for declaration in self.symbol_declarations(symbol)?.iter().flatten() {
            if matches!(
                self.ast(declaration)?.node(declaration)?.kind().known(),
                Some(
                    K::ClassDeclaration
                        | K::InterfaceDeclaration
                        | K::EnumDeclaration
                        | K::TypeAliasDeclaration
                )
            ) {
                return Ok(Some(declaration));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.createTypeFromGenericGlobalType
    pub(crate) fn type_from_generic_global(
        &mut self,
        target: TypeId,
        argument: TypeId,
    ) -> Result<TypeId, Error> {
        if target == self.builtins.empty_generic_type {
            return Ok(self.builtins.empty_object_type);
        }
        self.create_type_reference(target, &[argument])
    }
}
