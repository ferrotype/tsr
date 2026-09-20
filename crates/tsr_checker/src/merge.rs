//! Source symbols are immutable. Merging first creates a checker-owned transient
//! record with private tables; its declaration header shares capped read-only
//! backing until an append actually needs private storage.

use crate::{CheckerState, Error};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{
    symbol_flags as flags, DeclarationSlice, SymbolFlags, SymbolRef, SymbolTable, SymbolTableId,
    SyntaxKind,
};
use tsr_diagnostics as messages;

// port: tsc/internal/checker/checker.go:getExcludedSymbolFlags
pub(crate) fn excluded_symbol_flags(value: SymbolFlags) -> SymbolFlags {
    let mut result = 0;
    for (flag, excluded) in [
        (
            flags::BLOCK_SCOPED_VARIABLE,
            flags::BLOCK_SCOPED_VARIABLE_EXCLUDES,
        ),
        (
            flags::FUNCTION_SCOPED_VARIABLE,
            flags::FUNCTION_SCOPED_VARIABLE_EXCLUDES,
        ),
        (flags::PROPERTY, flags::PROPERTY_EXCLUDES),
        (flags::ENUM_MEMBER, flags::ENUM_MEMBER_EXCLUDES),
        (flags::FUNCTION, flags::FUNCTION_EXCLUDES),
        (flags::CLASS, flags::CLASS_EXCLUDES),
        (flags::INTERFACE, flags::INTERFACE_EXCLUDES),
        (flags::REGULAR_ENUM, flags::REGULAR_ENUM_EXCLUDES),
        (flags::CONST_ENUM, flags::CONST_ENUM_EXCLUDES),
        (flags::VALUE_MODULE, flags::VALUE_MODULE_EXCLUDES),
        (flags::METHOD, flags::METHOD_EXCLUDES),
        (flags::GET_ACCESSOR, flags::GET_ACCESSOR_EXCLUDES),
        (flags::SET_ACCESSOR, flags::SET_ACCESSOR_EXCLUDES),
        (flags::TYPE_PARAMETER, flags::TYPE_PARAMETER_EXCLUDES),
        (flags::TYPE_ALIAS, flags::TYPE_ALIAS_EXCLUDES),
        (flags::ALIAS, flags::ALIAS_EXCLUDES),
    ] {
        if value & flag != 0 {
            result |= excluded;
        }
    }
    if value & flags::REPLACEABLE_BY_METHOD != 0 {
        result &= !flags::METHOD;
    }
    result
}

fn compatible(target: SymbolFlags, source: SymbolFlags) -> bool {
    target & excluded_symbol_flags(source) == 0 || (target | source) & flags::ASSIGNMENT != 0
}

/// One table's names in a single buffer, each entry a byte range and its symbol.
pub(crate) type TableEntries = (Vec<u8>, Vec<(std::ops::Range<usize>, Option<SymbolId>)>);

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getMergedSymbol
    pub(crate) fn get_merged_symbol(&self, symbol: SymbolId) -> SymbolId {
        self.merged_symbols.get(&symbol).copied().unwrap_or(symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.cloneSymbol
    pub(crate) fn clone_symbol(&mut self, source: SymbolId) -> Result<SymbolId, Error> {
        let symbol = self.symbol(source)?;
        let (flags, name, declarations, parent, value, members, exports) = (
            symbol.flags(),
            symbol.name_to_owned(),
            symbol.declarations(),
            symbol.parent(),
            symbol.value_declaration(),
            symbol.members(),
            symbol.exports(),
        );
        // Validate the shared slice before retaining its capped header.
        self.declaration_slice(declarations)?;
        let declarations =
            declarations.slice_with_capacity(0..declarations.len(), declarations.len())?;
        let clone = self.new_symbol(flags, name)?;
        let members = self.clone_symbol_table(members)?;
        let exports = self.clone_symbol_table(exports)?;
        let target = self.symbol_mut(clone)?;
        target.declarations = declarations;
        target.parent = parent;
        target.value_declaration = value;
        target.members = members;
        target.exports = exports;
        // newSymbol deliberately leaves CheckFlags, ExportSymbol, and the lazy
        // runtime identity clear. Copying the entire symbol would violate that.
        self.merged_symbols.insert(source, clone);
        Ok(clone)
    }

    /// A checker-owned copy of `source` (Go's `maps.Clone` of a member or
    /// export table). A table this checker already owns is copied wholesale;
    /// a binder-owned one is interned into a table sized for it. Neither path
    /// builds an intermediate owned map or an owning string per name.
    pub(crate) fn clone_symbol_table(
        &mut self,
        source: Option<SymbolTableId>,
    ) -> Result<Option<SymbolTableId>, Error> {
        let Some(source) = source else {
            return Ok(None);
        };
        if source.arena() == self.tables.id() {
            return Ok(Some(self.tables.clone_own(source)?));
        }
        let program = self
            .program
            .as_ref()
            .ok_or(Error::Unsupported("query without a checker program"))?;
        Ok(Some(self.tables.clone_foreign(program.table(source)?)))
    }

    /// The entries of a table copied into one byte buffer, for a merge that
    /// mutates the checker while it walks them: one allocation per table
    /// instead of an owning string per entry.
    pub(crate) fn collect_table_entries(
        &self,
        table: SymbolTableId,
    ) -> Result<TableEntries, Error> {
        let read = self.table(table)?;
        let mut bytes = Vec::with_capacity(read.len() * 12);
        let mut entries = Vec::with_capacity(read.len());
        for (name, symbol) in read {
            let start = bytes.len();
            bytes.extend_from_slice(name);
            entries.push((start..bytes.len(), symbol));
        }
        Ok((bytes, entries))
    }

    // port: tsc/internal/checker/checker.go:Checker.mergeGlobalSymbol
    pub(crate) fn merge_global_symbol(&mut self, symbol: SymbolId) -> Result<(), Error> {
        let globals = self
            .builtins
            .globals
            .ok_or(Error::MissingLink("checker globals"))?;
        // The name is hashed once for the lookup and the insert, and read in
        // place both times: the merge in between may touch anything, so the
        // insert borrows the pool the name lives in field by field rather than
        // copying the name out.
        let (hash, target) = {
            let name = self.symbol(symbol)?;
            let name = name.name_bytes();
            let hash = tsr_ast::name_hash(name);
            (hash, self.table(globals)?.get_hashed(name, hash).flatten())
        };
        let merged = if let Some(target) = target {
            self.merge_symbol(target, symbol, false)?
        } else {
            self.get_merged_symbol(symbol)
        };
        let CheckerState {
            program,
            symbols,
            tables,
            ..
        } = self;
        let name = if symbol.arena() == symbols.id() {
            SymbolRef::Owned(symbols.get(symbol)?)
        } else {
            program
                .as_ref()
                .ok_or(Error::Unsupported("query without a checker program"))?
                .symbol(symbol)?
        };
        tables
            .get_mut(globals)?
            .insert_bytes_hashed(name.name_bytes(), hash, Some(merged));
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.mergeSymbolTable
    pub(crate) fn merge_symbol_table(
        &mut self,
        target: SymbolTableId,
        source: SymbolTableId,
        unidirectional: bool,
        merged_parent: Option<SymbolId>,
    ) -> Result<(), Error> {
        // Reject a source-owned mutation target before any recursive merge.
        self.tables.get(target)?;
        let (bytes, entries) = self.collect_table_entries(source)?;
        for (range, source_symbol) in entries {
            let name = &bytes[range];
            let target_symbol = self.table(target)?.get(name).flatten();
            let merged = match (target_symbol, source_symbol) {
                (Some(target), Some(source)) => {
                    Some(self.merge_symbol(target, source, unidirectional)?)
                }
                (None, source) => source.map(|symbol| self.get_merged_symbol(symbol)),
                (Some(_), None) => {
                    return Err(Error::MissingLink("mergeSymbolTable source symbol"));
                }
            };
            if let (Some(parent), Some(_), Some(merged)) = (merged_parent, target_symbol, merged) {
                if self.symbol(merged)?.flags() & flags::TRANSIENT != 0 {
                    self.symbol_mut(merged)?.parent = Some(parent);
                }
            }
            self.tables.get_mut(target)?.insert_bytes(name, merged);
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.mergeSymbol
    pub(crate) fn merge_symbol(
        &mut self,
        mut target: SymbolId,
        source: SymbolId,
        unidirectional: bool,
    ) -> Result<SymbolId, Error> {
        let mut target_flags = self.symbol(target)?.flags();
        let source_flags = self.symbol(source)?.flags();
        if !compatible(target_flags, source_flags) {
            if target_flags & flags::NAMESPACE_MODULE != 0 {
                if target != self.builtins.global_this_symbol {
                    let declaration = self.first_symbol_declaration(source)?;
                    let node = if let Some(declaration) = declaration {
                        tsr_ast::get_name_of_declaration(self.ast(declaration)?, Some(declaration))?
                    } else {
                        None
                    };
                    let name = self.symbol_to_string(target)?;
                    self.error_at(node, messages::Cannot_augment_module_0_with_value_exports_because_it_resolves_to_a_non_module_entity, vec![name])?;
                }
            } else {
                self.report_merge_symbol_error(target, source)?;
            }
            return Ok(target);
        }
        if target == source {
            return Ok(target);
        }
        if target_flags & flags::TRANSIENT == 0 {
            let symbol = self.symbol(target)?;
            let resolved_target = if tsr_ast::is_non_local_alias(
                Some(&symbol),
                flags::VALUE | flags::TYPE | flags::NAMESPACE,
            ) {
                // port: tsc/internal/checker/checker.go:Checker.resolveSymbol
                self.resolve_alias(target)?
            } else {
                target
            };
            if resolved_target == self.builtins.unknown_symbol {
                return Ok(source);
            }
            if !compatible(self.symbol(resolved_target)?.flags(), source_flags) {
                self.report_merge_symbol_error(target, source)?;
                return Ok(source);
            }
            target = self.clone_symbol(resolved_target)?;
            target_flags = self.symbol(target)?.flags();
        }
        if source_flags & flags::VALUE_MODULE != 0
            && target_flags & flags::VALUE_MODULE != 0
            && target_flags & flags::CONST_ENUM_ONLY_MODULE != 0
            && source_flags & flags::CONST_ENUM_ONLY_MODULE == 0
        {
            target_flags &= !flags::CONST_ENUM_ONLY_MODULE;
        }
        let added_flags = if target_flags & flags::CONST_ENUM_ONLY_MODULE == 0 {
            source_flags & !flags::CONST_ENUM_ONLY_MODULE
        } else {
            source_flags
        };
        self.symbol_mut(target)?.flags = target_flags | added_flags;
        if let Some(value) = self.symbol(source)?.value_declaration() {
            self.set_merged_value_declaration(target, value)?;
        }
        let source_declarations = self.symbol_declarations(source)?.to_vec();
        let target_declarations = self.symbol(target)?.declarations();
        let combined =
            self.append_merged_declarations(target_declarations, &source_declarations)?;
        self.symbol_mut(target)?.declarations = combined;
        if let Some(source_members) = self.symbol(source)?.members() {
            let members = if let Some(table) = self.symbol(target)?.members() {
                table
            } else {
                let table = self.alloc_symbol_table(SymbolTable::default());
                self.symbol_mut(target)?.members = Some(table);
                table
            };
            self.merge_symbol_table(members, source_members, unidirectional, None)?;
        }
        if let Some(source_exports) = self.symbol(source)?.exports() {
            let exports = if let Some(table) = self.symbol(target)?.exports() {
                table
            } else {
                let table = self.alloc_symbol_table(SymbolTable::default());
                self.symbol_mut(target)?.exports = Some(table);
                table
            };
            self.merge_symbol_table(exports, source_exports, unidirectional, Some(target))?;
        }
        if !unidirectional {
            self.merged_symbols.insert(source, target);
        }
        Ok(target)
    }

    fn append_merged_declarations(
        &mut self,
        target: DeclarationSlice,
        source: &[Option<NodeId>],
    ) -> Result<DeclarationSlice, Error> {
        if source.is_empty() {
            return Ok(target);
        }
        if target
            .backing_id()
            .is_none_or(|id| id.arena() == self.declarations.id())
        {
            return Ok(self.declarations.append_all(target, source)?);
        }
        let program = self.program.as_ref().ok_or(Error::Unsupported(
            "merge declarations without a checker program",
        ))?;
        let existing = program.declarations(target)?;
        Ok(self
            .declarations
            .append_imported(target, existing, source)?)
    }

    // port: tsc/internal/binder/binder.go:SetValueDeclaration
    pub(crate) fn set_merged_value_declaration(
        &mut self,
        symbol: SymbolId,
        node: NodeId,
    ) -> Result<(), Error> {
        let previous = self.symbol(symbol)?.value_declaration();
        let replace = if let Some(previous) = previous {
            let old_kind = self.node(previous)?.kind();
            let new_kind = self.node(node)?.kind();
            let assignment = |kind: tsr_ast::NodeKind| {
                matches!(
                    kind.known(),
                    Some(
                        SyntaxKind::BinaryExpression
                            | SyntaxKind::PropertyAccessExpression
                            | SyntaxKind::ElementAccessExpression
                            | SyntaxKind::Identifier
                            | SyntaxKind::CallExpression
                    )
                )
            };
            assignment(old_kind) && !assignment(new_kind)
                || old_kind != new_kind
                    && matches!(
                        old_kind.known(),
                        Some(SyntaxKind::ModuleDeclaration | SyntaxKind::Identifier)
                    )
        } else {
            true
        };
        if replace {
            self.symbol_mut(symbol)?.value_declaration = Some(node);
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.reportMergeSymbolError
    fn report_merge_symbol_error(
        &mut self,
        target: SymbolId,
        source: SymbolId,
    ) -> Result<(), Error> {
        let combined = self.symbol(target)?.flags() | self.symbol(source)?.flags();
        let message = if combined & flags::ENUM != 0 {
            messages::Enum_declarations_can_only_merge_with_namespace_or_other_enum_declarations
        } else if combined & flags::BLOCK_SCOPED_VARIABLE != 0 {
            messages::Cannot_redeclare_block_scoped_variable_0
        } else {
            messages::Duplicate_identifier_0
        };
        let source_plain_js = self.symbol_is_plain_js(source)?;
        let target_plain_js = self.symbol_is_plain_js(target)?;
        let name = self.symbol_to_string(source)?;
        if !source_plain_js {
            self.duplicate_symbol_errors(source, message, &name, target)?;
        }
        if !target_plain_js {
            self.duplicate_symbol_errors(target, message, &name, source)?;
        }
        Ok(())
    }

    fn symbol_is_plain_js(&self, symbol: SymbolId) -> Result<bool, Error> {
        let Some(node) = self.first_symbol_declaration(symbol)? else {
            return Ok(false);
        };
        let view = self.ast(node)?;
        let Some(file) = tsr_ast::utilities::get_source_file_of_node(view, Some(node))? else {
            return Ok(false);
        };
        Ok(tsr_ast::utilities_middle::is_plain_js_file(
            Some(&*view.source_file(file)?),
            self.program()?.host.options().check_js,
        ))
    }
}
