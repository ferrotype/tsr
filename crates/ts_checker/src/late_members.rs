//! Late-bound declaration members. The early table is visible during recursive
//! resolution; completion and declaration links are published only on success.

use crate::{type_flags as tf, CheckerState, Error, LinkStore, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    check_flags as cf, internal_symbol_names as names, modifier_flags as mf, node_flags as nf,
    symbol_flags as sf, JsString, SymbolTable, SymbolTableId, SyntaxKind as K,
};

#[derive(Clone, Copy, Default)]
pub(crate) struct ResolvedMemberTable {
    pub table: Option<SymbolTableId>,
    pub started: bool,
}

#[derive(Default)]
pub(crate) struct LateMemberState {
    pub members: LinkStore<SymbolId, [ResolvedMemberTable; 2]>,
    pub declarations: LinkStore<NodeId, Option<SymbolId>>,
    pub symbols: LinkStore<SymbolId, Option<SymbolId>>,
    pub unique_types: crate::types::Map<SymbolId, TypeId>,
}

impl CheckerState {
    pub(crate) fn raw_declaration_symbol(&self, node: NodeId) -> Result<Option<SymbolId>, Error> {
        Ok(self
            .checker_node_binding(node)?
            .and_then(|binding| binding.symbol))
    }

    // port: tsc/internal/checker/checker.go:Checker.getMembersOfSymbol
    pub(crate) fn members_of_symbol(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Option<SymbolTableId>, Error> {
        if self.symbol(symbol)?.flags() & sf::LATE_BINDING_CONTAINER == 0 {
            return Ok(self.symbol(symbol)?.members());
        }
        self.resolved_members_or_exports(symbol, false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getResolvedMembersOrExportsOfSymbol
    pub(crate) fn resolved_members_or_exports(
        &mut self,
        symbol: SymbolId,
        is_static: bool,
    ) -> Result<Option<SymbolTableId>, Error> {
        let index = usize::from(is_static);
        if let Some(states) = self.late_members.members.try_get(symbol) {
            if states[index].started {
                return Ok(states[index].table);
            }
        }
        let read = self.symbol(symbol)?;
        let early = if !is_static {
            read.members()
        } else if read.flags() & sf::MODULE != 0 {
            // Resolve the early exports without publishing the ordinary module
            // cache: this link must still combine the late static declarations.
            Some(self.module_exports_worker(symbol)?)
        } else {
            read.exports()
        };
        let early_index_merge = self
            .member_symbol(early, names::INDEX)?
            .map(|id| (id, self.merged_symbols.get(&id).copied()));
        self.late_members.members.get_or_default(symbol)[index] = ResolvedMemberTable {
            table: early,
            started: true,
        };
        let mut journal = Vec::new();
        let result = (|| {
            let mut late = SymbolTable::new();
            for declaration in self
                .symbol_declarations(symbol)?
                .to_vec()
                .into_iter()
                .flatten()
            {
                let read = self.ast(declaration)?.node(declaration)?;
                let list = match read.kind().known() {
                    Some(
                        K::InterfaceDeclaration
                        | K::ClassDeclaration
                        | K::ClassExpression
                        | K::TypeLiteral,
                    ) => read.member_list(),
                    Some(K::ObjectLiteralExpression) => read.property_list(),
                    _ => None,
                };
                for member in self.source_list(declaration, list)? {
                    if (self
                        .ast(member)?
                        .node(member)?
                        .modifier_flags(self.ast(member)?)?
                        & mf::STATIC
                        != 0)
                        == is_static
                    {
                        let raw = self.raw_declaration_symbol(member)?;
                        journal.push((
                            member,
                            self.late_members
                                .declarations
                                .try_get(member)
                                .copied()
                                .flatten(),
                            raw.map(|raw| {
                                (
                                    raw,
                                    self.late_members.symbols.try_get(raw).copied().flatten(),
                                )
                            }),
                        ));
                        self.resolve_late_member(symbol, early, &mut late, member)?;
                    }
                }
            }
            if is_static {
                if let Some(assignment) =
                    self.member_symbol(early, names::ASSIGNMENT_DECLARATION)?
                {
                    for member in self
                        .symbol_declarations(assignment)?
                        .to_vec()
                        .into_iter()
                        .flatten()
                    {
                        let raw = self.raw_declaration_symbol(member)?;
                        journal.push((
                            member,
                            self.late_members
                                .declarations
                                .try_get(member)
                                .copied()
                                .flatten(),
                            raw.map(|raw| {
                                (
                                    raw,
                                    self.late_members.symbols.try_get(raw).copied().flatten(),
                                )
                            }),
                        ));
                        self.resolve_late_member(symbol, early, &mut late, member)?;
                    }
                }
            }
            if late.is_empty() {
                return Ok(early);
            }
            let late = self.alloc_symbol_table(late);
            let early_empty = match early {
                Some(table) => self.table(table)?.is_empty(),
                None => true,
            };
            if early_empty {
                return Ok(Some(late));
            }
            let combined = self.alloc_symbol_table(SymbolTable::new());
            self.merge_symbol_table(combined, early.expect("nonempty early table"), false, None)?;
            self.merge_symbol_table(combined, late, false, None)?;
            Ok(Some(combined))
        })();
        if result.is_err() {
            if let Some((id, previous)) = early_index_merge {
                match previous {
                    Some(symbol) => {
                        self.merged_symbols.insert(id, symbol);
                    }
                    None => {
                        self.merged_symbols.remove(&id);
                    }
                }
            }
            for (node, previous, raw) in journal.into_iter().rev() {
                *self.late_members.declarations.get_or_default(node) = previous;
                if let Some((raw, previous)) = raw {
                    *self.late_members.symbols.get_or_default(raw) = previous;
                }
            }
        }
        self.late_members.members.get_or_default(symbol)[index] = match &result {
            Ok(table) => ResolvedMemberTable {
                table: *table,
                started: true,
            },
            Err(_) => ResolvedMemberTable::default(),
        };
        result
    }

    // port: tsc/internal/checker/checker.go:isLateBindableAST
    pub(crate) fn late_name(&self, declaration: NodeId) -> Result<Option<NodeId>, Error> {
        let read = self.ast(declaration)?.node(declaration)?;
        let name = if read.kind() == K::BinaryExpression {
            read.data_source()
                .as_binary_expression()
                .and_then(|data| data.left())
        } else {
            read.name()
        };
        let Some(name) = name else { return Ok(None) };
        let read = self.ast(name)?.node(name)?;
        let expression = match read.kind().known() {
            Some(K::ComputedPropertyName) => read.expression(),
            Some(K::ElementAccessExpression) => read
                .data_source()
                .as_element_access_expression()
                .and_then(|data| data.argument_expression()),
            _ => None,
        };
        match expression {
            Some(expression)
                if ts_ast::is_entity_name_expression(self.ast(expression)?, expression)? =>
            {
                Ok(Some(name))
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn late_name_type(&mut self, name: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(name)?.node(name)?;
        if read.kind() == K::ComputedPropertyName {
            return self.check_computed_property_name(name);
        }
        let argument = read
            .data_source()
            .as_element_access_expression()
            .and_then(|data| data.argument_expression())
            .ok_or(Error::MissingLink("late element argument"))?;
        self.check_expression_cached(argument)
    }

    fn resolve_late_member(
        &mut self,
        parent: SymbolId,
        early: Option<SymbolTableId>,
        late: &mut SymbolTable,
        declaration: NodeId,
    ) -> Result<(), Error> {
        let Some(name) = self.late_name(declaration)? else {
            return Ok(());
        };
        let ty = self.late_name_type(name)?;
        if self.types.flags(ty)? & (tf::STRING_OR_NUMBER_LITERAL | tf::UNIQUE_ES_SYMBOL) != 0 {
            self.late_bind_member(parent, early, late, declaration, name, ty)?;
        } else if self.source_type_assignable(
            ty,
            self.builtins.string_number_symbol_type,
            &mut Vec::new(),
        )? {
            self.late_bind_index_signature(early, late, declaration)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.lateBindIndexSignature
    fn late_bind_index_signature(
        &mut self,
        early: Option<SymbolTableId>,
        late: &mut SymbolTable,
        declaration: NodeId,
    ) -> Result<(), Error> {
        let index = if let Some(index) = late.get(names::INDEX).copied().flatten() {
            index
        } else {
            let index = match self.member_symbol(early, names::INDEX)? {
                Some(index) => {
                    let index = self.clone_symbol(index)?;
                    self.symbol_mut(index)?.check_flags |= cf::LATE;
                    index
                }
                None => self.new_symbol_ex(0, JsString::from_bytes(names::INDEX), cf::LATE)?,
            };
            late.insert(JsString::from_bytes(names::INDEX), Some(index));
            index
        };
        let raw = self
            .raw_declaration_symbol(declaration)?
            .ok_or(Error::MissingLink("late index declaration symbol"))?;
        let mut declarations = self.symbol_declarations(index)?.to_vec();
        if declarations.is_empty() || self.symbol(raw)?.flags() & sf::REPLACEABLE_BY_METHOD == 0 {
            declarations.push(Some(declaration));
            let declarations = self.declarations.alloc(declarations)?;
            self.symbol_mut(index)?.declarations = declarations;
        }
        // An index aggregates properties. It must not become the declaration's
        // resolved property symbol or the raw property's late-symbol link.
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.lateBindMember
    fn late_bind_member(
        &mut self,
        parent: SymbolId,
        early: Option<SymbolTableId>,
        late: &mut SymbolTable,
        declaration: NodeId,
        name_node: NodeId,
        ty: TypeId,
    ) -> Result<SymbolId, Error> {
        if let Some(Some(symbol)) = self.late_members.declarations.try_get(declaration) {
            return Ok(*symbol);
        }
        let raw = self
            .raw_declaration_symbol(declaration)?
            .ok_or(Error::MissingLink("late member symbol"))?;
        *self.late_members.declarations.get_or_default(declaration) = Some(raw);
        let result = (|| {
            let name = self
                .index_property_name(ty)?
                .ok_or(Error::MissingLink("late property name"))?;
            let flags = self.symbol(raw)?.flags();
            let mut target = if let Some(symbol) = late.get(name.as_bytes()).copied().flatten() {
                symbol
            } else {
                let symbol = self.new_symbol_ex(0, name.clone(), cf::LATE)?;
                late.insert(name.clone(), Some(symbol));
                symbol
            };
            if self.symbol(target)?.flags() & crate::merge::excluded_symbol_flags(flags) != 0 {
                let mut declarations = Vec::new();
                if let Some(early) = self.member_symbol(early, name.as_bytes())? {
                    declarations.extend(self.symbol_declarations(early)?.iter().flatten());
                }
                declarations.extend(self.symbol_declarations(target)?.iter().flatten());
                let display = if self.types.flags(ty)? & tf::UNIQUE_ES_SYMBOL != 0 {
                    ts_scanner::declaration_name_to_string(self.ast(name_node)?, Some(name_node))?
                } else {
                    name.clone()
                };
                for node in declarations.into_iter().chain(std::iter::once(declaration)) {
                    let name = self.ast(node)?.node(node)?.name().unwrap_or(node);
                    self.error_at(
                        Some(name),
                        ts_diagnostics::Duplicate_identifier_0,
                        vec![display.clone()],
                    )?;
                }
                let old = self.symbol(target)?.flags();
                if old & sf::ACCESSOR != 0 && old & sf::ACCESSOR != flags & sf::ACCESSOR {
                    self.symbol_mut(target)?.flags |= sf::ACCESSOR;
                }
                target = self.new_symbol_ex(0, name, cf::LATE)?;
            }
            self.value_symbol_links.get_or_default(target).name_type = Some(ty);
            self.add_late_declaration(target, raw, declaration)?;
            if self.symbol(target)?.parent().is_none() {
                self.symbol_mut(target)?.parent = Some(parent);
            }
            Ok(target)
        })();
        *self.late_members.declarations.get_or_default(declaration) = result.as_ref().ok().copied();
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.addDeclarationToLateBoundSymbol
    fn add_late_declaration(
        &mut self,
        target: SymbolId,
        raw: SymbolId,
        node: NodeId,
    ) -> Result<(), Error> {
        let flags = self.symbol(raw)?.flags();
        let mut declarations = self.symbol_declarations(target)?.to_vec();
        let previous_flags = self.symbol(target)?.flags();
        if declarations.is_empty() || flags & sf::REPLACEABLE_BY_METHOD == 0 {
            declarations.push(Some(node));
            self.symbol_mut(target)?.flags |= flags;
        } else if previous_flags & sf::REPLACEABLE_BY_METHOD != 0 && flags & sf::METHOD != 0 {
            let mut retained = Vec::new();
            let mut retained_flags = previous_flags & (sf::ACCESSOR | sf::TRANSIENT);
            for declaration in declarations.into_iter().flatten() {
                let symbol = self
                    .raw_declaration_symbol(declaration)?
                    .ok_or(Error::MissingLink("late declaration symbol"))?;
                if self.symbol(symbol)?.flags() & sf::REPLACEABLE_BY_METHOD == 0 {
                    retained.push(Some(declaration));
                    retained_flags |= self.symbol(symbol)?.flags();
                }
            }
            retained.push(Some(node));
            declarations = retained;
            self.symbol_mut(target)?.flags = retained_flags | flags;
        }
        let slice = self.declarations.alloc(declarations)?;
        self.symbol_mut(target)?.declarations = slice;
        if flags & sf::VALUE != 0 {
            self.set_merged_value_declaration(target, node)?;
        }
        *self.late_members.symbols.get_or_default(raw) = Some(target);
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getLateBoundSymbol
    pub(crate) fn late_bound_symbol(&mut self, symbol: SymbolId) -> Result<SymbolId, Error> {
        if self.symbol(symbol)?.flags() & sf::CLASS_MEMBER == 0
            || self.symbol(symbol)?.name_bytes() != names::COMPUTED
        {
            return Ok(symbol);
        }
        if let Some(Some(late)) = self.late_members.symbols.try_get(symbol) {
            return Ok(*late);
        }
        for node in self
            .symbol_declarations(symbol)?
            .to_vec()
            .into_iter()
            .flatten()
        {
            if let Some(name) = self.late_name(node)? {
                let ty = self.late_name_type(name)?;
                if self.index_property_name(ty)?.is_some() {
                    let parent = self
                        .symbol(symbol)?
                        .parent()
                        .ok_or(Error::MissingLink("late member parent"))?;
                    let parent = self.get_merged_symbol(parent);
                    let is_static = self
                        .ast(node)?
                        .node(node)?
                        .modifier_flags(self.ast(node)?)?
                        & mf::STATIC
                        != 0;
                    self.resolved_members_or_exports(parent, is_static)?;
                    break;
                }
            }
        }
        Ok(*self
            .late_members
            .symbols
            .get_or_default(symbol)
            .get_or_insert(symbol))
    }

    // port: tsc/internal/checker/checker.go:isInvalidComputedPropertyName
    pub(crate) fn is_invalid_computed_property_name(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let parent = read
            .parent()
            .ok_or(Error::MissingLink("computed property parent"))?;
        let declaration = self.ast(parent)?.node(parent)?;
        if matches!(
            declaration.kind().known(),
            Some(K::GetAccessor | K::SetAccessor)
        ) {
            return Ok(false);
        }
        let container = declaration
            .parent()
            .ok_or(Error::MissingLink("computed property container"))?;
        if !matches!(
            self.ast(container)?.node(container)?.kind().known(),
            Some(
                K::TypeLiteral | K::ClassDeclaration | K::ClassExpression | K::InterfaceDeclaration
            )
        ) {
            return Ok(false);
        }
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("computed property expression"))?;
        let expression_read = self.ast(expression)?.node(expression)?;
        let Some(binary) = expression_read.data_source().as_binary_expression() else {
            return Ok(false);
        };
        let operator = binary
            .operator_token()
            .ok_or(Error::MissingLink("computed property operator"))?;
        Ok(self.ast(operator)?.node(operator)?.kind() == K::InKeyword)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkComputedPropertyName
    pub(crate) fn check_computed_property_name(&mut self, node: NodeId) -> Result<TypeId, Error> {
        if let Some(Some(ty)) = self.query.type_nodes.try_get(node) {
            return Ok(*ty);
        }
        *self.query.type_nodes.get_or_default(node) = Some(self.builtins.circular_constraint_type);
        let result = (|| {
            let expression = self
                .ast(node)?
                .node(node)?
                .expression()
                .ok_or(Error::MissingLink("computed name expression"))?;
            if self.is_invalid_computed_property_name(node)? {
                return Ok(self.builtins.error_type);
            }
            let ty = self.check_expression(expression)?;
            let assignable_kind = self.type_assignable_to_kind(
                ty,
                tf::STRING_LIKE | tf::NUMBER_LIKE | tf::ES_SYMBOL_LIKE,
            )?;
            if self.types.flags(ty)? & tf::NULLABLE != 0
                || !assignable_kind
                    && !self.source_type_assignable(
                        ty,
                        self.builtins.string_number_symbol_type,
                        &mut Vec::new(),
                    )?
            {
                self.error_at(Some(node), ts_diagnostics::A_computed_property_name_must_be_of_type_string_number_symbol_or_any, Vec::new())?;
            }
            Ok(ty)
        })();
        *self.query.type_nodes.get_or_default(node) = result.as_ref().ok().copied();
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.getESSymbolLikeTypeForNode
    pub(crate) fn es_symbol_like_type_for_node(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let modifiers = read.modifier_flags(self.ast(node)?)?;
        let valid = match read.kind().known() {
            Some(K::PropertySignature) => modifiers & mf::READONLY != 0,
            Some(K::PropertyDeclaration) => {
                modifiers & (mf::READONLY | mf::STATIC) == mf::READONLY | mf::STATIC
            }
            Some(K::VariableDeclaration) => {
                let name_identifier = read
                    .name()
                    .map(|name| {
                        self.ast(name)?
                            .node(name)
                            .map(|name| name.kind() == K::Identifier)
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false);
                let parent = read
                    .parent()
                    .ok_or(Error::MissingLink("unique symbol variable parent"))?;
                let parent = self.ast(parent)?.node(parent)?;
                name_identifier
                    && parent.kind() == K::VariableDeclarationList
                    && parent.flags() & nf::CONSTANT != 0
                    && parent
                        .parent()
                        .map(|node| {
                            self.ast(node)?
                                .node(node)
                                .map(|read| read.kind() == K::VariableStatement)
                                .map_err(Error::from)
                        })
                        .transpose()?
                        .unwrap_or(false)
            }
            _ => false,
        };
        let symbol = if valid {
            self.raw_declaration_symbol(node)?
        } else {
            None
        };
        let Some(symbol) = symbol else {
            return Ok(self.builtins.es_symbol_type);
        };
        if let Some(&ty) = self.late_members.unique_types.get(&symbol) {
            return Ok(ty);
        }
        let name = JsString::from_bytes(
            [
                b"\xfe@".as_slice(),
                self.symbol(symbol)?.name_bytes(),
                b"@",
                self.symbol_runtime_id(symbol)?.to_string().as_bytes(),
            ]
            .concat(),
        );
        let ty = self.types.new_type(
            tf::UNIQUE_ES_SYMBOL,
            0,
            crate::types::Payload::UniqueEsSymbol(crate::types::UniqueEsSymbolData { name }),
        )?;
        self.types.get_mut(ty)?.symbol = Some(symbol);
        self.late_members.unique_types.insert(symbol, ty);
        Ok(ty)
    }
}
