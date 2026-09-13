//! Explicit and late-bound index signatures share the native declaration order.
//! Computed members aggregate their siblings; an explicit index takes priority.
use crate::{type_flags as tf, CheckerState, Error, IndexInfoId, TypeId, UnionReduction};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, SymbolTableId, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getIndexInfosOfIndexSymbol
    pub(crate) fn index_infos_of_symbol(
        &mut self,
        symbol: Option<SymbolId>,
        siblings: Option<SymbolTableId>,
    ) -> Result<Vec<IndexInfoId>, Error> {
        let Some(symbol) = symbol else {
            return Ok(Vec::new());
        };
        let mut indexes = Vec::new();
        let mut computed = [false; 3];
        let mut readonly = [true; 3];
        let keys = [
            self.builtins.string_type,
            self.builtins.number_type,
            self.builtins.es_symbol_type,
        ];
        let mut properties = Vec::new();
        for node in self
            .symbol_declarations(symbol)?
            .to_vec()
            .into_iter()
            .flatten()
        {
            let read = self.ast(node)?.node(node)?;
            if read.kind() == K::IndexSignature {
                let parameters = self.source_list(node, read.parameter_list())?;
                let value_node = read.type_node();
                let readonly = read.modifier_flags(self.ast(node)?)? & mf::READONLY != 0;
                if parameters.len() != 1 {
                    continue;
                }
                let Some(key_node) = self.ast(parameters[0])?.node(parameters[0])?.type_node()
                else {
                    continue;
                };
                let value = match value_node {
                    Some(node) => self.get_type_from_type_node(node)?,
                    None => self.builtins.any_type,
                };
                let key = self.get_type_from_type_node(key_node)?;
                let parts = if self.types.flags(key)? & tf::UNION != 0 {
                    self.types.union(key)?.types.to_vec()
                } else {
                    vec![key]
                };
                for key in parts {
                    if self.valid_index_key_type(key)? && !self.has_index_key(&indexes, key)? {
                        indexes.push(self.signatures.new_index_info(
                            key,
                            value,
                            readonly,
                            Some(node),
                            None,
                        )?);
                    }
                }
            } else if let Some(name) = self.late_name(node)? {
                let key = self.late_name_type(name)?;
                if self.has_index_key(&indexes, key)? {
                    continue;
                }
                if self.source_type_assignable(
                    key,
                    self.builtins.string_number_symbol_type,
                    &mut Vec::new(),
                )? {
                    let index = if self.source_type_assignable(key, keys[1], &mut Vec::new())? {
                        1
                    } else if self.source_type_assignable(key, keys[2], &mut Vec::new())? {
                        2
                    } else {
                        0
                    };
                    computed[index] = true;
                    readonly[index] &= self
                        .ast(node)?
                        .node(node)?
                        .modifier_flags(self.ast(node)?)?
                        & mf::READONLY
                        != 0;
                    properties.push(
                        self.raw_declaration_symbol(node)?
                            .ok_or(Error::MissingLink("computed index property"))?,
                    );
                }
            }
        }
        if computed.into_iter().any(|value| value) {
            if let Some(siblings) = siblings {
                properties.extend(
                    self.table(siblings)?
                        .into_iter()
                        .filter_map(|(_, value)| value)
                        .filter(|&value| value != symbol),
                );
            }
            for index in 0..3 {
                if computed[index] && !self.has_index_key(&indexes, keys[index])? {
                    indexes.push(self.object_literal_index_info(
                        readonly[index],
                        &properties,
                        keys[index],
                    )?);
                }
            }
        }
        Ok(indexes)
    }

    fn has_index_key(&self, indexes: &[IndexInfoId], key: TypeId) -> Result<bool, Error> {
        for &index in indexes {
            if self.signatures.index_info(index)?.key_type == key {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getObjectLiteralIndexInfo
    pub(crate) fn object_literal_index_info(
        &mut self,
        readonly: bool,
        properties: &[SymbolId],
        key: TypeId,
    ) -> Result<IndexInfoId, Error> {
        let mut types = Vec::new();
        let mut components = Vec::new();
        for &property in properties {
            let include = if key == self.builtins.string_type {
                !self.symbol_with_symbol_name(property)?
            } else if key == self.builtins.number_type {
                self.symbol_with_numeric_name(property)?
            } else {
                self.symbol_with_symbol_name(property)?
            };
            if include {
                types.push(self.get_type_of_symbol(property)?);
                if let Some((declaration, name)) = self.first_declaration_name(property)? {
                    if self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName {
                        components.push(declaration);
                    }
                }
            }
        }
        let value = if types.is_empty() {
            self.builtins.undefined_type
        } else {
            self.get_union_type_ex(&types, UnionReduction::Subtype, None, None)?
        };
        self.signatures.new_index_info(
            key,
            value,
            readonly,
            None,
            (!components.is_empty()).then(|| components.into()),
        )
    }

    fn first_declaration_name(&self, symbol: SymbolId) -> Result<Option<(NodeId, NodeId)>, Error> {
        let Some(declaration) = self.symbol_declarations(symbol)?.first().flatten() else {
            return Ok(None);
        };
        Ok(self
            .ast(declaration)?
            .node(declaration)?
            .name()
            .map(|name| (declaration, name)))
    }

    // port: tsc/internal/checker/checker.go:Checker.isSymbolWithSymbolName
    fn symbol_with_symbol_name(&mut self, symbol: SymbolId) -> Result<bool, Error> {
        if self.symbol(symbol)?.name_bytes().starts_with(b"\xfe@") {
            return Ok(true);
        }
        if let Some((_, name)) = self.first_declaration_name(symbol)? {
            if self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName {
                let ty = self.check_computed_property_name(name)?;
                return self.type_assignable_to_kind(ty, tf::ES_SYMBOL);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isSymbolWithNumericName
    // port: tsc/internal/checker/checker.go:Checker.isNumericName
    fn symbol_with_numeric_name(&mut self, symbol: SymbolId) -> Result<bool, Error> {
        if crate::indexes::numeric_name(self.symbol(symbol)?.name_bytes()).is_some() {
            return Ok(true);
        }
        if let Some((_, name)) = self.first_declaration_name(symbol)? {
            match self.ast(name)?.node(name)?.kind().known() {
                Some(K::ComputedPropertyName) => {
                    let ty = self.check_computed_property_name(name)?;
                    return self.type_assignable_to_kind(ty, tf::NUMBER_LIKE);
                }
                Some(K::Identifier | K::NumericLiteral | K::StringLiteral) => {
                    return Ok(crate::indexes::numeric_name(
                        self.ast(name)?.node_text(name)?.as_bytes(),
                    )
                    .is_some())
                }
                _ => {}
            }
        }
        Ok(false)
    }
}
