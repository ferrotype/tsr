//! Enum values, member identity, and the declared enum union. Value computation
//! has an explicit failed state: a port boundary cannot leave a completed prefix.

use crate::{
    object_flags as of, type_flags as tf, CheckerState, Error, LinkStore, LiteralValue, NumberKey,
    TypeAlias, TypeId, UnionReduction,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, symbol_flags as sf, SyntaxKind as K};
use ts_diagnostics as messages;
use ts_jsnum::Number;
use ts_jsstring::JsString;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum EnumValue {
    Number(Number),
    String(JsString),
}

impl EnumValue {
    // port: tsc/internal/checker/utilities.go:ValueToString
    pub(crate) fn diagnostic_text(&self) -> JsString {
        match self {
            Self::Number(_) => self.text(),
            Self::String(text) => {
                let mut result = vec![b'"'];
                result.extend_from_slice(&ts_jsstring::escape::escape_string(
                    text.as_bytes(),
                    ts_jsstring::QuoteChar::Double,
                ));
                result.push(b'"');
                JsString::from_bytes(result)
            }
        }
    }

    pub(crate) fn text(&self) -> JsString {
        match self {
            Self::Number(number) => JsString::from_bytes(number.to_string().into_bytes()),
            Self::String(text) => text.clone(),
        }
    }
}

/// `evaluator.Result`; absence is a nonconstant result, not an evaluator failure.
#[derive(Clone, Debug, Default)]
pub(crate) struct EnumEvaluation {
    pub value: Option<EnumValue>,
    pub is_syntactically_string: bool,
    pub resolved_other_files: bool,
    pub has_external_references: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum EnumComputation {
    Computing,
    Complete,
    Failed(Error),
}

#[derive(Default)]
pub(crate) struct EnumState {
    pub values: LinkStore<NodeId, EnumEvaluation>,
    pub computed: LinkStore<NodeId, Option<EnumComputation>>,
    pub checked: crate::types::Set<SymbolId>,
    pub string_literals: crate::types::Map<(SymbolId, JsString), TypeId>,
    pub number_literals: crate::types::Map<(SymbolId, NumberKey), TypeId>,
    pub nan_literals: crate::types::Map<SymbolId, TypeId>,
    pub relations: crate::types::Map<(SymbolId, SymbolId), bool>,
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkEnumMember
    pub(crate) fn check_enum_member(&mut self, member: NodeId) -> Result<(), Error> {
        let read = self.ast(member)?.node(member)?;
        let name = read.name().ok_or(Error::MissingLink("enum member name"))?;
        let initializer = read.initializer();
        if self.ast(name)?.node(name)?.kind() == K::PrivateIdentifier {
            self.error_at(
                Some(member),
                messages::An_enum_member_cannot_be_named_with_a_private_identifier,
                vec![],
            )?;
        }
        if let Some(initializer) = initializer {
            self.check_expression(initializer)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkEnumDeclaration
    pub(crate) fn check_enum_declaration(&mut self, declaration: NodeId) -> Result<(), Error> {
        self.check_grammar_modifiers(declaration)?;
        self.check_collisions_for_declaration_name(declaration)?;
        self.check_exports_on_merged_declarations(declaration)?;
        for member in self.source_list(
            declaration,
            self.ast(declaration)?.node(declaration)?.member_list(),
        )? {
            self.check_source_element(member)?;
        }
        if self
            .program()?
            .host
            .options()
            .erasable_syntax_only
            .is_true()
            && self.ast(declaration)?.node(declaration)?.flags() & nf::AMBIENT == 0
        {
            self.error_at(
                Some(declaration),
                messages::This_syntax_is_not_allowed_when_erasableSyntaxOnly_is_enabled,
                vec![],
            )?;
        }
        self.compute_enum_member_values(declaration)?;
        let symbol = self
            .get_symbol_of_declaration(declaration)?
            .ok_or(Error::MissingLink("enum symbol"))?;
        if self.enums.checked.contains(&symbol) {
            return Ok(());
        }
        let declarations = self.symbol_declarations(symbol)?.to_vec();
        let is_const = ts_ast::utilities::is_enum_const(self.ast(declaration)?, declaration)?;
        let mut missing_initializer = false;
        for node in declarations.iter().copied().flatten() {
            let read = self.ast(node)?.node(node)?;
            if read.kind() != K::EnumDeclaration {
                continue;
            }
            let name = read.name();
            let members = self.source_list(node, read.member_list())?;
            if declarations.len() > 1
                && ts_ast::utilities::is_enum_const(self.ast(node)?, node)? != is_const
            {
                self.error_at(
                    name,
                    messages::Enum_declarations_must_all_be_const_or_non_const,
                    vec![],
                )?;
            }
            if let Some(&first) = members.first() {
                let read = self.ast(first)?.node(first)?;
                if read.initializer().is_none() {
                    if missing_initializer {
                        self.error_at(read.name(), messages::In_an_enum_with_multiple_declarations_only_one_declaration_can_omit_an_initializer_for_its_first_enum_element, vec![])?;
                    } else {
                        missing_initializer = true;
                    }
                }
            }
        }
        self.enums.checked.insert(symbol);
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getParentOfSymbol
    pub(crate) fn parent_of_symbol(&mut self, symbol: SymbolId) -> Result<Option<SymbolId>, Error> {
        let Some(parent) = self.symbol(symbol)?.parent() else {
            return Ok(None);
        };
        let parent = self.late_bound_symbol(parent)?;
        Ok(Some(self.get_merged_symbol(parent)))
    }

    // port: tsc/internal/checker/checker.go:Checker.getDeclaredTypeOfEnum
    pub(crate) fn declared_enum_type(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        if let Some(Some(ty)) = self.query.declared_types.try_get(symbol) {
            return Ok(*ty);
        }
        let mut members = Vec::new();
        let mut journal = Vec::new();
        let result = (|| {
            for declaration in self
                .symbol_declarations(symbol)?
                .to_vec()
                .into_iter()
                .flatten()
            {
                let read = self.ast(declaration)?.node(declaration)?;
                if read.kind() != K::EnumDeclaration {
                    continue;
                }
                for member in self.source_list(declaration, read.member_list())? {
                    if ts_ast::has_dynamic_name(self.ast(member)?, Some(member))? {
                        continue;
                    }
                    let member_symbol = self
                        .get_symbol_of_declaration(member)?
                        .ok_or(Error::MissingLink("enum member symbol"))?;
                    let value = self.enum_member_value(member)?.value;
                    let ty = match value {
                        Some(value) => self.enum_literal_type(value, symbol, member_symbol)?,
                        None => self.computed_enum_type(member_symbol)?,
                    };
                    let fresh = self.get_fresh_type_of_literal_type(ty)?;
                    let previous = *self.query.declared_types.get_or_default(member_symbol);
                    journal.push((member_symbol, previous));
                    *self.query.declared_types.get_or_default(member_symbol) = Some(fresh);
                    members.push(ty);
                }
            }
            let ty = if members.is_empty() {
                self.computed_enum_type(symbol)?
            } else {
                let alias = self.types.push_alias(TypeAlias {
                    symbol,
                    type_arguments: [].into(),
                })?;
                self.get_union_type_ex(&members, UnionReduction::Literal, Some(alias), None)?
            };
            if self.types.flags(ty)? & tf::UNION != 0 {
                self.types.get_mut(ty)?.flags |= tf::ENUM_LITERAL;
                self.types.get_mut(ty)?.symbol = Some(symbol);
            }
            *self.query.declared_types.get_or_default(symbol) = Some(ty);
            Ok(ty)
        })();
        if result.is_err() {
            for (member, previous) in journal {
                *self.query.declared_types.get_or_default(member) = previous;
            }
        }
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.getDeclaredTypeOfEnumMember
    pub(crate) fn declared_enum_member_type(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        if let Some(Some(ty)) = self.query.declared_types.try_get(symbol) {
            return Ok(*ty);
        }
        let parent = self
            .parent_of_symbol(symbol)?
            .ok_or(Error::MissingLink("enum member parent"))?;
        let enum_type = self.declared_enum_type(parent)?;
        Ok(*self
            .query
            .declared_types
            .get_or_default(symbol)
            .get_or_insert(enum_type))
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfEnumMember
    pub(crate) fn type_of_enum_member(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        if let Some(ty) = self
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.resolved_type)
        {
            return Ok(ty);
        }
        let ty = self.declared_enum_member_type(symbol)?;
        self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfFuncClassEnumModuleWorker
    pub(crate) fn type_of_enum(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        if let Some(ty) = self
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.resolved_type)
        {
            return Ok(ty);
        }
        let ty = self.new_object_type(of::ANONYMOUS, Some(symbol))?;
        self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.createComputedEnumType
    fn computed_enum_type(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        let regular = self.new_literal_type(tf::ENUM, LiteralValue::ComputedEnum, None)?;
        self.types.get_mut(regular)?.symbol = Some(symbol);
        self.get_fresh_type_of_literal_type(regular)?;
        Ok(regular)
    }

    // port: tsc/internal/checker/checker.go:Checker.getEnumLiteralType
    fn enum_literal_type(
        &mut self,
        value: EnumValue,
        owner: SymbolId,
        symbol: SymbolId,
    ) -> Result<TypeId, Error> {
        let cached = match &value {
            EnumValue::String(text) => self.enums.string_literals.get(&(owner, text.clone())),
            EnumValue::Number(number) => match NumberKey::new(*number) {
                Some(key) => self.enums.number_literals.get(&(owner, key)),
                None => self.enums.nan_literals.get(&owner),
            },
        };
        if let Some(&ty) = cached {
            return Ok(ty);
        }
        let (flag, literal) = match &value {
            EnumValue::String(text) => (tf::STRING_LITERAL, LiteralValue::String(text.clone())),
            EnumValue::Number(number) => (tf::NUMBER_LITERAL, LiteralValue::Number(*number)),
        };
        let ty = self.new_literal_type(tf::ENUM_LITERAL | flag, literal, None)?;
        self.types.get_mut(ty)?.symbol = Some(symbol);
        match value {
            EnumValue::String(text) => {
                self.enums.string_literals.insert((owner, text), ty);
            }
            EnumValue::Number(number) => match NumberKey::new(number) {
                Some(key) => {
                    self.enums.number_literals.insert((owner, key), ty);
                }
                None => {
                    self.enums.nan_literals.insert(owner, ty);
                }
            },
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getBaseTypeOfEnumLikeType
    pub(crate) fn base_type_of_enum_like(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let record = *self.types.get(ty)?;
        if record.flags & tf::ENUM_LIKE != 0 {
            let symbol = record
                .symbol
                .ok_or(Error::MissingLink("enum type symbol"))?;
            if self.symbol(symbol)?.flags() & sf::ENUM_MEMBER != 0 {
                let parent = self
                    .parent_of_symbol(symbol)?
                    .ok_or(Error::MissingLink("enum member parent"))?;
                return self.get_declared_type_of_symbol(parent);
            }
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getEnumMemberValue
    pub(crate) fn enum_member_value(&mut self, member: NodeId) -> Result<EnumEvaluation, Error> {
        let parent = self
            .ast(member)?
            .node(member)?
            .parent()
            .ok_or(Error::MissingLink("enum member parent"))?;
        self.compute_enum_member_values(parent)?;
        Ok(self.enums.values.get_or_default(member).clone())
    }

    // port: tsc/internal/checker/checker.go:Checker.computeEnumMemberValues
    pub(crate) fn compute_enum_member_values(&mut self, declaration: NodeId) -> Result<(), Error> {
        match self.enums.computed.try_get(declaration).copied().flatten() {
            Some(EnumComputation::Computing | EnumComputation::Complete) => return Ok(()),
            Some(EnumComputation::Failed(error)) => return Err(error),
            None => {}
        }
        *self.enums.computed.get_or_default(declaration) = Some(EnumComputation::Computing);
        let result = (|| {
            let mut auto = Some(Number::new(0.0));
            let mut previous = None;
            for member in self.source_list(
                declaration,
                self.ast(declaration)?.node(declaration)?.member_list(),
            )? {
                let result = self.compute_enum_member_value(member, auto, previous)?;
                auto = match &result.value {
                    Some(EnumValue::Number(value)) => Some(Number::new(value.value() + 1.0)),
                    _ => None,
                };
                *self.enums.values.get_or_default(member) = result;
                previous = Some(member);
            }
            Ok(())
        })();
        *self.enums.computed.get_or_default(declaration) = Some(match result {
            Ok(()) => EnumComputation::Complete,
            Err(error) => EnumComputation::Failed(error),
        });
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.computeEnumMemberValue
    fn compute_enum_member_value(
        &mut self,
        member: NodeId,
        auto: Option<Number>,
        previous: Option<NodeId>,
    ) -> Result<EnumEvaluation, Error> {
        let read = self.ast(member)?.node(member)?;
        let name = read.name().ok_or(Error::MissingLink("enum member name"))?;
        let parent = read
            .parent()
            .ok_or(Error::MissingLink("enum member parent"))?;
        let initializer = read.initializer();
        let name_read = self.ast(name)?.node(name)?;
        if name_read.kind() == K::ComputedPropertyName
            && ts_ast::has_dynamic_name(self.ast(member)?, Some(member))?
        {
            self.error_at(
                Some(name),
                messages::Computed_property_names_are_not_allowed_in_enums,
                vec![],
            )?;
        } else {
            let numeric = if name_read.kind() == K::BigIntLiteral {
                true
            } else {
                let text_node = if name_read.kind() == K::ComputedPropertyName {
                    name_read
                        .expression()
                        .ok_or(Error::MissingLink("computed enum member name"))?
                } else {
                    name
                };
                let text_read = self.ast(text_node)?.node_text(text_node)?;
                let text = text_read.as_bytes();
                !matches!(text, b"Infinity" | b"-Infinity" | b"NaN")
                    && ts_jsnum::from_string(text).to_string().as_bytes() == text
            };
            if numeric {
                self.error_at(
                    Some(name),
                    messages::An_enum_member_cannot_have_a_numeric_name,
                    vec![],
                )?;
            }
        }
        if initializer.is_some() {
            return self.compute_constant_enum_member_value(member);
        }
        let ambient = self.ast(parent)?.node(parent)?.flags() & nf::AMBIENT != 0;
        if ambient && !ts_ast::utilities::is_enum_const(self.ast(parent)?, parent)? {
            return Ok(EnumEvaluation::default());
        }
        let Some(auto) = auto else {
            self.error_at(
                Some(name),
                messages::Enum_member_must_have_initializer,
                vec![],
            )?;
            return Ok(EnumEvaluation::default());
        };
        if self.enum_isolated_modules() {
            if let Some(previous) = previous {
                if self.ast(previous)?.node(previous)?.initializer().is_some() {
                    let value = self.enum_member_value(previous)?;
                    if !matches!(value.value, Some(EnumValue::Number(_)))
                        || value.resolved_other_files
                    {
                        self.error_at(Some(name), messages::Enum_member_following_a_non_literal_numeric_member_must_have_an_initializer_when_isolatedModules_is_enabled, vec![])?;
                    }
                }
            }
        }
        Ok(EnumEvaluation {
            value: Some(EnumValue::Number(auto)),
            ..Default::default()
        })
    }

    pub(crate) fn enum_isolated_modules(&self) -> bool {
        self.program.as_ref().is_some_and(|program| {
            let options = program.host.options();
            options.isolated_modules.is_true() || options.verbatim_module_syntax.is_true()
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.computeConstantEnumMemberValue
    fn compute_constant_enum_member_value(
        &mut self,
        member: NodeId,
    ) -> Result<EnumEvaluation, Error> {
        let read = self.ast(member)?.node(member)?;
        let parent = read
            .parent()
            .ok_or(Error::MissingLink("enum member parent"))?;
        let initializer = read
            .initializer()
            .ok_or(Error::MissingLink("enum member initializer"))?;
        let is_const = ts_ast::utilities::is_enum_const(self.ast(parent)?, parent)?;
        let result = self.evaluate_enum_expression(initializer, Some(member))?;
        if let Some(value) = &result.value {
            if is_const {
                if let EnumValue::Number(value) = value {
                    if !value.value().is_finite() {
                        self.error_at(Some(initializer), if value.value().is_nan() {
                            messages::X_const_enum_member_initializer_was_evaluated_to_disallowed_value_NaN
                        } else { messages::X_const_enum_member_initializer_was_evaluated_to_a_non_finite_value }, vec![])?;
                    }
                }
            }
            if self.enum_isolated_modules()
                && matches!(value, EnumValue::String(_))
                && !result.is_syntactically_string
            {
                let parent_name = self
                    .ast(parent)?
                    .node(parent)?
                    .name()
                    .ok_or(Error::MissingLink("enum name"))?;
                let member_name = self
                    .ast(member)?
                    .node(member)?
                    .name()
                    .ok_or(Error::MissingLink("enum member name"))?;
                let mut name = self
                    .ast(parent_name)?
                    .node_text(parent_name)?
                    .as_bytes()
                    .to_vec();
                name.push(b'.');
                name.extend_from_slice(self.ast(member_name)?.node_text(member_name)?.as_bytes());
                self.error_at(Some(initializer), messages::X_0_has_a_string_type_but_must_have_syntactically_recognizable_string_syntax_when_isolatedModules_is_enabled, vec![JsString::from_bytes(name)])?;
            }
        } else if is_const {
            self.error_at(
                Some(initializer),
                messages::X_const_enum_member_initializers_must_be_constant_expressions,
                vec![],
            )?;
        } else if self.ast(parent)?.node(parent)?.flags() & nf::AMBIENT != 0 {
            self.error_at(Some(initializer), messages::In_ambient_enum_declarations_member_initializer_must_be_constant_expression, vec![])?;
        } else {
            let ty = self.check_expression(initializer)?;
            let (_, diagnostic) = self.check_type_related_ex(ty, self.builtins.number_type, crate::RelationKind::Assignable, Some(initializer), Some(messages::Type_0_is_not_assignable_to_type_1_as_required_for_computed_enum_member_values))?;
            if let Some(diagnostic) = diagnostic {
                self.add_diagnostic(diagnostic)?;
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveAnonymousTypeMembers
    pub(crate) fn resolve_enum_members(
        &mut self,
        ty: TypeId,
        symbol: SymbolId,
    ) -> Result<(), Error> {
        let members = self.symbol(symbol)?.exports();
        self.set_structured_type_members(ty, members, &[], &[], &[])?;
        let index = self.member_symbol(members, ts_ast::internal_symbol_names::INDEX)?;
        let indexes = if let Some(index) = index {
            self.index_infos_of_symbol(Some(index), members)?
        } else {
            let declared = self.declared_enum_type(symbol)?;
            let mut numeric = self.types.flags(declared)? & tf::ENUM != 0;
            if !numeric {
                let properties = self
                    .types
                    .structured(ty)?
                    .properties
                    .clone()
                    .unwrap_or_default();
                for &property in properties.iter() {
                    let property_type = self.get_type_of_symbol(property)?;
                    if self.types.flags(property_type)? & tf::NUMBER_LIKE != 0 {
                        numeric = true;
                        break;
                    }
                }
            }
            if numeric {
                vec![self.builtins.enum_number_index_info]
            } else {
                vec![]
            }
        };
        self.set_structured_type_members(ty, members, &[], &[], &indexes)
    }

    // port: tsc/internal/checker/relater.go:Checker.isEnumTypeRelatedTo
    /// Boolean relation path. Diagnostic callers use `enum_relation_mismatch`
    /// to repeat a failed comparison and construct its native message chain.
    pub(crate) fn enum_types_related(
        &mut self,
        source: SymbolId,
        target: SymbolId,
    ) -> Result<bool, Error> {
        let source = self.enum_owner_symbol(source)?;
        let target = self.enum_owner_symbol(target)?;
        if source == target {
            return Ok(true);
        }
        if self.symbol(source)?.name_bytes() != self.symbol(target)?.name_bytes()
            || self.symbol(source)?.flags() & sf::REGULAR_ENUM == 0
            || self.symbol(target)?.flags() & sf::REGULAR_ENUM == 0
        {
            return Ok(false);
        }
        if let Some(&related) = self.enums.relations.get(&(source, target)) {
            return Ok(related);
        }
        let related = self.enum_relation_mismatch(source, target)?.is_none();
        self.enums.relations.insert((source, target), related);
        Ok(related)
    }

    pub(crate) fn enum_owner_symbol(&mut self, symbol: SymbolId) -> Result<SymbolId, Error> {
        if self.symbol(symbol)?.flags() & sf::ENUM_MEMBER != 0 {
            self.parent_of_symbol(symbol)?
                .ok_or(Error::MissingLink("enum member parent"))
        } else {
            Ok(symbol)
        }
    }

    pub(crate) fn enum_relation_mismatch(
        &mut self,
        source: SymbolId,
        target: SymbolId,
    ) -> Result<Option<EnumRelationMismatch>, Error> {
        let target_type = self.get_type_of_symbol(target)?;
        let source_type = self.get_type_of_symbol(source)?;
        self.resolve_type_members(source_type)?;
        let properties = self
            .types
            .structured(source_type)?
            .properties
            .clone()
            .unwrap_or_default();
        for &source_property in properties.iter() {
            if self.symbol(source_property)?.flags() & sf::ENUM_MEMBER == 0 {
                continue;
            }
            let name = self.symbol(source_property)?.name_to_owned();
            let target_property = self.constituent_property(target_type, name.as_bytes(), false)?;
            let missing = match target_property {
                Some(property) => self.symbol(property)?.flags() & sf::ENUM_MEMBER == 0,
                None => true,
            };
            if missing {
                return Ok(Some(EnumRelationMismatch::Missing {
                    property: source_property,
                    target,
                }));
            }
            let target_property =
                target_property.ok_or(Error::MissingLink("target enum property"))?;
            let source_declaration = self.enum_member_declaration(source_property)?;
            let target_declaration = self.enum_member_declaration(target_property)?;
            let source_value = self.enum_member_value(source_declaration)?.value;
            let target_value = self.enum_member_value(target_declaration)?.value;
            if source_value != target_value {
                if let (Some(source_value), Some(target_value)) = (&source_value, &target_value) {
                    return Ok(Some(EnumRelationMismatch::Different {
                        target,
                        property: target_property,
                        expected: target_value.clone(),
                        actual: source_value.clone(),
                    }));
                }
                let string = match (source_value, target_value) {
                    (Some(EnumValue::String(value)), _) | (_, Some(EnumValue::String(value))) => {
                        Some(value)
                    }
                    _ => None,
                };
                if let Some(value) = string {
                    return Ok(Some(EnumRelationMismatch::StringAndUnknown {
                        target,
                        property: target_property,
                        value,
                    }));
                }
            }
        }
        Ok(None)
    }

    fn enum_member_declaration(&self, symbol: SymbolId) -> Result<NodeId, Error> {
        for node in self.symbol_declarations(symbol)?.iter().flatten() {
            if self.ast(node)?.node(node)?.kind() == K::EnumMember {
                return Ok(node);
            }
        }
        Err(Error::MissingLink("enum member declaration"))
    }
}

/// Retains nonowning identities and primitive values for a native relation
/// diagnostic; formatting stays with the relater's speculative error state.
pub(crate) enum EnumRelationMismatch {
    Missing {
        property: SymbolId,
        target: SymbolId,
    },
    Different {
        target: SymbolId,
        property: SymbolId,
        expected: EnumValue,
        actual: EnumValue,
    },
    StringAndUnknown {
        target: SymbolId,
        property: SymbolId,
        value: JsString,
    },
}

#[cfg(any(test, feature = "storage-pilot"))]
impl CheckerState {
    pub(crate) fn census_enums(&self, census: &mut crate::census::Census) {
        census.add(
            "enum_links",
            self.enums.values.len(),
            self.enums.values.structural_bytes(),
        );
        census.add(
            "enum_links",
            self.enums.computed.len(),
            self.enums.computed.structural_bytes(),
        );
        census.add(
            "enum_links",
            self.enums.checked.len(),
            self.enums.checked.allocation_size(),
        );
        census.map("literal", &self.enums.string_literals);
        census.map("literal", &self.enums.number_literals);
        census.map("literal", &self.enums.nan_literals);
        census.map("enum_relations", &self.enums.relations);
        for (_, text) in self.enums.string_literals.keys() {
            census.text("literal", text);
        }
        for value in self.enums.values.values() {
            if let Some(EnumValue::String(text)) = &value.value {
                census.text("enum_links", text);
            }
        }
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:isConstEnumObjectType
    pub(crate) fn const_enum_object_type(&self, ty: TypeId) -> Result<bool, Error> {
        let read = self.types.get(ty)?;
        if read.object_flags & of::ANONYMOUS == 0 {
            return Ok(false);
        }
        match read.symbol {
            Some(symbol) => Ok(self.symbol(symbol)?.flags() & sf::CONST_ENUM != 0),
            None => Ok(false),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkConstEnumAccess
    pub(crate) fn check_const_enum_access(
        &mut self,
        node: NodeId,
        ty: TypeId,
    ) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let parent = read
            .parent()
            .ok_or(Error::MissingLink("const enum use parent"))?;
        let parent_read = self.ast(parent)?.node(parent)?;
        let kind = read.kind();
        let allowed = matches!(
            parent_read.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) && parent_read.expression() == Some(node)
            || matches!(kind.known(), Some(K::Identifier | K::QualifiedName))
                && self.enum_import_or_export_assignment(node)?
            || parent_read.kind() == K::TypeQuery
                && parent_read
                    .data_source()
                    .as_type_query_node()
                    .and_then(|data| data.expr_name())
                    == Some(node)
            || parent_read.kind() == K::ExportSpecifier;
        if !allowed {
            self.error_at(Some(node),messages::X_const_enums_can_only_be_used_in_property_or_index_access_expressions_or_the_right_hand_side_of_an_import_declaration_or_export_assignment_or_type_query,vec![])?;
        }
        let options = self.program()?.host.options();
        let isolated = options.isolated_modules.is_true();
        let verbatim = options.verbatim_module_syntax.is_true();
        let mut check = isolated;
        if !check && verbatim && allowed {
            let first = ts_ast::utilities_middle::get_first_identifier(self.ast(node)?, node)?;
            let name = self.ast(first)?.node_text(first)?.into_js_string();
            check = self
                .resolve_name_ex(Some(node), name.as_bytes(), sf::ALIAS, None, false, true)?
                .is_none();
        }
        if check {
            let symbol = self
                .types
                .get(ty)?
                .symbol
                .ok_or(Error::MissingLink("const enum object symbol"))?;
            let declaration = self
                .symbol(symbol)?
                .value_declaration()
                .ok_or(Error::MissingLink("const enum declaration"))?;
            let source = ts_ast::utilities::get_source_file_of_node(
                self.ast(declaration)?,
                Some(declaration),
            )?
            .ok_or(Error::MissingLink("const enum declaration source"))?;
            let file = self.ast(source)?.source_file(source)?;
            let redirect = self
                .program()?
                .host
                .get_project_reference_from_output_dts(file.path())?;
            let preserved =
                redirect.is_some_and(|redirect| redirect.options.should_preserve_const_enums());
            if self.ast(declaration)?.node(declaration)?.flags() & nf::AMBIENT != 0
                && !self.valid_type_only_alias_use_site(node)?
                && !preserved
            {
                let flag = JsString::from_bytes(if verbatim {
                    b"verbatimModuleSyntax".as_slice()
                } else {
                    b"isolatedModules".as_slice()
                });
                self.error_at(
                    Some(node),
                    messages::Cannot_access_ambient_const_enums_when_0_is_enabled,
                    vec![flag],
                )?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/utilities.go:isInRightSideOfImportOrExportAssignment
    fn enum_import_or_export_assignment(&self, mut node: NodeId) -> Result<bool, Error> {
        while let Some(parent) = self.ast(node)?.node(node)?.parent() {
            if self.ast(parent)?.node(parent)?.kind() != K::QualifiedName {
                break;
            }
            node = parent;
        }
        let Some(parent) = self.ast(node)?.node(node)?.parent() else {
            return Ok(false);
        };
        let read = self.ast(parent)?.node(parent)?;
        Ok(match read.kind().known() {
            Some(K::ImportEqualsDeclaration) => {
                read.data_source()
                    .as_import_equals_declaration()
                    .and_then(|data| data.module_reference())
                    == Some(node)
            }
            Some(K::ExportAssignment) => read.expression() == Some(node),
            _ => false,
        })
    }
}
