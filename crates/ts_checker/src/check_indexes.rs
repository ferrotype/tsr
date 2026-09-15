//! Declaration index constraints. Late-bound index declarations may overlap;
//! explicit duplicates are diagnosed, and inherited constraints retain their
//! source location instead of assigning every error to the derived interface.
use crate::{
    object_flags as of, type_flags as tf, type_format_flags as ff, CheckerState, Error,
    IndexInfoId, TypeId,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{internal_symbol_names as names, symbol_flags as sf, SyntaxKind as K};

impl CheckerState {
    pub(crate) fn check_source_index_constraints(
        &mut self,
        ty: TypeId,
        declaration: NodeId,
        is_static_index: bool,
    ) -> Result<(), Error> {
        if self.query.index_constraints_checked.contains(&ty) {
            return Ok(());
        }
        self.check_index_constraints(ty, is_static_index)?;
        self.check_class_or_interface_duplicate_indexes(declaration)?;
        self.query.index_constraints_checked.insert(ty);
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkIndexConstraints
    pub(crate) fn check_index_constraints(
        &mut self,
        ty: TypeId,
        is_static_index: bool,
    ) -> Result<(), Error> {
        let indexes = self.index_infos_of_type(ty)?;
        if indexes.is_empty() {
            return Ok(());
        }
        for property in self.get_properties_of_type(ty)? {
            if is_static_index && self.symbol(property)?.flags() & sf::PROTOTYPE != 0 {
                continue;
            }
            let key =
                self.literal_type_from_property(property, tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)?;
            let value = self.non_missing_symbol_type(property)?;
            self.check_index_property(ty, property, key, value, &indexes)?;
        }
        let declaration = match self.types.get(ty)?.symbol {
            Some(symbol) => self.symbol(symbol)?.value_declaration(),
            None => None,
        };
        if let Some(declaration) = declaration {
            if ts_ast::utilities::is_class_like(&self.ast(declaration)?.node(declaration)?) {
                let members = self.source_list(
                    declaration,
                    self.ast(declaration)?.node(declaration)?.member_list(),
                )?;
                for member in members {
                    // Only process instance properties against instance index signatures
                    // and static properties against static index signatures.
                    if ts_ast::utilities::is_static(self.ast(member)?, member)? != is_static_index
                        || self.has_bindable_name(member)?
                    {
                        continue;
                    }
                    let Some(symbol) = self.get_symbol_of_declaration(member)? else {
                        continue;
                    };
                    let name = self
                        .ast(member)?
                        .node(member)?
                        .name()
                        .ok_or(Error::MissingLink("computed member name"))?;
                    let expression = self
                        .ast(name)?
                        .node(name)?
                        .expression()
                        .ok_or(Error::MissingLink("computed member expression"))?;
                    let key = self.get_type_of_expression(expression)?;
                    let value = self.non_missing_symbol_type(symbol)?;
                    self.check_index_property(ty, symbol, key, value, &indexes)?;
                }
            }
        }
        if indexes.len() > 1 {
            for &source in &indexes {
                let check = self.signatures.index_info(source)?.clone();
                for &target in &indexes {
                    if source == target {
                        continue;
                    }
                    let info = self.signatures.index_info(target)?.clone();
                    if !self.applicable_index_type(check.key_type, info.key_type)? {
                        continue;
                    }
                    let local = self.local_index_declaration(ty, check.declaration)?;
                    let other = self.local_index_declaration(ty, info.declaration)?;
                    let error = match local.or(other) {
                        Some(node) => Some(node),
                        None => self.inherited_index_error_node(
                            ty,
                            None,
                            check.key_type,
                            info.key_type,
                        )?,
                    };
                    if let Some(error) = error {
                        if !self.source_type_assignable(
                            check.value_type,
                            info.value_type,
                            &mut Vec::new(),
                        )? {
                            let args = self.index_error_type_names(&[
                                check.key_type,
                                check.value_type,
                                info.key_type,
                                info.value_type,
                            ])?;
                            self.error_at(Some(error), ts_diagnostics::X_0_index_type_1_is_not_assignable_to_2_index_type_3, args)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkIndexConstraintForProperty
    fn check_index_property(
        &mut self,
        ty: TypeId,
        property: SymbolId,
        key: TypeId,
        value: TypeId,
        indexes: &[IndexInfoId],
    ) -> Result<(), Error> {
        let declaration = self.symbol(property)?.value_declaration();
        let name = declaration
            .map(|node| {
                self.ast(node)?
                    .node(node)
                    .map(|read| read.name())
                    .map_err(Error::from)
            })
            .transpose()?
            .flatten();
        if let Some(name) = name {
            if self.ast(name)?.node(name)?.kind() == K::PrivateIdentifier {
                return Ok(());
            }
        }
        let computed = match (declaration, name) {
            (Some(declaration), _)
                if self.ast(declaration)?.node(declaration)?.kind() == K::BinaryExpression =>
            {
                Some(declaration)
            }
            (Some(declaration), Some(name))
                if self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName =>
            {
                Some(declaration)
            }
            _ => None,
        };
        let local = if self
            .symbol(property)?
            .parent()
            .map(|parent| self.get_merged_symbol(parent))
            == self.types.get(ty)?.symbol
        {
            declaration
        } else {
            None
        };
        for &index in indexes {
            let info = self.signatures.index_info(index)?.clone();
            if !self.applicable_index_type(key, info.key_type)? {
                continue;
            }
            let other = self.local_index_declaration(ty, info.declaration)?;
            let error = match local.or(other) {
                Some(node) => Some(node),
                None => self.inherited_index_error_node(ty, Some(property), key, info.key_type)?,
            };
            if let Some(error) = error {
                if !self.source_type_assignable(value, info.value_type, &mut Vec::new())? {
                    let name = self.symbol_to_string(property)?;
                    let mut args = vec![name.clone()];
                    args.extend(self.index_error_type_names(&[
                        value,
                        info.key_type,
                        info.value_type,
                    ])?);
                    let mut diagnostic = self.diagnostic_for_node(
                        Some(error),
                        ts_diagnostics::Property_0_of_type_1_is_not_assignable_to_2_index_type_3,
                        args,
                    )?;
                    if let Some(computed) = computed.filter(|&node| node != error) {
                        diagnostic.related_information.push(std::sync::Arc::new(
                            self.diagnostic_for_node(
                                Some(computed),
                                ts_diagnostics::X_0_is_declared_here,
                                vec![name],
                            )?,
                        ));
                    }
                    self.add_diagnostic(diagnostic)?;
                }
            }
        }
        Ok(())
    }

    fn local_index_declaration(
        &mut self,
        ty: TypeId,
        node: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        let Some(node) = node else { return Ok(None) };
        let symbol = self
            .get_symbol_of_declaration(node)?
            .ok_or(Error::MissingLink("index declaration symbol"))?;
        Ok((self
            .symbol(symbol)?
            .parent()
            .map(|parent| self.get_merged_symbol(parent))
            == self.types.get(ty)?.symbol)
            .then_some(node))
    }

    fn inherited_index_error_node(
        &mut self,
        ty: TypeId,
        property: Option<SymbolId>,
        key: TypeId,
        other_key: TypeId,
    ) -> Result<Option<NodeId>, Error> {
        if self.types.object_flags(ty)? & of::INTERFACE == 0 {
            return Ok(None);
        }
        for base in self.interface_base_types(ty)?.to_vec() {
            let first = match property {
                Some(property) => {
                    let name = self.symbol(property)?.name_to_owned();
                    self.resolve_type_members(base)?;
                    self.member_symbol(self.types.structured(base)?.members, name.as_bytes())?
                        .is_some()
                }
                None => self.index_info_of_type(base, key)?.is_some(),
            };
            if first && self.index_info_of_type(base, other_key)?.is_some() {
                return Ok(None);
            }
        }
        let symbol = self
            .types
            .get(ty)?
            .symbol
            .ok_or(Error::MissingLink("interface symbol"))?;
        for declaration in self.symbol_declarations(symbol)?.iter().flatten() {
            if self.ast(declaration)?.node(declaration)?.kind() == K::InterfaceDeclaration {
                return Ok(Some(declaration));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkClassOrInterfaceForDuplicateIndexSignatures
    pub(crate) fn check_class_or_interface_duplicate_indexes(
        &mut self,
        node: NodeId,
    ) -> Result<(), Error> {
        let symbol = self
            .get_symbol_of_declaration(node)?
            .ok_or(Error::MissingLink("index owner symbol"))?;
        if !self.query.index_signatures_checked.insert(symbol) {
            return Ok(());
        }
        let result = self.check_duplicate_index_signatures(node);
        if result.is_err() {
            self.query.index_signatures_checked.remove(&symbol);
        }
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypeForDuplicateIndexSignatures
    fn check_duplicate_index_signatures(&mut self, node: NodeId) -> Result<(), Error> {
        let symbol = self
            .get_symbol_of_declaration(node)?
            .ok_or(Error::MissingLink("index owner symbol"))?;
        let members = self.members_of_symbol(symbol)?;
        let Some(index) = self.member_symbol(members, names::INDEX)? else {
            return Ok(());
        };
        // Native diagnostics are sorted before exposure; retain first-seen key
        // order here rather than depending on either runtime's map iteration.
        let mut groups: Vec<(TypeId, Vec<NodeId>)> = Vec::new();
        for declaration in self
            .symbol_declarations(index)?
            .to_vec()
            .into_iter()
            .flatten()
        {
            let read = self.ast(declaration)?.node(declaration)?;
            if read.kind() != K::IndexSignature {
                continue;
            }
            let parameters = self.source_list(declaration, read.parameter_list())?;
            if parameters.len() != 1 {
                continue;
            }
            let Some(annotation) = self.ast(parameters[0])?.node(parameters[0])?.type_node() else {
                continue;
            };
            let ty = self.get_type_from_type_node(annotation)?;
            let parts = if self.types.flags(ty)? & tf::UNION != 0 {
                self.types.union(ty)?.types.to_vec()
            } else {
                vec![ty]
            };
            for ty in parts {
                if let Some((_, nodes)) = groups.iter_mut().find(|(key, _)| *key == ty) {
                    nodes.push(declaration);
                } else {
                    groups.push((ty, vec![declaration]));
                }
            }
        }
        for (ty, nodes) in groups {
            if nodes.len() > 1 {
                for node in nodes {
                    let args = self.index_error_type_names(&[ty])?;
                    self.error_at(
                        Some(node),
                        ts_diagnostics::Duplicate_index_signature_for_type_0,
                        args,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn index_error_type_names(&mut self, types: &[TypeId]) -> Result<Vec<ts_ast::JsString>, Error> {
        types
            .iter()
            .map(|&ty| {
                self.type_to_string(
                    ty,
                    ff::ALLOW_UNIQUE_ES_SYMBOL_TYPE | ff::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
                )
            })
            .collect()
    }

    // port: tsc/internal/checker/checker.go:Checker.hasBindableName
    pub(crate) fn has_bindable_name(&mut self, node: NodeId) -> Result<bool, Error> {
        if !ts_ast::has_dynamic_name(self.ast(node)?, Some(node))? {
            return Ok(true);
        }
        match self.late_name(node)? {
            Some(name) => {
                let ty = self.late_name_type(name)?;
                Ok(
                    self.types.flags(ty)? & (tf::STRING_OR_NUMBER_LITERAL | tf::UNIQUE_ES_SYMBOL)
                        != 0,
                )
            }
            None => Ok(false),
        }
    }
}
