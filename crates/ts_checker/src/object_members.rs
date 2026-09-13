//! Declared, inherited and instantiated object members. Publish the early
//! member state only for recursive reads in the current operation; errors clear
//! its completion flag, so a later operation retries rather than using a prefix.

use crate::{
    object_flags as of, type_flags as tf, CheckerState, Error, IndexInfoId, MapperId, SignatureId,
    TypeId,
};
use ts_arena::SymbolId;
use ts_ast::{internal_symbol_names as names, symbol_flags as sf, SymbolTable};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.resolveStructuredTypeMembers
    pub(crate) fn resolve_type_members(&mut self, ty: TypeId) -> Result<(), Error> {
        let record = *self.types.get(ty)?;
        if record.object_flags & of::MEMBERS_RESOLVED != 0 {
            return Ok(());
        }
        let result = if record.object_flags & of::REFERENCE != 0 {
            let target = self.types.target(ty)?;
            let parameters = self
                .types
                .interface(target)?
                .all_type_parameters
                .clone()
                .unwrap_or_else(|| [].into());
            let mut arguments = self.get_type_arguments(ty)?.to_vec();
            if arguments.len() + 1 == parameters.len() {
                arguments.push(ty);
            }
            self.resolve_object_type_members(ty, target, &parameters, &arguments)
        } else if record.object_flags & of::CLASS_OR_INTERFACE != 0 {
            self.resolve_object_type_members(ty, ty, &[], &[])
        } else if record.object_flags & of::REVERSE_MAPPED != 0 {
            self.resolve_reverse_mapped_members(ty)
        } else if record.object_flags & of::ANONYMOUS != 0 {
            self.resolve_anonymous_type_members(ty)
        } else if record.object_flags & of::MAPPED != 0 {
            self.resolve_mapped_members(ty)
        } else if record.flags & tf::UNION_OR_INTERSECTION != 0 {
            self.resolve_compound_type_members(ty)
        } else {
            Err(Error::Unsupported(
                "resolveStructuredTypeMembers: type family",
            ))
        };
        if result.is_err() {
            self.types.get_mut(ty)?.object_flags &=
                !(of::MEMBERS_RESOLVED | of::UNRESOLVED_MEMBERS);
        }
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveDeclaredMembers
    fn resolve_declared_members(&mut self, ty: TypeId) -> Result<(), Error> {
        if self.types.interface(ty)?.declared_members_resolved {
            return Ok(());
        }
        let symbol = self
            .types
            .get(ty)?
            .symbol
            .ok_or(Error::MissingLink("declared member symbol"))?;
        let symbol = self.get_merged_symbol(symbol);
        let members = self.members_of_symbol(symbol)?;
        self.types.interface_mut(ty)?.declared_members_resolved = true;
        self.types.interface_mut(ty)?.declared_members = members;
        let result = (|| {
            let call_symbol = self.member_symbol(members, names::CALL)?;
            let construct_symbol = self.member_symbol(members, names::NEW)?;
            let calls = self.signatures_of_symbol(call_symbol)?;
            let constructs = self.signatures_of_symbol(construct_symbol)?;
            let index = self.member_symbol(members, names::INDEX)?;
            let indexes = self.index_infos_of_symbol(index, members)?;
            let interface = self.types.interface_mut(ty)?;
            interface.declared_call_signatures = (!calls.is_empty()).then(|| calls.into());
            interface.declared_construct_signatures =
                (!constructs.is_empty()).then(|| constructs.into());
            interface.declared_index_infos = (!indexes.is_empty()).then(|| indexes.into());
            Ok(())
        })();
        if result.is_err() {
            self.types.interface_mut(ty)?.declared_members_resolved = false;
        }
        result
    }

    pub(crate) fn member_symbol(
        &self,
        table: Option<ts_ast::SymbolTableId>,
        name: &[u8],
    ) -> Result<Option<SymbolId>, Error> {
        match table {
            Some(table) => Ok(self.table(table)?.get(name).flatten()),
            None => Ok(None),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateIndexInfo
    fn instantiate_index_infos(
        &mut self,
        indexes: &[IndexInfoId],
        mapper: MapperId,
    ) -> Result<Vec<IndexInfoId>, Error> {
        let mut result = Vec::with_capacity(indexes.len());
        for &index in indexes {
            let info = self.signatures.index_info(index)?.clone();
            let value = self.instantiate_type(info.value_type, Some(mapper))?;
            result.push(if value == info.value_type {
                index
            } else {
                self.signatures.new_index_info(
                    info.key_type,
                    value,
                    info.is_readonly,
                    info.declaration,
                    info.components,
                )?
            });
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveObjectTypeMembers
    fn resolve_object_type_members(
        &mut self,
        ty: TypeId,
        source: TypeId,
        parameters: &[TypeId],
        arguments: &[TypeId],
    ) -> Result<(), Error> {
        self.resolve_declared_members(source)?;
        let declared = self.types.interface(source)?;
        let mut members = declared.declared_members;
        let mut calls = declared
            .declared_call_signatures
            .as_deref()
            .unwrap_or_default()
            .to_vec();
        let mut constructs = declared
            .declared_construct_signatures
            .as_deref()
            .unwrap_or_default()
            .to_vec();
        let mut indexes = declared
            .declared_index_infos
            .as_deref()
            .unwrap_or_default()
            .to_vec();
        let mapper = if parameters == arguments {
            None
        } else {
            Some(self.new_type_mapper(parameters, arguments)?)
        };
        if let Some(mapper) = mapper {
            let mut table = SymbolTable::new();
            if let Some(members) = members {
                let entries = self
                    .table(members)?
                    .into_iter()
                    .map(|(name, symbol)| (ts_ast::JsString::from_bytes(name), symbol))
                    .collect::<Vec<_>>();
                for (name, symbol) in entries {
                    if let Some(symbol) = symbol {
                        if self.is_named_member(symbol, name.as_bytes())? {
                            table.insert(name, Some(self.instantiate_symbol(symbol, mapper)?));
                        }
                    }
                }
            }
            members = (!table.is_empty()).then(|| self.alloc_symbol_table(table));
            for call in &mut calls {
                *call = self.instantiate_signature(*call, mapper)?;
            }
            for construct in &mut constructs {
                *construct = self.instantiate_signature(*construct, mapper)?;
            }
            indexes = self.instantiate_index_infos(&indexes, mapper)?;
        }
        let bases = self.interface_base_types(source)?;
        if !bases.is_empty() {
            let mut table = SymbolTable::new();
            if let Some(members) = members {
                for (name, symbol) in self.table(members)? {
                    table.insert(ts_ast::JsString::from_bytes(name), symbol);
                }
            }
            members = Some(self.alloc_symbol_table(table));
            self.set_structured_type_members(ty, members, &calls, &constructs, &indexes)?;
            self.types.get_mut(ty)?.object_flags |= of::UNRESOLVED_MEMBERS;
            for &base in bases.iter() {
                let base = self.instantiate_type(base, mapper)?;
                let base = if let Some(&this) = arguments.last() {
                    self.get_type_with_this_argument(base, this, false)?
                } else {
                    base
                };
                for property in self.get_properties_of_type(base)? {
                    let name = self.symbol(property)?.name_to_owned();
                    let mut table = self
                        .tables
                        .get_mut(members.expect("inherited table created above"))?;
                    if table.get(name.as_bytes()).flatten().is_none() {
                        table.insert(name, Some(property));
                    }
                }
                // A class extending an `any` base records `any` as its base type;
                // upstream reads its signatures and index infos through the
                // type-level accessors rather than resolving structured members.
                calls.extend(self.signatures_of_type(base, false)?);
                constructs.extend(self.signatures_of_type(base, true)?);
                let inherited = if base == self.builtins.any_type {
                    vec![self.builtins.any_base_type_index_info]
                } else {
                    self.index_infos_of_type(base)?
                };
                for index in inherited {
                    let key = self.signatures.index_info(index)?.key_type;
                    let mut found = false;
                    for &existing in &indexes {
                        found |= self.signatures.index_info(existing)?.key_type == key;
                    }
                    if !found {
                        indexes.push(index);
                    }
                }
            }
            self.types.get_mut(ty)?.object_flags &= !of::UNRESOLVED_MEMBERS;
        }
        self.set_structured_type_members(ty, members, &calls, &constructs, &indexes)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveAnonymousTypeMembers
    fn resolve_anonymous_type_members(&mut self, ty: TypeId) -> Result<(), Error> {
        let object = self.types.object(ty)?;
        let target = object.target;
        let mapper = object.mapper;
        if let Some(target) = target {
            self.set_structured_type_members(ty, None, &[], &[], &[])?;
            let mapper = mapper.ok_or(Error::MissingLink("anonymous instantiation mapper"))?;
            let mut table = SymbolTable::new();
            for property in self.get_properties_of_type(target)? {
                let instantiated = self.instantiate_symbol(property, mapper)?;
                table.insert(self.symbol(property)?.name_to_owned(), Some(instantiated));
            }
            self.resolve_type_members(target)?;
            let structured = self.types.structured(target)?;
            let signatures = structured.signatures.clone().unwrap_or_else(|| [].into());
            let count = structured.call_signature_count as usize;
            let index_infos = structured.index_infos.clone().unwrap_or_else(|| [].into());
            let mut calls = Vec::new();
            let mut constructs = Vec::new();
            for (index, &signature) in signatures.iter().enumerate() {
                let signature = self.instantiate_signature(signature, mapper)?;
                if index < count {
                    calls.push(signature);
                } else {
                    constructs.push(signature);
                }
            }
            let indexes = self.instantiate_index_infos(&index_infos, mapper)?;
            let members = (!table.is_empty()).then(|| self.alloc_symbol_table(table));
            return self.set_structured_type_members(ty, members, &calls, &constructs, &indexes);
        }
        let symbol = self.get_merged_symbol(
            self.types
                .get(ty)?
                .symbol
                .ok_or(Error::MissingLink("anonymous type symbol"))?,
        );
        let read = self.symbol(symbol)?;
        let flags = read.flags();
        let exports = read.exports();
        if flags & sf::CLASS != 0 {
            return self.resolve_class_static_members(ty, symbol);
        }
        if flags & sf::ENUM != 0 {
            return self.resolve_enum_members(ty, symbol);
        }
        if flags & sf::VALUE_MODULE != 0 && flags & (sf::FUNCTION | sf::METHOD) == 0 {
            return self.resolve_namespace_type_members(ty, symbol);
        }
        let members = if flags & sf::TYPE_LITERAL != 0 {
            self.members_of_symbol(symbol)?
        } else {
            exports
        };
        if flags & (sf::TYPE_LITERAL | sf::FUNCTION | sf::METHOD) == 0 {
            return Err(Error::Unsupported(
                "resolveAnonymousTypeMembers: class/enum/module",
            ));
        }
        self.set_structured_type_members(ty, members, &[], &[], &[])?;
        let call_symbol = if flags & (sf::FUNCTION | sf::METHOD) != 0 {
            Some(symbol)
        } else {
            self.member_symbol(members, names::CALL)?
        };
        let construct_symbol = self.member_symbol(members, names::NEW)?;
        let index_symbol = self.member_symbol(members, names::INDEX)?;
        let calls = self.signatures_of_symbol(call_symbol)?;
        let constructs = self.signatures_of_symbol(construct_symbol)?;
        let indexes = self.index_infos_of_symbol(index_symbol, members)?;
        self.set_structured_type_members(ty, members, &calls, &constructs, &indexes)
    }

    pub(crate) fn signatures_of_type(
        &mut self,
        ty: TypeId,
        construct: bool,
    ) -> Result<Vec<SignatureId>, Error> {
        let ty = self.reduced_apparent_type(ty)?;
        if self.types.flags(ty)? & tf::STRUCTURED_TYPE == 0 {
            return Ok(Vec::new());
        }
        self.resolve_type_members(ty)?;
        let members = self.types.structured(ty)?;
        let signatures = members.signatures.as_deref().unwrap_or_default();
        let count = members.call_signature_count as usize;
        Ok(if construct {
            &signatures[count..]
        } else {
            &signatures[..count]
        }
        .to_vec())
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getUnionIndexInfos
    pub(crate) fn union_index_infos(
        &mut self,
        types: &[TypeId],
    ) -> Result<Vec<IndexInfoId>, Error> {
        let mut result = Vec::new();
        let Some(&first) = types.first() else {
            return Ok(result);
        };
        for index in self.index_infos_of_type(first)? {
            let key = self.signatures.index_info(index)?.key_type;
            let mut values = Vec::new();
            let mut readonly = false;
            for &ty in types {
                if let Some(index) = self.index_info_of_type(ty, key)? {
                    let info = self.signatures.index_info(index)?;
                    values.push(info.value_type);
                    readonly |= info.is_readonly;
                } else {
                    break;
                }
            }
            if values.len() == types.len() {
                let value = self.get_union_type(&values)?;
                result.push(
                    self.signatures
                        .new_index_info(key, value, readonly, None, None)?,
                );
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.getIndexInfoOfType
    pub(crate) fn index_info_of_type(
        &mut self,
        ty: TypeId,
        key: TypeId,
    ) -> Result<Option<IndexInfoId>, Error> {
        for index in self.index_infos_of_type(ty)? {
            if self.signatures.index_info(index)?.key_type == key {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.appendIndexInfo
    pub(crate) fn append_index_info(
        &mut self,
        indexes: &mut Vec<IndexInfoId>,
        new: IndexInfoId,
        union: bool,
    ) -> Result<(), Error> {
        let new_info = self.signatures.index_info(new)?.clone();
        for index in indexes.iter_mut() {
            let old = self.signatures.index_info(*index)?.clone();
            if old.key_type == new_info.key_type {
                let value = if union {
                    self.get_union_type(&[old.value_type, new_info.value_type])?
                } else {
                    self.get_intersection_type(&[old.value_type, new_info.value_type])?
                };
                let readonly = if union {
                    old.is_readonly || new_info.is_readonly
                } else {
                    old.is_readonly && new_info.is_readonly
                };
                *index =
                    self.signatures
                        .new_index_info(old.key_type, value, readonly, None, None)?;
                return Ok(());
            }
        }
        indexes.push(new);
        Ok(())
    }
}
