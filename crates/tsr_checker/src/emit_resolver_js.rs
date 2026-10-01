//! The JavaScript-emit half of the emit resolver (`printer.EmitResolver` in
//! `tsc/internal/checker/emitresolver.go`): the alias elision, reference and
//! constant queries the script transforms ask, and the JSX factory entities.
//! Each entry point is a method of the checker operation; the declarations
//! transformer's half is the `DeclarationEmitResolver` implementation.

use crate::emit_reference::{ReferenceAnswer, ReferenceQuery};
use crate::linked_references::{is_const_enum_or_const_enum_only_module, ReferenceHint};
use crate::{type_flags as tf, CheckerState, Error, Operation};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{modifier_flags as mf, symbol_flags as sf, SyntaxKind as K};
use tsr_printer::emit_resolver::{ConstantValue, DeclarationEmitResolver, EnumMemberValue};
pub use tsr_printer::script_resolver::TypeReferenceSerializationKind;
use tsr_printer::script_resolver::{
    EmitResolver, EmitResolverError, ReferenceResolver, ResolverResult,
};

fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}

impl Operation<'_> {
    // port: tsc/internal/checker/emitresolver.go:EmitResolver.GetJsxFactoryEntity
    /// The factory entity at `location`, built in the checker's factory.
    pub fn jsx_factory_entity(&mut self, location: NodeId) -> Result<Option<NodeId>, Error> {
        self.state_mut().jsx_factory_entity(Some(location))
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.GetJsxFragmentFactoryEntity
    pub fn jsx_fragment_factory_entity(
        &mut self,
        location: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        self.state_mut().jsx_fragment_factory_entity(Some(location))
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.IsReferencedAliasDeclaration
    pub fn is_referenced_alias_declaration(&mut self, node: NodeId) -> Result<bool, Error> {
        let state = self.state_mut();
        if !state.can_collect_alias_data()? || !state.emit_parse_node(node)? {
            return Ok(true);
        }
        let view = state.ast(node)?;
        if !tsr_ast::is_alias_symbol_declaration(view, node)? {
            return Ok(false);
        }
        let Some(symbol) = state.get_symbol_of_declaration(node)? else {
            return Ok(false);
        };
        if state.module_aliases.referenced.contains(&symbol) {
            return Ok(true);
        }
        let Some(target) = state
            .module_aliases
            .targets
            .get(&symbol)
            .and_then(|target| target.as_ref().ok().copied())
        else {
            return Ok(false);
        };
        let exported = state.node(node)?.modifier_flags(state.ast(node)?)? & mf::EXPORT != 0;
        if exported && state.module_symbol_flags(target, false, false)? & sf::VALUE != 0 {
            let preserve = state
                .program()?
                .host
                .options()
                .should_preserve_const_enums();
            if preserve || !is_const_enum_or_const_enum_only_module(state.symbol(target)?.flags()) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.IsValueAliasDeclaration
    pub fn is_value_alias_declaration(&mut self, node: NodeId) -> Result<bool, Error> {
        let state = self.state_mut();
        if !state.can_collect_alias_data()? || !state.emit_parse_node(node)? {
            return Ok(true);
        }
        state.emit_value_alias_declaration(node)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.IsTopLevelValueImportEqualsWithEntityName
    pub fn is_top_level_value_import_equals_with_entity_name(
        &mut self,
        node: NodeId,
    ) -> Result<bool, Error> {
        let state = self.state_mut();
        if !state.can_collect_alias_data()? {
            return Ok(true);
        }
        if !state.emit_parse_node(node)? {
            return Ok(false);
        }
        let read = state.node(node)?;
        if read.kind() != K::ImportEqualsDeclaration {
            return Ok(false);
        }
        let parent = required(read.parent(), "import equals parent")?;
        if state.node(parent)?.kind() != K::SourceFile {
            return Ok(false);
        }
        let reference = read
            .data_source()
            .as_import_equals_declaration()
            .and_then(|data| data.module_reference());
        match reference {
            None => return Ok(false),
            Some(reference) => {
                let reference = state.node(reference)?;
                if tsr_ast::node_is_missing(Some(&reference))
                    || reference.kind() == K::ExternalModuleReference
                {
                    return Ok(false);
                }
            }
        }
        let symbol = state.get_symbol_of_declaration(node)?;
        state.emit_alias_resolved_to_value(symbol, false)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.MarkLinkedReferencesRecursively
    /// Marks the aliases every emitted reference in `file` retains, as import
    /// elision does before it asks which imports are referenced.
    pub fn mark_linked_references_recursively(&mut self, file: NodeId) -> Result<(), Error> {
        let state = self.state_mut();
        if !state.emit_parse_node(file)? {
            return Ok(());
        }
        let mut stack = state.source_children(file)?;
        stack.reverse();
        while let Some(node) = stack.pop() {
            let read = state.node(node)?;
            let kind = read.kind();
            if kind == K::ImportEqualsDeclaration
                && read.modifier_flags(state.ast(node)?)? & mf::EXPORT == 0
            {
                continue; // These are deferred and marked in a chain when referenced
            }
            if kind == K::ImportDeclaration {
                continue; // likewise, these are ultimately what get marked by calls on other nodes - we want to skip them
            }
            state.mark_linked_references(node, ReferenceHint::Unspecified, None, None)?;
            let mut children = state.source_children(node)?;
            children.reverse();
            stack.extend(children);
        }
        Ok(())
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.GetConstantValue
    // port: tsc/internal/checker/services.go:Checker.GetConstantValue
    /// The inlined value of a constant enum member reference, or of an enum member.
    pub fn constant_value(&mut self, node: NodeId) -> Result<Option<ConstantValue>, Error> {
        let state = self.state_mut();
        if state.node(node)?.kind() == K::EnumMember {
            return state.enum_member_constant(node);
        }
        if state
            .query
            .resolved_symbols
            .try_get(node)
            .copied()
            .flatten()
            .is_none()
        {
            state.check_expression_cached(node)?; // ensure cached resolved symbol is set
        }
        let mut symbol = state
            .query
            .resolved_symbols
            .try_get(node)
            .copied()
            .flatten();
        if symbol.is_none() && tsr_ast::is_entity_name_expression(state.ast(node)?, node)? {
            symbol = state.resolve_entity_name(node, sf::VALUE, true)?;
        }
        if let Some(symbol) = symbol {
            if state.symbol(symbol)?.flags() & sf::ENUM_MEMBER != 0 {
                // inline property\index accesses only for const enums
                let member = required(
                    state.symbol(symbol)?.value_declaration(),
                    "enum member declaration",
                )?;
                let declaration = required(state.node(member)?.parent(), "enum member parent")?;
                let view = state.ast(declaration)?;
                if tsr_ast::utilities::get_combined_modifier_flags(view, declaration)? & mf::CONST
                    != 0
                {
                    return state.enum_member_constant(member);
                }
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.GetReferencedExportContainer
    pub fn referenced_export_container(
        &mut self,
        node: NodeId,
        prefix_locals: bool,
    ) -> Result<Option<NodeId>, Error> {
        let state = self.state_mut();
        if !state.emit_parse_node(node)? {
            return Ok(None);
        }
        match state.emit_reference_query(node, ReferenceQuery::ExportContainer { prefix_locals })? {
            ReferenceAnswer::Node(node) => Ok(node),
            ReferenceAnswer::Nodes(_) => Err(Error::MissingLink("export container answer")),
        }
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.SetReferencedImportDeclaration
    pub fn set_referenced_import_declaration(
        &mut self,
        node: NodeId,
        declaration: NodeId,
    ) -> Result<(), Error> {
        self.state_mut().emit.import_refs.insert(node, declaration);
        Ok(())
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.GetReferencedImportDeclaration
    pub fn referenced_import_declaration(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let state = self.state_mut();
        if !state.emit_parse_node(node)? {
            return Ok(state.emit.import_refs.get(&node).copied());
        }
        let Some(symbol) = state.referenced_value_or_alias_symbol(node)? else {
            return Ok(None);
        };
        if tsr_ast::is_non_local_alias(Some(&state.symbol(symbol)?), sf::VALUE)
            && state.module_type_only_alias(symbol, sf::VALUE)?.is_none()
        {
            return state.alias_declaration_or_none(symbol);
        }
        Ok(None)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.GetReferencedValueDeclaration
    pub fn referenced_value_declaration(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let state = self.state_mut();
        if !state.emit_parse_node(node)? {
            return Ok(None);
        }
        state.emit_referenced_value_declaration(node)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.GetReferencedValueDeclarations
    pub fn referenced_value_declarations(
        &mut self,
        node: NodeId,
    ) -> Result<Option<Vec<NodeId>>, Error> {
        let state = self.state_mut();
        if !state.emit_parse_node(node)? {
            return Ok(None);
        }
        match state.emit_reference_query(node, ReferenceQuery::ValueDeclarations)? {
            ReferenceAnswer::Nodes(nodes) => Ok(nodes),
            ReferenceAnswer::Node(_) => Err(Error::MissingLink("value declarations answer")),
        }
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.GetTypeReferenceSerializationKind
    pub fn type_reference_serialization_kind(
        &mut self,
        type_name: Option<NodeId>,
        location: Option<NodeId>,
    ) -> Result<TypeReferenceSerializationKind, Error> {
        use TypeReferenceSerializationKind as Kind;
        let (Some(type_name), Some(location)) = (type_name, location) else {
            return Ok(Kind::Unknown);
        };
        let state = self.state_mut();
        // Resolve the symbol as a value to ensure the type can be reached at runtime during emit.
        let mut type_only = false;
        if state.node(type_name)?.kind() == K::QualifiedName {
            let first =
                tsr_ast::utilities_middle::get_first_identifier(state.ast(type_name)?, type_name)?;
            if let Some(root) =
                state.resolve_entity_name_at(first, sf::VALUE, true, true, Some(location))?
            {
                let declarations: Vec<NodeId> =
                    state.symbol_declarations(root)?.iter().flatten().collect();
                if !declarations.is_empty() {
                    let mut every = true;
                    for declaration in declarations {
                        let view = state.ast(declaration)?;
                        if !tsr_ast::utilities_modules::is_type_only_import_or_export_declaration(
                            view,
                            declaration,
                        )? {
                            every = false;
                            break;
                        }
                    }
                    type_only = every;
                }
            }
        }
        let value =
            state.resolve_entity_name_at(type_name, sf::VALUE, true, true, Some(location))?;
        let resolved_value = match value {
            Some(symbol) if state.symbol(symbol)?.flags() & sf::ALIAS != 0 => {
                Some(state.resolve_alias(symbol)?)
            }
            other => other,
        };
        if let Some(value) = value {
            type_only = type_only || state.module_type_only_alias(value, sf::VALUE)?.is_some();
        }
        // Resolve the symbol as a type so that we can provide a more useful hint for the type serializer.
        let type_symbol =
            state.resolve_entity_name_at(type_name, sf::TYPE, true, true, Some(location))?;
        let resolved_type = match type_symbol {
            Some(symbol) if state.symbol(symbol)?.flags() & sf::ALIAS != 0 => {
                Some(state.resolve_alias(symbol)?)
            }
            other => other,
        };
        // In case the value symbol can't be resolved (e.g. because of missing declarations), use type symbol for reachability check.
        if let Some(type_symbol) = type_symbol {
            type_only = type_only
                || state
                    .module_type_only_alias(type_symbol, sf::TYPE)?
                    .is_some();
        }
        if let Some(resolved) = resolved_value.filter(|&value| Some(value) == resolved_type) {
            if state.global_promise_constructor_symbol(true)? == Some(resolved) {
                return Ok(Kind::Promise);
            }
            let constructor = state.get_type_of_symbol(resolved)?;
            if state.is_constructor_type(constructor)? {
                return Ok(if type_only {
                    Kind::TypeWithCallSignature
                } else {
                    Kind::TypeWithConstructSignatureAndValue
                });
            }
        }
        // We might not be able to resolve type symbol so use unknown type in that case (eg error case)
        let Some(resolved_type) = resolved_type else {
            return Ok(if type_only {
                Kind::ObjectType
            } else {
                Kind::Unknown
            });
        };
        let ty = state.get_declared_type_of_symbol(resolved_type)?;
        if state.is_error_type(ty)? {
            return Ok(if type_only {
                Kind::ObjectType
            } else {
                Kind::Unknown
            });
        }
        let flags = state.types.flags(ty)?;
        Ok(if flags & tf::ANY_OR_UNKNOWN != 0 {
            Kind::ObjectType
        } else if state.type_assignable_to_kind(ty, tf::VOID | tf::NULLABLE | tf::NEVER)? {
            Kind::VoidNullableOrNeverType
        } else if state.type_assignable_to_kind(ty, tf::BOOLEAN_LIKE)? {
            Kind::BooleanType
        } else if state.type_assignable_to_kind(ty, tf::NUMBER_LIKE)? {
            Kind::NumberLikeType
        } else if state.type_assignable_to_kind(ty, tf::BIG_INT_LIKE)? {
            Kind::BigIntLikeType
        } else if state.type_assignable_to_kind(ty, tf::STRING_LIKE)? {
            Kind::StringLikeType
        } else if state.is_tuple_type(ty)? {
            Kind::ArrayLikeType
        } else if state.type_assignable_to_kind(ty, tf::ES_SYMBOL_LIKE)? {
            Kind::EsSymbolType
        } else if flags & tf::OBJECT != 0 && !state.signatures_of_type(ty, false)?.is_empty() {
            // isFunctionType
            Kind::TypeWithCallSignature
        } else if state.is_array_type(ty)? {
            Kind::ArrayLikeType
        } else {
            Kind::ObjectType
        })
    }
}

impl CheckerState {
    /// `canCollectSymbolAliasAccessibilityData`.
    fn can_collect_alias_data(&self) -> Result<bool, Error> {
        Ok(!self
            .program()?
            .host
            .options()
            .verbatim_module_syntax
            .is_true())
    }

    /// `getEnumMemberValue(node).Value` for an enum member.
    pub(crate) fn enum_member_constant(
        &mut self,
        member: NodeId,
    ) -> Result<Option<ConstantValue>, Error> {
        let parent = required(self.node(member)?.parent(), "enum member parent")?;
        self.compute_enum_member_values(parent)?;
        Ok(self.enums.values.try_get(member).and_then(|value| {
            value.value.as_ref().map(|value| match value {
                crate::enums::EnumValue::Number(number) => ConstantValue::Number(*number),
                crate::enums::EnumValue::String(text) => ConstantValue::String(text.clone()),
            })
        }))
    }

    // port: tsc/internal/checker/checker.go:Checker.getReferencedValueOrAliasSymbol
    fn referenced_value_or_alias_symbol(
        &mut self,
        reference: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        if let Some(symbol) = self
            .query
            .resolved_symbols
            .try_get(reference)
            .copied()
            .flatten()
        {
            if symbol != self.builtins.unknown_symbol {
                return Ok(Some(symbol));
            }
        }
        let name = self.node_text(reference)?.into_js_string();
        self.resolve_name_ex(
            Some(reference),
            name.as_bytes(),
            sf::VALUE | sf::EXPORT_VALUE | sf::ALIAS,
            None,
            false,
            false,
        )
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.isValueAliasDeclarationWorker
    fn emit_value_alias_declaration(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.node(node)?;
        match read.kind().known() {
            Some(K::ImportEqualsDeclaration) => {
                let symbol = self.get_symbol_of_declaration(node)?;
                self.emit_alias_resolved_to_value(symbol, false)
            }
            Some(
                K::ImportClause | K::NamespaceImport | K::ImportSpecifier | K::ExportSpecifier,
            ) => {
                let symbol = self.get_symbol_of_declaration(node)?;
                Ok(symbol.is_some() && self.emit_alias_resolved_to_value(symbol, true)?)
            }
            Some(K::ExportDeclaration) => {
                let clause = read
                    .data_source()
                    .as_export_declaration()
                    .and_then(|data| data.export_clause());
                let Some(clause) = clause else {
                    return Ok(false);
                };
                if self.node(clause)?.kind() == K::NamespaceExport {
                    return Ok(true);
                }
                for element in self.source_list(clause, self.node(clause)?.element_list())? {
                    if self.emit_value_alias_declaration(element)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Some(K::ExportAssignment) => {
                if let Some(expression) = read.expression() {
                    if self.node(expression)?.kind() == K::Identifier {
                        let symbol = self.get_symbol_of_declaration(node)?;
                        return self.emit_alias_resolved_to_value(symbol, true);
                    }
                }
                Ok(true)
            }
            Some(K::BinaryExpression) => {
                if self.emit_common_js_exports(node)? {
                    let right = read
                        .data_source()
                        .as_binary_expression()
                        .and_then(|data| data.right());
                    if let Some(right) = right {
                        if self.node(right)?.kind() == K::Identifier {
                            let symbol = self.get_symbol_of_declaration(node)?;
                            return self.emit_alias_resolved_to_value(symbol, true);
                        }
                    }
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.isAliasResolvedToValue
    fn emit_alias_resolved_to_value(
        &mut self,
        symbol: Option<SymbolId>,
        exclude_type_only_values: bool,
    ) -> Result<bool, Error> {
        let Some(symbol) = symbol else {
            return Ok(false);
        };
        if let Some(value) = self.symbol(symbol)?.value_declaration() {
            if let Some(container) =
                tsr_ast::utilities::get_source_file_of_node(self.ast(value)?, Some(value))?
            {
                // Ensures cjs export assignment is setup, since this symbol may point at, and merge with, the file itself.
                // If we don't, the merge may not have yet occurred, and the flags check below will be missing flags that
                // are added as a result of the merge.
                let file_symbol = self.get_symbol_of_declaration(container)?;
                self.resolve_external_module_symbol(file_symbol, false)?;
            }
        }
        let target = self.resolve_alias(symbol)?;
        let target = self.get_export_symbol_of_value_symbol_if_exported(target)?;
        if target == self.builtins.unknown_symbol {
            return Ok(!exclude_type_only_values
                || self.direct_type_only_alias_declaration(symbol)?.is_none());
        }
        // const enums and modules that contain only const enums are not considered values from the emit perspective
        // unless 'preserveConstEnums' option is set to true
        let preserve = self.program()?.host.options().should_preserve_const_enums();
        Ok(
            self.module_symbol_flags(symbol, exclude_type_only_values, true)? & sf::VALUE != 0
                && (preserve
                    || !is_const_enum_or_const_enum_only_module(self.symbol(target)?.flags())),
        )
    }
}

fn answer<T>(result: Result<T, Error>) -> ResolverResult<T> {
    result.map_err(EmitResolverError::new)
}

/// `binder.ReferenceResolver` as the checker's emit resolver answers it.
impl ReferenceResolver for Operation<'_> {
    fn get_referenced_export_container(
        &mut self,
        node: NodeId,
        prefix_locals: bool,
    ) -> ResolverResult<Option<NodeId>> {
        answer(self.referenced_export_container(node, prefix_locals))
    }
    fn get_referenced_import_declaration(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        answer(self.referenced_import_declaration(node))
    }
    fn get_referenced_value_declaration(&mut self, node: NodeId) -> ResolverResult<Option<NodeId>> {
        answer(self.referenced_value_declaration(node))
    }
    fn get_referenced_value_declarations(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<Vec<NodeId>>> {
        answer(self.referenced_value_declarations(node))
    }
    fn get_element_access_expression_name(
        &mut self,
        expression: NodeId,
    ) -> ResolverResult<tsr_ast::JsString> {
        answer(DeclarationEmitResolver::element_access_expression_name(
            self, expression,
        ))
    }
    fn get_referenced_member_value_declaration(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        answer(DeclarationEmitResolver::referenced_member_value_declaration(self, node))
    }
}

/// The script transforms' half of `printer.EmitResolver`.
impl EmitResolver for Operation<'_> {
    fn is_referenced_alias_declaration(&mut self, node: NodeId) -> ResolverResult<bool> {
        answer(Operation::is_referenced_alias_declaration(self, node))
    }
    fn is_value_alias_declaration(&mut self, node: NodeId) -> ResolverResult<bool> {
        answer(Operation::is_value_alias_declaration(self, node))
    }
    fn is_top_level_value_import_equals_with_entity_name(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<bool> {
        answer(Operation::is_top_level_value_import_equals_with_entity_name(self, node))
    }
    fn mark_linked_references_recursively(&mut self, file: NodeId) -> ResolverResult<()> {
        answer(Operation::mark_linked_references_recursively(self, file))
    }
    fn get_external_module_file_from_declaration(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        answer(DeclarationEmitResolver::external_module_file_from_declaration(self, node))
    }
    fn get_effective_declaration_flags(&mut self, node: NodeId, flags: u32) -> ResolverResult<u32> {
        answer(DeclarationEmitResolver::effective_declaration_flags(
            self, node, flags,
        ))
    }
    fn get_type_reference_serialization_kind(
        &mut self,
        name: Option<NodeId>,
        serial_scope: Option<NodeId>,
    ) -> ResolverResult<TypeReferenceSerializationKind> {
        answer(self.type_reference_serialization_kind(name, serial_scope))
    }
    fn get_constant_value(&mut self, node: NodeId) -> ResolverResult<Option<ConstantValue>> {
        answer(self.constant_value(node))
    }
    fn get_enum_member_value(&mut self, node: NodeId) -> ResolverResult<EnumMemberValue> {
        answer(DeclarationEmitResolver::enum_member_value(self, node))
    }
    fn get_jsx_factory_entity(&mut self, location: NodeId) -> ResolverResult<Option<NodeId>> {
        answer(self.jsx_factory_entity(location))
    }
    fn get_jsx_fragment_factory_entity(
        &mut self,
        location: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        answer(self.jsx_fragment_factory_entity(location))
    }
    fn set_referenced_import_declaration(
        &mut self,
        node: NodeId,
        reference: NodeId,
    ) -> ResolverResult<()> {
        answer(Operation::set_referenced_import_declaration(
            self, node, reference,
        ))
    }
}
