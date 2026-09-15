//! Source declarations and the first type queries, over the retained bound graph.
//! Unsupported branches are failures of the port, not language diagnostics.

use crate::{
    node_check_flags as nc, object_flags as of, type_flags as tf, CheckerState, Error, LinkStore,
    TypeAlias, TypeId, TypeSystemEntity, TypeSystemPropertyName,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{check_flags, node_flags as nf, symbol_flags as sf, SymbolFlags, SyntaxKind as K};
use ts_jsstring::JsString;

#[derive(Default)]
pub(crate) struct QueryState {
    pub this_assignments:
        crate::types::Map<SymbolId, crate::assignment_declarations::ThisAssignment>,
    pub resolved_symbols: LinkStore<NodeId, Option<SymbolId>>,
    pub declared_types: LinkStore<SymbolId, Option<TypeId>>,
    pub type_nodes: LinkStore<NodeId, Option<TypeId>>,
    pub global_types: crate::types::Map<&'static str, TypeId>,
    /// `deferredGlobalImportMetaExpressionType`: the synthetic `ImportMetaExpression`.
    pub import_meta_expression_type: Option<TypeId>,
    pub global_type_aliases: crate::types::Map<(&'static str, usize, bool), Option<SymbolId>>,
    pub references: LinkStore<SymbolId, SymbolFlags>,
    /// `sourceFileLinks.identifierCheckNodes`, keyed by source file.
    pub identifier_check_nodes: crate::types::Map<NodeId, Vec<NodeId>>,
    /// `sourceFileLinks.unusedChecked`, with sticky Rust errors scoped to that
    /// phase rather than invalidating a completed type check.
    pub unused_checks: crate::types::Map<NodeId, Result<(), Error>>,
    /// `Checker.renamedBindingElementsInTypes`.
    pub renamed_binding_elements_in_types: Vec<NodeId>,
    pub scope_changes: LinkStore<NodeId, ts_core::Tristate>,
    /// The union/intersection slice of deferredSymbolLinks.constituents. The containing
    /// type lives in valueSymbolLinks; no owning references back to the checker.
    pub deferred_property_write_types: LinkStore<SymbolId, Option<crate::TypeList>>,
    pub deferred_property_types: LinkStore<SymbolId, Option<crate::TypeList>>,
    pub type_aliases: LinkStore<SymbolId, crate::type_parameters::TypeAliasLinks>,
    pub outer_type_parameters: LinkStore<NodeId, Option<crate::TypeList>>,
    pub source_signatures: LinkStore<NodeId, Option<crate::SignatureId>>,
    pub apparent_types: crate::types::Map<TypeId, TypeId>,
    pub type_parameters_checked: crate::types::Set<SymbolId>,
    /// `declaredTypeLinks.interfaceChecked` and `indexSignaturesChecked`.
    pub interfaces_checked: crate::types::Set<SymbolId>,
    pub index_signatures_checked: crate::types::Set<SymbolId>,
    pub index_constraints_checked: crate::types::Set<TypeId>,
    pub accessor_pairs_checked: crate::types::Set<NodeId>,
    pub context_free_types: crate::types::Map<NodeId, TypeId>,
    pub array_literal_types: crate::types::Map<TypeId, TypeId>,
    pub widened_types: crate::types::Map<TypeId, TypeId>,
    pub assertion_types: crate::types::Map<NodeId, TypeId>,
    pub reported_unreachable: crate::types::Set<NodeId>,
    pub unresolved_symbols: crate::types::Map<JsString, SymbolId>,
    pub error_types: crate::types::Map<crate::CacheKey, TypeId>,
    pub undefined_properties: crate::types::Map<JsString, SymbolId>,
    pub function_symbols_checked: crate::types::Set<SymbolId>,
}

fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}

impl CheckerState {
    // port: tsc/internal/ast/utilities.go:IsTypeDeclaration
    pub(crate) fn is_type_declaration(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        Ok(match read.kind().known() {
            Some(
                K::TypeParameter
                | K::ClassDeclaration
                | K::InterfaceDeclaration
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration
                | K::EnumDeclaration,
            ) => true,
            Some(K::ImportClause) => read.is_type_only(),
            Some(K::ImportSpecifier | K::ExportSpecifier) => {
                let parent = required(read.parent(), "specifier parent")?;
                let grandparent = required(
                    self.ast(parent)?.node(parent)?.parent(),
                    "specifier grandparent",
                )?;
                self.ast(grandparent)?.node(grandparent)?.is_type_only()
            }
            _ => false,
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getSymbolOfDeclaration
    pub(crate) fn get_symbol_of_declaration(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(symbol) = self.raw_declaration_symbol(node)? else {
            return Ok(None);
        };
        let symbol = self.late_bound_symbol(symbol)?;
        Ok(Some(self.get_merged_symbol(symbol)))
    }

    // port: tsc/internal/checker/checker.go:Checker.getSymbolAtLocation
    pub(crate) fn get_symbol_at_location(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.flags() & nf::IN_WITH_STATEMENT != 0 {
            return Ok(None);
        }
        if read.kind() == K::SourceFile {
            let source = self.ast(node)?.source_file(node)?;
            return if ts_ast::utilities::is_external_or_common_js_module(&source) {
                self.get_symbol_of_declaration(node)
            } else {
                Ok(None)
            };
        }
        if let Some(parent) = read.parent() {
            let parent_read = self.ast(parent)?.node(parent)?;
            if crate::query_location::declaration_or_import_name(self.ast(node)?, node)? {
                if matches!(
                    parent_read.kind().known(),
                    Some(K::ImportSpecifier | K::ExportSpecifier)
                ) && parent_read.property_name() == Some(node)
                {
                    return match self.get_symbol_of_declaration(parent)? {
                        Some(symbol) => self.immediate_aliased_symbol(symbol),
                        None => Err(Error::MissingLink("import property symbol")),
                    };
                }
                return self.get_symbol_of_declaration(parent);
            }
            if parent_read.kind() == K::ComputedPropertyName
                && matches!(
                    read.kind().known(),
                    Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral | K::NumericLiteral)
                )
            {
                if let Some(declaration) = parent_read.parent() {
                    if ts_ast::is_declaration(&self.ast(declaration)?.node(declaration)?) {
                        return self.get_symbol_of_declaration(declaration);
                    }
                }
            }
        }
        if read.kind() == K::Identifier {
            if let Some(parent) = read.parent() {
                let binding = self.ast(parent)?.node(parent)?;
                if binding.kind() == K::BindingElement && binding.property_name() == Some(node) {
                    if let Some(pattern) = binding.parent() {
                        if self.ast(pattern)?.node(pattern)?.kind() == K::ObjectBindingPattern {
                            let ty = self.get_type_at_location(pattern)?;
                            let name = self.ast(node)?.node_text(node)?.into_js_string();
                            if let Some(property) =
                                self.constituent_property(ty, name.as_bytes(), false)?
                            {
                                return Ok(Some(property));
                            }
                        }
                    }
                }
            }
        }
        let read = self.ast(node)?.node(node)?;
        if read.kind() == K::Identifier {
            if let Some(parent) = read.parent() {
                let parent_read = self.ast(parent)?.node(parent)?;
                if parent_read.kind() == K::MetaProperty && parent_read.name() == Some(node) {
                    let (keyword, _) = self.meta_property_parts(parent)?;
                    let text = self.ast(node)?.node_text(node)?;
                    if keyword == K::NewKeyword && text.as_bytes() == b"target" {
                        let ty = self.check_new_target_meta_property(parent)?;
                        return Ok(self.types.get(ty)?.symbol);
                    }
                    if keyword == K::ImportKeyword && text.as_bytes() == b"meta" {
                        let ty = self.global_import_meta_expression_type()?;
                        let Some(symbol) = self.types.get(ty)?.symbol else {
                            return Ok(None);
                        };
                        let members = self.symbol(symbol)?.members();
                        return self.member_symbol(members, b"meta");
                    }
                    return Ok(None);
                }
            }
        }
        let this_in_type_query = is_this_in_type_query(self.ast(node)?, node)?;
        if !this_in_type_query
            && matches!(
                read.kind().known(),
                Some(
                    K::Identifier
                        | K::PrivateIdentifier
                        | K::PropertyAccessExpression
                        | K::QualifiedName
                )
            )
        {
            return self.symbol_of_expression_name(node);
        }
        match read.kind().known() {
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral | K::NumericLiteral) => {
                self.symbol_at_literal_location(node)
            }
            Some(K::ThisKeyword | K::Identifier) => {
                let container = ts_ast::get_this_container(self.ast(node)?, node, false, false)?;
                if ts_ast::utilities::is_function_like(Some(&self.ast(container)?.node(container)?))
                {
                    let signature = self.signature_from_declaration(container)?;
                    if let Some(this_parameter) = self.signatures.get(signature)?.this_parameter {
                        return Ok(Some(this_parameter));
                    }
                }
                if is_in_expression_context(self.ast(node)?, node)? {
                    let ty = self.check_expression(node)?;
                    return Ok(self.types.get(ty)?.symbol);
                }
                let ty = self.type_from_this_node(node)?;
                Ok(self.types.get(ty)?.symbol)
            }
            Some(K::ThisType) => {
                let ty = self.type_from_this_node(node)?;
                Ok(self.types.get(ty)?.symbol)
            }
            Some(K::SuperKeyword | K::MetaProperty) => {
                let ty = self.check_expression(node)?;
                Ok(self.types.get(ty)?.symbol)
            }
            Some(K::ConstructorKeyword) => {
                // constructor keyword for an overload, should take us to the definition if it exist
                match read.parent() {
                    Some(constructor)
                        if self.ast(constructor)?.node(constructor)?.kind() == K::Constructor =>
                    {
                        match self.ast(constructor)?.node(constructor)?.parent() {
                            Some(class) => self.raw_declaration_symbol(class),
                            None => Ok(None),
                        }
                    }
                    _ => Ok(None),
                }
            }
            Some(
                K::DefaultKeyword
                | K::FunctionKeyword
                | K::EqualsGreaterThanToken
                | K::ClassKeyword,
            ) => match read.parent() {
                Some(parent) => self.get_symbol_of_declaration(parent),
                None => Ok(None),
            },
            Some(K::ExportKeyword) => match read.parent() {
                Some(parent) if self.ast(parent)?.node(parent)?.kind() == K::ExportAssignment => {
                    self.raw_declaration_symbol(parent)
                }
                _ => Ok(None),
            },
            Some(K::ImportKeyword | K::NewKeyword) => {
                // Parsed meta-properties store their keyword as a kind field,
                // not a child node. Native checkMetaPropertyKeyword is a stub;
                // this arm cannot be reached by the parsed baseline walker.
                let Some(parent) = read.parent() else {
                    return Ok(None);
                };
                let parent_read = self.ast(parent)?.node(parent)?;
                if parent_read.kind() != K::MetaProperty {
                    return Ok(None);
                }
                if read.kind() == K::ImportKeyword {
                    if let Some(name) = parent_read.name() {
                        if self.ast(name)?.node_text(name)?.as_bytes() == b"defer" {
                            return Ok(None);
                        }
                    }
                }
                let ty = self.check_meta_property(parent)?;
                Ok(self.types.get(ty)?.symbol)
            }
            Some(K::InstanceOfKeyword) => {
                let Some(parent) = read.parent() else {
                    return Ok(None);
                };
                let parent_read = self.ast(parent)?.node(parent)?;
                let Some(right) = parent_read
                    .data_source()
                    .as_binary_expression()
                    .and_then(|data| data.right())
                else {
                    return Ok(None);
                };
                let ty = self.check_expression_cached(right)?;
                if let Some(has_instance) = self.symbol_has_instance_method_of_object_type(ty)? {
                    if let Some(symbol) = self.types.get(has_instance)?.symbol {
                        return Ok(Some(symbol));
                    }
                }
                Ok(self.types.get(ty)?.symbol)
            }
            Some(K::ImportType) => {
                let literal = read
                    .data_source()
                    .as_import_type_node()
                    .and_then(|data| data.argument())
                    .map(|argument| {
                        Ok::<_, Error>(
                            self.ast(argument)?
                                .node(argument)?
                                .data_source()
                                .as_literal_type_node()
                                .and_then(|data| data.literal()),
                        )
                    })
                    .transpose()?
                    .flatten();
                match literal {
                    Some(literal)
                        if self.ast(literal)?.node(literal)?.kind() == K::StringLiteral =>
                    {
                        self.get_symbol_at_location(literal)
                    }
                    _ => Ok(None),
                }
            }
            Some(K::JsxNamespacedName) => Err(Error::Unsupported(
                "getSymbolAtLocation: JSX namespaced name",
            )),
            _ => Ok(None),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getDeclaredTypeOfSymbol
    pub(crate) fn get_declared_type_of_symbol(
        &mut self,
        symbol: SymbolId,
    ) -> Result<TypeId, Error> {
        let flags = self.symbol(symbol)?.flags();
        if flags & (sf::CLASS | sf::INTERFACE) != 0 {
            return self.declared_interface_type(symbol);
        }
        if flags & sf::TYPE_PARAMETER != 0 {
            return self.get_declared_type_of_type_parameter(symbol);
        }
        if flags & sf::TYPE_ALIAS != 0 {
            return self.get_declared_type_of_type_alias(symbol);
        }
        if flags & sf::ENUM != 0 {
            return self.declared_enum_type(symbol);
        }
        if flags & sf::ENUM_MEMBER != 0 {
            return self.declared_enum_member_type(symbol);
        }
        if flags & sf::ALIAS != 0 {
            let target = self.resolve_alias(symbol)?;
            return self.get_declared_type_of_symbol(target);
        }
        Ok(self.builtins.error_type)
    }

    pub(crate) fn push_source_resolution(
        &mut self,
        symbol: SymbolId,
        property: TypeSystemPropertyName,
    ) -> bool {
        self.push_type_resolution(TypeSystemEntity::Symbol(symbol), property)
    }

    pub(crate) fn push_type_resolution(
        &mut self,
        target: TypeSystemEntity,
        property: TypeSystemPropertyName,
    ) -> bool {
        let cycle_start =
            self.resolution
                .find_resolution_cycle_start_index(target, property, |entry| {
                    self.type_resolution_has_property(entry)
                });
        self.resolution
            .push_after_cycle_check(target, property, cycle_start)
    }

    /// The shared probe serves both resolution pushes and speculative alias
    /// reads. Produced intermediate properties stop the cycle search.
    // port: tsc/internal/checker/checker.go:Checker.typeResolutionHasProperty
    pub(crate) fn type_resolution_has_property(
        &self,
        entry: &crate::resolution::TypeResolution,
    ) -> bool {
        use TypeSystemEntity::{Node, Signature, Symbol, Type};
        use TypeSystemPropertyName as Property;
        match (entry.property_name, entry.target) {
            (Property::AliasTarget, Symbol(symbol)) => self
                .module_aliases
                .targets
                .get(&symbol)
                .is_some_and(Result::is_ok),
            (Property::DeclaredType, Symbol(symbol)) => self
                .query
                .declared_types
                .try_get(symbol)
                .is_some_and(Option::is_some),
            (Property::Type, Symbol(symbol)) => self
                .value_symbol_links
                .try_get(symbol)
                .is_some_and(|links| links.resolved_type.is_some()),
            (Property::WriteType, Symbol(symbol)) => self
                .value_symbol_links
                .try_get(symbol)
                .is_some_and(|links| links.write_type.is_some()),
            (Property::InitializerIsUndefined, Node(node)) => {
                self.emit_checks
                    .node_flags
                    .try_get(node)
                    .copied()
                    .unwrap_or_default()
                    & nc::INITIALIZER_IS_UNDEFINED_COMPUTED
                    != 0
            }
            (Property::ResolvedTypeArguments, Type(ty)) => self
                .types
                .type_reference(ty)
                .is_ok_and(|data| data.resolved_type_arguments.is_some()),
            (Property::ResolvedBaseTypes, Type(ty)) => self
                .types
                .interface(ty)
                .is_ok_and(|data| data.base_types_resolved),
            (Property::ResolvedBaseConstructorType, Type(ty)) => self
                .types
                .interface(ty)
                .is_ok_and(|data| data.resolved_base_constructor_type.is_some()),
            (Property::ResolvedReturnType, Signature(signature)) => self
                .signatures
                .get(signature)
                .is_ok_and(|data| data.resolved_return_type.is_some()),
            (Property::ResolvedBaseConstraint, Type(ty)) => self
                .types
                .base_constraint_slot(ty)
                .is_ok_and(|slot| slot.is_some_and(Option::is_some)),
            // Entity/property mismatches are invalid internal entries. As in
            // the other checked resolution probes, they cannot be cache hits.
            _ => false,
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getDeclaredTypeOfTypeAlias
    fn get_declared_type_of_type_alias(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        if let Some(Some(ty)) = self.query.declared_types.try_get(symbol) {
            return Ok(*ty);
        }
        let declarations = self.symbol_declarations(symbol)?.to_vec();
        let mut declaration = None;
        // Upstream's `IsTypeOrJSTypeAliasDeclaration`: a JSDoc `@typedef` or
        // `@callback` declares a JS type alias.
        for node in declarations.into_iter().flatten() {
            if matches!(
                self.ast(node)?.node(node)?.kind().known(),
                Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
            ) {
                declaration = Some(node);
                break;
            }
        }
        let declaration = required(declaration, "type alias declaration")?;
        let read = self.ast(declaration)?.node(declaration)?;
        let type_node = required(read.type_node(), "type alias annotation")?;
        if !self.push_source_resolution(symbol, TypeSystemPropertyName::DeclaredType) {
            return Ok(self.builtins.error_type);
        }
        let result = self.get_type_from_type_node(type_node);
        let complete = self.resolution.pop();
        let mut ty = result?;
        if complete {
            let parameters = self.get_local_type_parameters(symbol)?;
            if !parameters.is_empty() {
                let key = self.object_instantiation_key(&parameters, None, false)?;
                let links = self.query.type_aliases.get_or_default(symbol);
                links.instantiations.insert(key, ty);
                links.parameters = Some(parameters);
            }
            if ty == self.builtins.intrinsic_marker_type
                && self.symbol(symbol)?.name_bytes() == b"BuiltinIteratorReturn"
            {
                let options = self.program()?.host.options();
                ty = if options.strict_option_value(options.strict_builtin_iterator_return) {
                    self.builtins.undefined_type
                } else {
                    self.builtins.any_type
                };
            }
        } else {
            let error_node = self
                .ast(declaration)?
                .node(declaration)?
                .name()
                .unwrap_or(declaration);
            let name = self.symbol_to_string(symbol)?;
            self.error_at(
                Some(error_node),
                ts_diagnostics::Type_alias_0_circularly_references_itself,
                vec![name],
            )?;
            ty = self.builtins.error_type;
        }
        Ok(*self
            .query
            .declared_types
            .get_or_default(symbol)
            .get_or_insert(ty))
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromTypeNode
    pub(crate) fn get_type_from_type_node(&mut self, node: NodeId) -> Result<TypeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let ty = self.get_type_from_type_node_worker(node)?;
            self.conditional_flow_type(ty, node)
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromTypeNodeWorker
    fn get_type_from_type_node_worker(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let builtin = match read.kind().known() {
            Some(K::AnyKeyword | K::JSDocAllType) => Some(self.builtins.any_type),
            Some(K::UnknownKeyword) => Some(self.builtins.unknown_type),
            Some(K::StringKeyword) => Some(self.builtins.string_type),
            Some(K::NumberKeyword) => Some(self.builtins.number_type),
            Some(K::BigIntKeyword) => Some(self.builtins.bigint_type),
            Some(K::BooleanKeyword) => Some(self.builtins.boolean_type),
            Some(K::SymbolKeyword) => Some(self.builtins.es_symbol_type),
            Some(K::VoidKeyword) => Some(self.builtins.void_type),
            Some(K::UndefinedKeyword) => Some(self.builtins.undefined_type),
            Some(K::NullKeyword) => Some(self.builtins.null_type),
            Some(K::NeverKeyword) => Some(self.builtins.never_type),
            Some(K::ObjectKeyword) => Some(self.builtins.non_primitive_type),
            Some(K::IntrinsicKeyword) => Some(self.builtins.intrinsic_marker_type),
            _ => None,
        };
        if let Some(ty) = builtin {
            return Ok(ty);
        }
        if let Some(Some(ty)) = self.query.type_nodes.try_get(node) {
            return Ok(*ty);
        }
        let ty = match read.kind().known() {
            Some(K::TypeQuery) => self.source_type_query(node)?,
            Some(K::ImportType) => self.type_from_import_node(node)?,
            Some(K::ConditionalType) => self.source_conditional_type(node)?,
            Some(K::InferType) => self.source_infer_type(node)?,
            Some(K::UnionType | K::IntersectionType) => {
                self.get_type_from_union_or_intersection_type_node(node)?
            }
            Some(K::JSDocNullableType | K::JSDocOptionalType | K::JSDocVariadicType) => {
                let kind = read.kind();
                let annotation = if let Some(data) = read.data_source().as_js_doc_variadic_type() {
                    data.r#type()
                } else if let Some(data) = read.data_source().as_js_doc_optional_type() {
                    data.r#type()
                } else {
                    read.type_node()
                };
                let annotation = required(annotation, "JSDoc type operand")?;
                let ty = self.get_type_from_type_node(annotation)?;
                if kind == K::JSDocVariadicType {
                    self.create_array_type(ty, false)?
                } else if kind == K::JSDocOptionalType {
                    self.add_type_optionality(ty, false, true)?
                } else if self.options.strict_null_checks {
                    self.nullable_type(ty, tf::NULL)?
                } else {
                    ty
                }
            }
            Some(K::ParenthesizedType | K::JSDocNonNullableType) => {
                return self
                    .get_type_from_type_node(required(read.type_node(), "parenthesized type")?)
            }
            Some(K::LiteralType) => {
                let literal = required(
                    read.data_source()
                        .as_literal_type_node()
                        .ok_or(ts_arena::Error::InvalidGraph)?
                        .literal(),
                    "literal type",
                )?;
                if self.ast(literal)?.node(literal)?.kind() == K::NullKeyword {
                    return Ok(self.builtins.null_type);
                }
                let ty = self.check_expression(literal)?;
                self.get_regular_type_of_literal_type(ty)?
            }
            Some(K::TypeLiteral | K::FunctionType | K::ConstructorType) => {
                let symbol = self.get_symbol_of_declaration(node)?;
                let alias = self.alias_for_type_node(node)?;
                let members = symbol
                    .map(|symbol| self.members_of_symbol(symbol))
                    .transpose()?
                    .flatten();
                let no_members = match members {
                    Some(table) => self.table(table)?.is_empty(),
                    None => true,
                };
                if symbol.is_none() || no_members && alias.is_none() {
                    self.builtins.empty_type_literal_type
                } else {
                    let ty = self.new_object_type(of::ANONYMOUS, symbol)?;
                    if let Some(alias) = alias {
                        self.types.get_mut(ty)?.alias = Some(self.types.push_alias(alias)?);
                    }
                    ty
                }
            }
            Some(K::TypeReference | K::ExpressionWithTypeArguments) => {
                match self.intended_jsdoc_type(node)? {
                    Some(ty) => ty,
                    None => self.source_type_reference(node)?,
                }
            }
            Some(K::MappedType) => self.source_mapped_type(node)?,
            Some(K::TemplateLiteralType) => self.source_template_type(node)?,
            Some(K::ThisType | K::ThisKeyword) => self.type_from_this_node(node)?,
            Some(K::IndexedAccessType) => self.source_indexed_access_type(node)?,
            Some(K::TypeOperator)
                if read
                    .data_source()
                    .as_type_operator_node()
                    .is_some_and(|data| data.operator() == K::KeyOfKeyword) =>
            {
                let operand = read
                    .type_node()
                    .ok_or(Error::MissingLink("keyof operand"))?;
                let ty = self.get_type_from_type_node(operand)?;
                self.get_index_type(ty, 0)?
            }
            Some(K::TypeOperator)
                if read
                    .data_source()
                    .as_type_operator_node()
                    .is_some_and(|data| data.operator() == K::UniqueKeyword) =>
            {
                let argument = read
                    .type_node()
                    .ok_or(Error::MissingLink("unique type operand"))?;
                let mut declaration = read
                    .parent()
                    .ok_or(Error::MissingLink("unique type parent"))?;
                if self.ast(argument)?.node(argument)?.kind() == K::SymbolKeyword {
                    while self.ast(declaration)?.node(declaration)?.kind() == K::ParenthesizedType {
                        declaration = self
                            .ast(declaration)?
                            .node(declaration)?
                            .parent()
                            .ok_or(Error::MissingLink("parenthesized unique type parent"))?;
                    }
                    self.es_symbol_like_type_for_node(declaration)?
                } else {
                    self.builtins.error_type
                }
            }
            Some(K::TypePredicate) => {
                if read
                    .data_source()
                    .as_type_predicate_node()
                    .is_some_and(|data| data.asserts_modifier().is_some())
                {
                    self.builtins.void_type
                } else {
                    self.builtins.boolean_type
                }
            }
            Some(K::ArrayType | K::TupleType) => self.source_array_or_tuple_type(node)?,
            Some(K::OptionalType | K::RestType | K::NamedTupleMember) => {
                self.source_tuple_element_type(node)?
            }
            Some(K::TypeOperator)
                if read
                    .data_source()
                    .as_type_operator_node()
                    .is_some_and(|data| data.operator() == K::ReadonlyKeyword) =>
            {
                self.get_type_from_type_node(
                    read.type_node()
                        .ok_or(Error::MissingLink("readonly type"))?,
                )?
            }
            // Native default, reached by location queries on the identifier
            // inside a TypeReference, among other non-type syntax. All native
            // type-family arms are handled above; this is not an unported type.
            _ => return Ok(self.builtins.error_type),
        };
        *self.query.type_nodes.get_or_default(node) = Some(ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getAliasForTypeNode
    pub(crate) fn alias_for_type_node(&mut self, node: NodeId) -> Result<Option<TypeAlias>, Error> {
        let mut parent = self.ast(node)?.node(node)?.parent();
        while let Some(current) = parent {
            let read = self.ast(current)?.node(current)?;
            if read.kind() == K::ParenthesizedType
                || read
                    .data_source()
                    .as_type_operator_node()
                    .is_some_and(|data| data.operator() == K::ReadonlyKeyword)
            {
                parent = read.parent();
                continue;
            }
            if !matches!(
                read.kind().known(),
                Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
            ) {
                return Ok(None);
            }
            let Some(symbol) = self.get_symbol_of_declaration(current)? else {
                return Ok(None);
            };
            let type_arguments = self.get_local_type_parameters(symbol)?;
            return Ok(Some(TypeAlias {
                symbol,
                type_arguments,
            }));
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromUnionTypeNode
    // port: tsc/internal/checker/checker.go:Checker.getTypeFromIntersectionTypeNode
    fn get_type_from_union_or_intersection_type_node(
        &mut self,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        let alias = self.alias_for_type_node(node)?;
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let is_union = read.kind() == K::UnionType;
        let list = required(
            if is_union {
                read.data_source()
                    .as_union_type_node()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .types()
            } else {
                read.data_source()
                    .as_intersection_type_node()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .types()
            },
            "compound types",
        )?;
        let nodes = view
            .node_slice(view.list(list)?.nodes())?
            .iter()
            .collect::<Vec<_>>();
        let mut types = Vec::with_capacity(nodes.len());
        for node in nodes {
            types.push(self.get_type_from_type_node(required(node, "compound constituent")?)?);
        }
        let alias = alias
            .map(|alias| self.types.push_alias(alias))
            .transpose()?;
        if is_union {
            return self.get_union_type_ex(&types, crate::UnionReduction::Literal, alias, None);
        }
        let mut flags = 0;
        if types.len() == 2 {
            if let Some(empty) = types
                .iter()
                .position(|&ty| ty == self.builtins.empty_type_literal_type)
            {
                let other = self.types.flags(types[1 - empty])?;
                if other & tf::TEMPLATE_LITERAL != 0 {
                    return Err(Error::Unsupported(
                        "getTypeFromIntersectionTypeNode: pattern literal",
                    ));
                }
                if other & (tf::STRING | tf::NUMBER | tf::BIG_INT) != 0 {
                    flags = crate::intersection::NO_SUPERTYPE_REDUCTION;
                }
            }
        }
        self.get_intersection_type_ex(&types, flags, alias)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkExpressionCached
    // port: tsc/internal/checker/checker.go:Checker.checkExpressionCachedEx
    pub(crate) fn check_expression_cached(&mut self, node: NodeId) -> Result<TypeId, Error> {
        if let Some(Some(ty)) = self.query.type_nodes.try_get(node) {
            return Ok(*ty);
        }
        let saved_loops = std::mem::take(&mut self.flow.loop_stack);
        let saved_cache = std::mem::take(&mut self.flow.expression_cache);
        let result = self.check_expression(node);
        self.flow.loop_stack = saved_loops;
        self.flow.expression_cache = saved_cache;
        let ty = result?;
        *self.query.type_nodes.get_or_default(node) = Some(ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkExpressionWorker
    pub(crate) fn check_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.check_expression_ex(node, 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkExpressionEx
    pub(crate) fn check_expression_ex(&mut self, node: NodeId, mode: u32) -> Result<TypeId, Error> {
        let previous_mode = std::mem::replace(&mut self.expression_mode, mode);
        let previous = self.current_node.replace(node);
        self.instantiation.count = 0;
        let result = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let ty = self.check_expression_worker(node)?;
            let ty = self.instantiate_single_generic_function(node, ty, mode)?;
            if self.const_enum_object_type(ty)? {
                self.check_const_enum_access(node, ty)?;
            }
            Ok(ty)
        });
        self.current_node = previous;
        self.expression_mode = previous_mode;
        result
    }

    fn check_expression_worker(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let ty = match read.kind().known() {
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral) => {
                let text = self.ast(node)?.node_text(node)?;
                self.get_string_literal_type(JsString::from_bytes(text.as_bytes()))?
            }
            Some(K::NumericLiteral) => {
                self.check_grammar_numeric_literal(node)?;
                let text = self.ast(node)?.node_text(node)?;
                self.get_number_literal_type(ts_jsnum::from_string(text.as_bytes()))?
            }
            Some(K::BigIntLiteral) => {
                self.check_grammar_big_int_literal(node)?;
                let text = self.ast(node)?.node_text(node)?;
                self.get_big_int_literal_type(ts_jsnum::PseudoBigInt::new(
                    &ts_jsnum::parse_pseudo_big_int(text.as_bytes()),
                    false,
                ))?
            }
            Some(K::TrueKeyword) => return Ok(self.builtins.true_type),
            Some(K::FalseKeyword) => return Ok(self.builtins.false_type),
            Some(K::NullKeyword) => return Ok(self.builtins.null_widening_type),
            Some(K::ParenthesizedExpression) => {
                return self.check_expression_ex(
                    required(read.expression(), "parenthesized expression")?,
                    self.expression_mode,
                )
            }
            Some(K::SyntheticExpression) => return self.check_synthetic_expression(node),
            Some(K::SpreadElement) => {
                return self.check_spread_expression(node, self.expression_mode)
            }
            Some(K::PrivateIdentifier) => self.check_private_identifier_expression(node)?,
            Some(K::Identifier) => return self.check_identifier(node),
            Some(K::ElementAccessExpression) => return self.check_element_access(node),
            Some(K::PropertyAccessExpression | K::QualifiedName) => {
                return self.check_property_expression(node)
            }
            Some(K::FunctionExpression | K::ArrowFunction) => {
                return self.check_function_expression(node)
            }
            Some(K::CallExpression)
                if crate::external_resolution::is_import_call(self.ast(node)?, &read)? =>
            {
                return self.check_import_call_expression(node)
            }
            Some(K::CallExpression | K::NewExpression) => return self.check_call_expression(node),
            Some(K::ExpressionWithTypeArguments) => {
                return self.check_instantiation_expression(node)
            }
            Some(K::TaggedTemplateExpression) => {
                return self.check_tagged_template_expression(node)
            }
            Some(K::ObjectLiteralExpression) => return self.check_object_literal(node),
            Some(K::ArrayLiteralExpression) => return self.check_array_literal(node),
            Some(K::TemplateExpression) => return self.check_template_expression(node),
            Some(K::YieldExpression) => return self.check_yield_expression(node),
            Some(K::AwaitExpression) => return self.check_await_expression(node),
            Some(K::AsExpression | K::TypeAssertionExpression) => {
                return self.check_assertion_expression(node)
            }
            Some(K::SatisfiesExpression) => return self.check_satisfies_expression(node),
            Some(K::NonNullExpression) => return self.check_non_null_assertion(node),
            Some(K::ThisKeyword) => return self.check_this_expression(node),
            Some(K::SuperKeyword) => return self.check_super_expression(node),
            Some(K::ClassExpression) => return self.check_class_expression(node),
            Some(K::OmittedExpression) => return Ok(self.builtins.undefined_widening_type),
            Some(K::BinaryExpression) => return self.check_binary_expression(node),
            Some(K::ConditionalExpression) => return self.check_conditional_expression(node),
            Some(K::TypeOfExpression) => {
                let expression = required(read.expression(), "typeof operand")?;
                self.check_expression(expression)?;
                return Ok(self.typeof_result_type());
            }
            Some(K::VoidExpression) => {
                let expression = required(read.expression(), "void operand")?;
                self.check_expression(expression)?;
                return Ok(self.builtins.undefined_widening_type);
            }
            Some(K::PrefixUnaryExpression | K::PostfixUnaryExpression) => {
                return self.check_unary_expression(node)
            }
            Some(K::RegularExpressionLiteral) => {
                return self.check_regular_expression_literal(node)
            }
            Some(K::DeleteExpression) => return self.check_delete_expression(node),
            Some(K::MetaProperty) => return self.check_meta_property(node),
            // JSX expressions are outside the frozen denominator; every other
            // kind upstream accepts is ported above.
            _ => return Err(Error::Unsupported("checkExpressionWorker")),
        };
        self.get_fresh_type_of_literal_type(ty)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarNumericLiteral
    pub(crate) fn check_grammar_numeric_literal(&mut self, node: NodeId) -> Result<(), Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let literal = read
            .data_source()
            .as_numeric_literal()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        // The scanner's normalized text can itself acquire a decimal point.
        // Fractional spelling is determined from the original source bytes.
        if literal.token_flags() & ts_ast::token_flags::SCIENTIFIC != 0
            || ts_scanner::get_text_of_node(view, node)?
                .as_bytes()
                .contains(&b'.')
            || ts_jsnum::from_string(view.node_text(node)?.as_bytes()).value()
                <= 9_007_199_254_740_991.0
        {
            return Ok(());
        }
        let diagnostic = self.diagnostic_for_node(Some(node), ts_diagnostics::Numeric_literals_with_absolute_values_equal_to_2_53_or_greater_are_too_large_to_be_represented_accurately_as_integers, Vec::new())?;
        self.add_suggestion_diagnostic(diagnostic)?;
        Ok(())
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarBigIntLiteral
    fn check_grammar_big_int_literal(&mut self, node: NodeId) -> Result<(), Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let parent = read.parent().map(|parent| view.node(parent)).transpose()?;
        let literal_type = match parent {
            Some(parent) if parent.kind() == K::LiteralType => true,
            Some(parent) if parent.kind() == K::PrefixUnaryExpression => parent
                .parent()
                .map(|grandparent| {
                    view.node(grandparent)
                        .map(|node| node.kind() == K::LiteralType)
                })
                .transpose()?
                .unwrap_or(false),
            _ => false,
        };
        if !literal_type
            && read.flags() & nf::AMBIENT == 0
            && self.program()?.host.options().emit_script_target() < ts_core::ScriptTarget::ES2020
        {
            let source = required(
                ts_ast::utilities::get_source_file_of_node(view, Some(node))?,
                "bigint source",
            )?;
            if view.source_file(source)?.diagnostics().is_empty() {
                self.error_at(Some(node), ts_diagnostics::BigInt_literals_are_not_available_when_targeting_lower_than_ES2020, Vec::new())?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.isLiteralOfContextualType
    pub(crate) fn literal_of_context(
        &mut self,
        candidate: TypeId,
        contextual: Option<TypeId>,
    ) -> Result<bool, Error> {
        let Some(contextual) = contextual else {
            return Ok(false);
        };
        let flags = self.types.flags(contextual)?;
        if flags & tf::UNION_OR_INTERSECTION != 0 {
            for &ty in self.types.compound_types(contextual)?.clone().iter() {
                if self.literal_of_context(candidate, Some(ty))? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if flags & tf::INSTANTIABLE_NON_PRIMITIVE != 0 {
            let constraint = self
                .base_constraint_of_type(contextual)?
                .unwrap_or(self.builtins.unknown_type);
            for (primitive, literal) in [
                (tf::STRING, tf::STRING_LITERAL),
                (tf::NUMBER, tf::NUMBER_LITERAL),
                (tf::BIG_INT, tf::BIG_INT_LITERAL),
                (tf::ES_SYMBOL, tf::UNIQUE_ES_SYMBOL),
            ] {
                if self.maybe_type_of_kind(constraint, primitive)?
                    && self.maybe_type_of_kind(candidate, literal)?
                {
                    return Ok(true);
                }
            }
            return self.literal_of_context(candidate, Some(constraint));
        }
        Ok(
            flags & (tf::STRING_LITERAL | tf::INDEX | tf::TEMPLATE_LITERAL | tf::STRING_MAPPING)
                != 0
                && self.maybe_type_of_kind(candidate, tf::STRING_LITERAL)?
                || flags & tf::NUMBER_LITERAL != 0
                    && self.maybe_type_of_kind(candidate, tf::NUMBER_LITERAL)?
                || flags & tf::BIG_INT_LITERAL != 0
                    && self.maybe_type_of_kind(candidate, tf::BIG_INT_LITERAL)?
                || flags & tf::BOOLEAN_LITERAL != 0
                    && self.maybe_type_of_kind(candidate, tf::BOOLEAN_LITERAL)?
                || flags & tf::UNIQUE_ES_SYMBOL != 0
                    && self.maybe_type_of_kind(candidate, tf::UNIQUE_ES_SYMBOL)?,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfSymbol
    pub(crate) fn get_type_of_symbol(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        let read = self.symbol(symbol)?;
        if read.check_flags() & check_flags::DEFERRED_TYPE != 0 {
            return self.get_type_of_symbol_with_deferred_type(symbol);
        }
        if read.check_flags() & check_flags::INSTANTIATED != 0 {
            return self.get_type_of_instantiated_symbol(symbol);
        }
        if read.check_flags() & check_flags::MAPPED != 0 {
            return self.type_of_mapped_symbol(symbol);
        }
        if read.check_flags() & check_flags::REVERSE_MAPPED != 0 {
            return self.type_of_reverse_mapped_symbol(symbol);
        }
        if let Some(ty) = self
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.resolved_type)
        {
            return Ok(ty);
        }
        if read.flags() & sf::CLASS != 0 {
            return self.type_of_class(symbol);
        }
        if read.flags() & sf::PROTOTYPE != 0 {
            return self.type_of_prototype(symbol);
        }
        if read.flags() & sf::ENUM != 0 {
            return self.type_of_enum(symbol);
        }
        if read.flags() & sf::ENUM_MEMBER != 0 {
            return self.type_of_enum_member(symbol);
        }
        if read.flags() & sf::ACCESSOR != 0 {
            return self.type_of_accessors(symbol);
        }
        if read.flags() & sf::VALUE_MODULE != 0 && self.shorthand_ambient_module(symbol)? {
            return Ok(self.builtins.any_type);
        }
        if read.flags() & (sf::FUNCTION | sf::METHOD | sf::VALUE_MODULE) != 0 {
            let optional = read.flags() & sf::OPTIONAL != 0;
            let ty = self.new_object_type(of::ANONYMOUS, Some(symbol))?;
            let ty = self.add_type_optionality(ty, true, optional)?;
            self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
            return Ok(ty);
        }
        if read.flags() & sf::ALIAS != 0 && read.flags() & (sf::VARIABLE | sf::PROPERTY) == 0 {
            return self.type_of_alias(symbol);
        }
        if read.flags() & (sf::VARIABLE | sf::PROPERTY) == 0 {
            return Ok(self.builtins.error_type);
        }
        if symbol == self.builtins.require_symbol {
            return Ok(self.builtins.any_type);
        }
        let declaration = required(read.value_declaration(), "value declaration")?;
        if self.ast(declaration)?.node(declaration)?.kind() == K::SourceFile {
            let source = self.ast(declaration)?.source_file(declaration)?;
            if source.script_kind == ts_core::ScriptKind::JSON {
                let statements = self.source_list(
                    declaration,
                    self.ast(declaration)?.node(declaration)?.statement_list(),
                )?;
                let ty = if let Some(&statement) = statements.first() {
                    let expression = required(
                        self.ast(statement)?.node(statement)?.expression(),
                        "JSON root expression",
                    )?;
                    let ty = self.check_expression(expression)?;
                    let ty = self.widen_literal_type(ty)?;
                    self.widened_type(ty)?
                } else {
                    self.builtins.empty_object_type
                };
                self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
                return Ok(ty);
            }
        }
        if !self.push_source_resolution(symbol, TypeSystemPropertyName::Type) {
            return self.report_symbol_circularity(symbol);
        }
        let result = (|| {
            if self.symbol(symbol)?.flags() & sf::MODULE_EXPORTS != 0 {
                if self.symbol(symbol)?.name_to_owned().as_bytes() == b"exports" {
                    let module = self
                        .get_symbol_of_declaration(declaration)?
                        .ok_or(Error::MissingLink("exports source symbol"))?;
                    let export = self
                        .resolve_external_module_symbol(Some(module), false)?
                        .ok_or(Error::MissingLink("resolved exports symbol"))?;
                    self.get_type_of_symbol(export)
                } else {
                    let members = self.symbol(symbol)?.members();
                    self.new_anonymous_type(Some(symbol), members, &[], &[], &[])
                }
            } else if self.ast(declaration)?.node(declaration)?.kind() == K::PropertyAssignment {
                self.check_object_property_assignment(declaration, 0)
            } else if self.ast(declaration)?.node(declaration)?.kind()
                == K::ShorthandPropertyAssignment
            {
                self.check_shorthand_property_assignment(declaration, true, 0)
            } else if self.ast(declaration)?.node(declaration)?.kind() == K::MethodDeclaration {
                self.check_object_literal_method(declaration)
            } else if matches!(
                self.ast(declaration)?.node(declaration)?.kind().known(),
                Some(K::BinaryExpression | K::CallExpression)
            ) {
                self.widened_assignment_declaration_type(symbol)
            } else if self.ast(declaration)?.node(declaration)?.kind() == K::ExportAssignment {
                let read = self.ast(declaration)?.node(declaration)?;
                if let Some(annotation) = read.type_node() {
                    self.get_type_from_type_node(annotation)
                } else {
                    let expression = required(read.expression(), "export assignment expression")?;
                    let ty = self.check_expression_cached(expression)?;
                    self.widen_type_for_variable_like(declaration, Some(ty), false)
                }
            } else {
                self.type_of_variable_like(declaration)
            }
        })();
        let complete = self.resolution.pop();
        let ty = result?;
        let ty = if complete {
            ty
        } else {
            self.report_symbol_circularity(symbol)?
        };
        if self
            .value_symbol_links
            .get_or_default(symbol)
            .resolved_type
            .is_none()
            && !self.parameter_of_context_sensitive_signature(declaration)?
        {
            self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getWidenedLiteralType
    pub(crate) fn widen_literal_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if self.is_fresh_literal_type(ty)? {
            if self.types.flags(ty)? & tf::ENUM_LIKE != 0 {
                return self.base_type_of_enum_like(ty);
            }
            return Ok(match self.types.flags(ty)? {
                tf::STRING_LITERAL => self.builtins.string_type,
                tf::NUMBER_LITERAL => self.builtins.number_type,
                tf::BIG_INT_LITERAL => self.builtins.bigint_type,
                tf::BOOLEAN_LITERAL => self.builtins.boolean_type,
                _ => return Err(Error::Unsupported("getWidenedLiteralType: enum")),
            });
        }
        if self.types.flags(ty)? & tf::UNION != 0 {
            return self
                .map_type(ty, &mut |checker, part| {
                    checker.widen_literal_type(part).map(Some)
                })?
                .ok_or(Error::MissingLink("widened literal union"));
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getApparentType
    pub(crate) fn properties_of_primitive_type(
        &mut self,
        ty: TypeId,
    ) -> Result<Vec<SymbolId>, Error> {
        let Some(apparent) = self.apparent_primitive_type(ty)? else {
            return Ok(Vec::new());
        };
        self.resolve_type_members(apparent)?;
        Ok(self
            .types
            .structured(apparent)?
            .properties
            .as_deref()
            .unwrap_or_default()
            .to_vec())
    }

    pub(crate) fn apparent_primitive_type(&mut self, ty: TypeId) -> Result<Option<TypeId>, Error> {
        let flags = self.types.flags(ty)?;
        let name = if flags & tf::STRING_LIKE != 0 {
            Some("String")
        } else if flags & tf::NUMBER_LIKE != 0 {
            Some("Number")
        } else if flags & tf::BOOLEAN_LIKE != 0 {
            Some("Boolean")
        } else if flags & tf::BIG_INT_LIKE != 0 {
            Some("BigInt")
        } else if flags & tf::ES_SYMBOL_LIKE != 0 {
            Some("Symbol")
        } else if flags & tf::NON_PRIMITIVE != 0 {
            return Ok(Some(self.builtins.empty_object_type));
        } else if flags & (tf::ANY | tf::UNKNOWN | tf::VOID | tf::UNDEFINED | tf::NULL | tf::NEVER)
            != 0
        {
            None
        } else {
            return Err(Error::Unsupported("getPropertiesOfType: apparent type"));
        };
        let Some(name) = name else {
            return Ok(None);
        };
        // These global resolver closures are lazy and do not report an absent
        // library type. Preserve that ordering instead of eagerly loading them
        // during program initialization.
        if matches!(name, "BigInt" | "Symbol") && !self.query.global_types.contains_key(name) {
            let ty = self.get_global_type(name, 0, false)?;
            self.query.global_types.insert(name, ty);
        }
        let apparent = self
            .query
            .global_types
            .get(name)
            .copied()
            .ok_or(Error::Unsupported(
                "getApparentType without program initialization",
            ))?;
        Ok(Some(apparent))
    }
}

// port: tsc/internal/ast/utilities.go:IsInExpressionContext
pub(crate) fn is_in_expression_context(
    view: ts_ast::AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    let Some(parent) = view.node(node)?.parent() else {
        return Ok(false);
    };
    let read = view.node(parent)?;
    Ok(match read.kind().known() {
        Some(
            K::VariableDeclaration
            | K::Parameter
            | K::PropertyDeclaration
            | K::PropertySignature
            | K::EnumMember
            | K::PropertyAssignment
            | K::BindingElement,
        ) => read.initializer() == Some(node),
        Some(
            K::ExpressionStatement
            | K::IfStatement
            | K::DoStatement
            | K::WhileStatement
            | K::ReturnStatement
            | K::WithStatement
            | K::SwitchStatement
            | K::CaseClause
            | K::DefaultClause
            | K::ThrowStatement
            | K::TypeAssertionExpression
            | K::AsExpression
            | K::TemplateSpan
            | K::ComputedPropertyName
            | K::SatisfiesExpression,
        ) => read.expression() == Some(node),
        Some(K::ForStatement) => {
            let data = read
                .data_source()
                .as_for_statement()
                .ok_or(ts_arena::Error::InvalidGraph)?;
            data.initializer() == Some(node)
                && view.node(node)?.kind() != K::VariableDeclarationList
                || data.condition() == Some(node)
                || data.incrementor() == Some(node)
        }
        Some(K::ForInStatement | K::ForOfStatement) => {
            let (initializer, expression) = match read.data_source().as_for_in_or_of_statement() {
                Some(data) => (data.initializer(), data.expression()),
                None => (None, None),
            };
            initializer == Some(node) && view.node(node)?.kind() != K::VariableDeclarationList
                || expression == Some(node)
        }
        Some(K::Decorator | K::JsxExpression | K::JsxSpreadAttribute | K::SpreadAssignment) => true,
        Some(K::ExpressionWithTypeArguments) => {
            read.expression() == Some(node) && !part_of_type_node(view, parent)?
        }
        Some(K::ShorthandPropertyAssignment) => {
            read.data_source()
                .as_shorthand_property_assignment()
                .and_then(|data| data.object_assignment_initializer())
                == Some(node)
        }
        _ => is_expression_node(view, parent)?,
    })
}

// port: tsc/internal/ast/utilities.go:IsExpressionNode
pub(crate) fn is_expression_node(view: ts_ast::AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    Ok(match read.kind().known() {
        Some(
            K::SuperKeyword
            | K::NullKeyword
            | K::TrueKeyword
            | K::FalseKeyword
            | K::RegularExpressionLiteral
            | K::ArrayLiteralExpression
            | K::ObjectLiteralExpression
            | K::PropertyAccessExpression
            | K::ElementAccessExpression
            | K::CallExpression
            | K::NewExpression
            | K::TaggedTemplateExpression
            | K::AsExpression
            | K::TypeAssertionExpression
            | K::SatisfiesExpression
            | K::NonNullExpression
            | K::ParenthesizedExpression
            | K::FunctionExpression
            | K::ClassExpression
            | K::ArrowFunction
            | K::VoidExpression
            | K::DeleteExpression
            | K::TypeOfExpression
            | K::PrefixUnaryExpression
            | K::PostfixUnaryExpression
            | K::BinaryExpression
            | K::ConditionalExpression
            | K::SpreadElement
            | K::TemplateExpression
            | K::OmittedExpression
            | K::JsxElement
            | K::JsxSelfClosingElement
            | K::JsxFragment
            | K::YieldExpression
            | K::AwaitExpression,
        ) => true,
        Some(K::MetaProperty) => {
            // `import.defer` in `import.defer(...)` is not an expression
            match read.parent() {
                Some(parent) => {
                    let parent_read = view.node(parent)?;
                    !crate::external_resolution::is_import_call(view, &parent_read)?
                        || parent_read.expression() != Some(node)
                }
                None => true,
            }
        }
        Some(K::ExpressionWithTypeArguments) => match read.parent() {
            Some(parent) => view.node(parent)?.kind() != K::HeritageClause,
            None => true,
        },
        Some(K::QualifiedName) => {
            let mut current = node;
            while let Some(parent) = view.node(current)?.parent() {
                if view.node(parent)?.kind() != K::QualifiedName {
                    break;
                }
                current = parent;
            }
            match view.node(current)?.parent() {
                Some(parent) => matches!(
                    view.node(parent)?.kind().known(),
                    Some(
                        K::TypeQuery
                            | K::JSDocLink
                            | K::JSDocLinkCode
                            | K::JSDocLinkPlain
                            | K::JSDocNameReference
                    )
                ),
                None => false,
            }
        }
        Some(K::PrivateIdentifier) => match read.parent() {
            Some(parent) => {
                let parent_read = view.node(parent)?;
                match parent_read.data_source().as_binary_expression() {
                    Some(data) => {
                        data.left() == Some(node)
                            && data
                                .operator_token()
                                .map(|token| view.node(token).map(|read| read.kind()))
                                .transpose()?
                                == Some(K::InKeyword.into())
                    }
                    None => false,
                }
            }
            None => false,
        },
        Some(K::Identifier) => {
            let type_context = match read.parent() {
                Some(parent) => matches!(
                    view.node(parent)?.kind().known(),
                    Some(
                        K::TypeQuery
                            | K::JSDocLink
                            | K::JSDocLinkCode
                            | K::JSDocLinkPlain
                            | K::JSDocNameReference
                    )
                ),
                None => false,
            };
            type_context || is_in_expression_context(view, node)?
        }
        Some(
            K::NumericLiteral
            | K::BigIntLiteral
            | K::StringLiteral
            | K::NoSubstitutionTemplateLiteral
            | K::ThisKeyword,
        ) => is_in_expression_context(view, node)?,
        _ => false,
    })
}

// port: tsc/internal/ast/utilities.go:IsPartOfTypeNode
fn part_of_type_node(view: ts_ast::AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    let kind = read.kind();
    // The kind range only; `ExpressionWithTypeArguments` is decided by its parent below.
    if kind.raw() >= K::TypePredicate as i16 && kind.raw() <= K::ImportType as i16 {
        return Ok(true);
    }
    Ok(match kind.known() {
        Some(
            K::AnyKeyword
            | K::UnknownKeyword
            | K::NumberKeyword
            | K::BigIntKeyword
            | K::StringKeyword
            | K::BooleanKeyword
            | K::SymbolKeyword
            | K::ObjectKeyword
            | K::UndefinedKeyword
            | K::NullKeyword
            | K::NeverKeyword,
        ) => true,
        Some(K::VoidKeyword) => match read.parent() {
            Some(parent) => view.node(parent)?.kind() != K::VoidExpression,
            None => true,
        },
        // port: tsc/internal/ast/utilities.go:isPartOfTypeExpressionWithTypeArguments
        Some(K::ExpressionWithTypeArguments) => match read.parent() {
            Some(parent) if view.node(parent)?.kind() == K::HeritageClause => {
                let heritage = view.node(parent)?;
                let class_extends = heritage.parent().is_some_and(|grand| {
                    view.node(grand)
                        .is_ok_and(|read| ts_ast::utilities::is_class_like(&read))
                }) && heritage
                    .data_source()
                    .as_heritage_clause()
                    .is_some_and(|data| data.token() != K::ImplementsKeyword);
                !class_extends
            }
            _ => false,
        },
        _ => false,
    })
}

// port: tsc/internal/ast/utilities.go:IsThisInTypeQuery
pub(crate) fn is_this_in_type_query(
    view: ts_ast::AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    let read = view.node(node)?;
    if read.kind() != K::Identifier || view.node_text(node)?.as_bytes() != b"this" {
        return Ok(false);
    }
    let mut current = node;
    loop {
        let Some(parent) = view.node(current)?.parent() else {
            return Ok(false);
        };
        let parent_read = view.node(parent)?;
        if parent_read.kind() == K::QualifiedName
            && parent_read
                .data_source()
                .as_qualified_name()
                .and_then(|data| data.left())
                == Some(current)
        {
            current = parent;
            continue;
        }
        return Ok(parent_read.kind() == K::TypeQuery);
    }
}
