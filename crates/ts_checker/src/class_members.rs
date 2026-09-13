//! Static class members and constructor signatures. Publish the early export
//! table before resolving bases and signatures so recursive typeof reads see it.

use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::SymbolId;
use ts_ast::{internal_symbol_names as names, symbol_flags as sf, JsString, SymbolTable};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.resolveAnonymousTypeMembers
    pub(crate) fn resolve_class_static_members(
        &mut self,
        ty: TypeId,
        symbol: SymbolId,
    ) -> Result<(), Error> {
        let mut members = self.resolved_members_or_exports(symbol, true)?;
        self.set_structured_type_members(ty, members, &[], &[], &[])?;
        let class = self.declared_interface_type(symbol)?;
        let base = self.class_base_constructor_type(class)?;
        let mut base_index = None;
        if self.types.flags(base)? & (tf::OBJECT | tf::INTERSECTION | tf::TYPE_VARIABLE) != 0 {
            let mut inherited = SymbolTable::new();
            if let Some(members) = members {
                for (name, symbol) in self.table(members)? {
                    inherited.insert(JsString::from_bytes(name), symbol);
                }
            }
            for property in self.get_properties_of_type(base)? {
                let name = self.symbol(property)?.name_to_owned();
                if inherited.get(name.as_bytes()).copied().flatten().is_none() {
                    inherited.insert(name, Some(property));
                }
            }
            members = Some(self.alloc_symbol_table(inherited));
            self.set_structured_type_members(ty, members, &[], &[], &[])?;
        } else if base == self.builtins.any_type {
            base_index = Some(self.builtins.any_base_type_index_info);
        }
        let index = self.member_symbol(members, names::INDEX)?;
        let indexes = if index.is_some() {
            self.index_infos_of_symbol(index, members)?
        } else {
            base_index.into_iter().collect()
        };
        let calls = if self.symbol(symbol)?.flags() & (sf::FUNCTION | sf::METHOD) != 0 {
            self.signatures_of_symbol(Some(symbol))?
        } else {
            Vec::new()
        };
        let constructor = self.member_symbol(self.symbol(symbol)?.members(), names::CONSTRUCTOR)?;
        let mut constructs = self.signatures_of_symbol(constructor)?;
        if constructs.is_empty() {
            constructs = self.default_construct_signatures(class)?;
        }
        self.set_structured_type_members(ty, members, &calls, &constructs, &indexes)
    }
}
