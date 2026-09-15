//! The executable type-display slice of checker/nodebuilderimpl.go. Each builder
//! owns its synthetic syntax and emit flags until printing finishes.

#[path = "node_builder_cache.rs"]
pub(crate) mod cache;
#[path = "node_builder_class_emit.rs"]
mod class_emit;
#[path = "node_builder_enum.rs"]
mod enums;
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, LiteralValue, TypeId};
use ts_arena::SymbolId;
use ts_ast::{
    check_flags, symbol_flags as sf, token_flags, AstBuilder, Factory, FactoryMethods, JsString,
    NodeId, NodeListId, SyntaxKind as K,
};
use ts_core::{LanguageVariant, TextRange};
use ts_jsstring::SourceText;
use ts_nodebuilder::flags as nf;
use ts_printer::{emit_flags, EmitContext};

#[path = "accessibility.rs"]
mod accessibility;
#[path = "node_builder_emit.rs"]
mod declaration_emit;
#[path = "node_builder_extra.rs"]
mod extra;
#[path = "node_builder_names.rs"]
mod names;
#[path = "node_builder_pseudo.rs"]
mod pseudo;
#[path = "node_builder_pseudo_output.rs"]
mod pseudo_output;
#[path = "node_builder_reuse.rs"]
mod reuse;
#[path = "node_builder_scopes.rs"]
mod scopes;
#[path = "node_builder_serialize.rs"]
mod serialize;

pub(crate) struct NodeBuilder<'a> {
    pub(crate) checker: &'a mut CheckerState,
    pub(crate) ast: AstBuilder,
    pub(crate) emit: EmitContext,
    pub(crate) flags: ts_nodebuilder::Flags,
    pub(crate) enclosing: Option<NodeId>,
    pub(crate) mapper: Option<crate::MapperId>,
    pub(crate) suppress_inference_fallback: bool,
    pub(crate) encountered_error: bool,
    pub(crate) enclosing_symbol_types: crate::types::Map<SymbolId, TypeId>,
    pub(crate) id_to_symbol: crate::types::Map<NodeId, Option<SymbolId>>,
    type_parameter_names: scopes::TypeParameterNames,
    reuse_boundaries: Vec<reuse::RecoveryBoundary>,
    internal_flags: ts_nodebuilder::InternalFlags,
    tracker: Option<&'a mut dyn ts_printer::emit_resolver::DeclarationSymbolTracker>,
    pub(crate) approximate_length: usize,
    truncating: bool,
    visited: Vec<TypeId>,
    symbol_depth: Vec<class_emit::SymbolIdentity>,
    infer_parameters: crate::TypeList,
    reverse_mapped_stack: Vec<SymbolId>,
    name_access: names::NameAccess,
    specifiers: crate::types::Map<cache::SpecifierKey, JsString>,
    serialized: crate::types::Map<cache::SerializedKey, cache::SerializedType>,
    tracked_symbols: Vec<cache::TrackedSymbol>,
    reported_diagnostic: bool,
    cached: bool,
}

impl<'a> NodeBuilder<'a> {
    /// Begin an independent public request on this builder. Generated syntax
    /// and emit metadata survive; resolution and formatting state do not.
    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.enterContext
    pub(crate) fn prepare_context(
        &mut self,
        enclosing: Option<NodeId>,
        flags: ts_nodebuilder::Flags,
        internal_flags: ts_nodebuilder::InternalFlags,
    ) -> Result<(), Error> {
        if let Some(node) = enclosing {
            self.retain_source_node(node)?;
        }
        self.enclosing = enclosing;
        self.flags = flags;
        self.internal_flags = internal_flags;
        self.mapper = None;
        self.suppress_inference_fallback = false;
        self.encountered_error = false;
        self.enclosing_symbol_types.clear();
        self.type_parameter_names = scopes::TypeParameterNames::default();
        self.reuse_boundaries.clear();
        self.approximate_length = 0;
        self.truncating = false;
        self.visited.clear();
        self.symbol_depth.clear();
        self.infer_parameters = [].into();
        self.reverse_mapped_stack.clear();
        self.name_access = names::NameAccess::default();
        self.tracked_symbols.clear();
        self.reported_diagnostic = false;
        Ok(())
    }

    pub(crate) fn new(checker: &'a mut CheckerState, flags: ts_nodebuilder::Flags) -> Self {
        Self::with_emit(checker, flags, EmitContext::new())
    }

    fn with_emit(
        checker: &'a mut CheckerState,
        flags: ts_nodebuilder::Flags,
        emit: EmitContext,
    ) -> Self {
        let ast = AstBuilder::with_hooks(
            SourceText::from_bytes(b"".as_slice()),
            &checker.counters,
            emit.factory_hooks(),
        );
        Self {
            checker,
            ast,
            emit,
            flags,
            enclosing: None,
            mapper: None,
            suppress_inference_fallback: false,
            encountered_error: false,
            enclosing_symbol_types: crate::types::Map::default(),
            id_to_symbol: crate::types::Map::default(),
            type_parameter_names: scopes::TypeParameterNames::default(),
            reuse_boundaries: Vec::new(),
            internal_flags: 0,
            tracker: None,
            approximate_length: 0,
            truncating: false,
            visited: Vec::new(),
            symbol_depth: Vec::new(),
            infer_parameters: [].into(),
            reverse_mapped_stack: Vec::new(),
            name_access: names::NameAccess::default(),
            specifiers: crate::types::Map::default(),
            serialized: crate::types::Map::default(),
            tracked_symbols: Vec::new(),
            reported_diagnostic: false,
            cached: false,
        }
    }

    /// Transfer the caller's syntax factory into the request and back on both
    /// success and a returned error. Returned ids remain in the caller's owner.
    /// A panic retires the enclosing checker operation before any reuse.
    #[allow(
        clippy::too_many_arguments,
        reason = "Native serialization has separate output, context, flags and tracker inputs"
    )]
    pub(crate) fn with_output<T>(
        checker: &'a mut CheckerState,
        output: &mut AstBuilder,
        emit: &mut EmitContext,
        enclosing: NodeId,
        flags: ts_nodebuilder::Flags,
        internal_flags: ts_nodebuilder::InternalFlags,
        tracker: &'a mut dyn ts_printer::emit_resolver::DeclarationSymbolTracker,
        action: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut builder = Self::new(checker, flags);
        std::mem::swap(&mut builder.ast, output);
        std::mem::swap(&mut builder.emit, emit);
        builder.enclosing = Some(enclosing);
        builder.internal_flags = internal_flags;
        builder.tracker = Some(tracker);
        let result = action(&mut builder);
        if builder.truncating && builder.flags & nf::NO_TRUNCATION != 0 {
            builder.report(ts_printer::emit_resolver::DeclarationTrackerEvent::Truncation);
        }
        std::mem::swap(&mut builder.ast, output);
        std::mem::swap(&mut builder.emit, emit);
        result
    }

    fn report(&mut self, event: ts_printer::emit_resolver::DeclarationTrackerEvent) {
        if self.defer_reuse_report(&event) {
            return;
        }
        use ts_printer::emit_resolver::DeclarationTrackerEvent as Event;
        if !matches!(
            event,
            Event::InferenceFallback(_)
                | Event::PushErrorFallbackNode(_)
                | Event::PopErrorFallbackNode
        ) {
            self.reported_diagnostic = true;
        }
        if let Some(tracker) = self.tracker.as_deref_mut() {
            tracker.report(event);
        }
    }

    fn track_symbol(&mut self, symbol: SymbolId, meaning: u32) -> Result<bool, Error> {
        if self.defer_reuse_symbol(symbol, self.enclosing, meaning) {
            return Ok(false);
        }
        let mut reported = false;
        if let Some(tracker) = self.tracker.as_deref_mut() {
            if tracker.track_symbol_without_accessibility(symbol) {
                return Ok(false);
            }
            let accessibility = self.checker.emit_symbol_accessible(
                Some(symbol),
                self.enclosing,
                meaning,
                true,
                true,
            )?;
            reported = self
                .tracker
                .as_deref_mut()
                .expect("tracker was checked")
                .track_symbol(symbol, self.enclosing, meaning, accessibility);
        }
        if reported {
            self.reported_diagnostic = true;
        } else if self.checker.symbol(symbol)?.flags() & sf::TYPE_PARAMETER == 0 {
            self.tracked_symbols.push(cache::TrackedSymbol {
                symbol,
                enclosing: self.enclosing,
                meaning,
            });
        }
        Ok(reported)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.getNameOfSymbolAsWritten
    fn symbol_name(&self, symbol: SymbolId) -> Result<JsString, Error> {
        let read = self.checker.symbol(symbol)?;
        let declarations = self.checker.symbol_declarations(symbol)?;
        if read.name_bytes() == ts_ast::internal_symbol_names::DEFAULT
            && self.flags & nf::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE == 0
        {
            let external =
                if self.flags & nf::IN_INITIAL_ENTITY_NAME == 0 || declarations.is_empty() {
                    true
                } else if let Some(enclosing) = self.enclosing {
                    self.default_binding_context(declarations.first().flatten())?
                        != self.default_binding_context(Some(enclosing))?
                } else {
                    false
                };
            if external {
                return Ok(JsString::from_bytes(b"default".as_slice()));
            }
        }
        for declaration in declarations.iter().flatten() {
            let view = self.checker.ast(declaration)?;
            if let Some(name) = ts_ast::get_name_of_declaration(view, Some(declaration))? {
                if view.node(name)?.kind() == K::ComputedPropertyName
                    && read.check_flags() & check_flags::LATE == 0
                {
                    if let Some(name_type) = self
                        .checker
                        .value_symbol_links
                        .try_get(symbol)
                        .and_then(|links| links.name_type)
                    {
                        if self.checker.types.flags(name_type)?
                            & (tf::STRING_LITERAL | tf::NUMBER_LITERAL)
                            != 0
                        {
                            if let Some(name) = self.symbol_name_from_name_type(symbol)? {
                                return Ok(name);
                            }
                        }
                    }
                }
                return Ok(ts_scanner::declaration_name_to_string(view, Some(name))?);
            }
        }
        if let Some(declaration) = declarations.first().flatten() {
            let view = self.checker.ast(declaration)?;
            let node = view.node(declaration)?;
            if let Some(parent) = node.parent() {
                if view.node(parent)?.kind() == K::VariableDeclaration {
                    return Ok(ts_scanner::declaration_name_to_string(
                        view,
                        view.node(parent)?.name(),
                    )?);
                }
            }
            match node.kind().known() {
                Some(K::ClassExpression) => {
                    return Ok(JsString::from_bytes(b"(Anonymous class)".as_slice()))
                }
                Some(K::FunctionExpression | K::ArrowFunction) => {
                    return Ok(JsString::from_bytes(b"(Anonymous function)".as_slice()))
                }
                _ => {}
            }
        }
        if let Some(name) = self.symbol_name_from_name_type(symbol)? {
            return Ok(name);
        }
        Ok(JsString::from_bytes(
            ts_ast::escape_internal_symbol_name(read.name_bytes()).into_owned(),
        ))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:isDefaultBindingContext
    fn default_binding_context(&self, mut node: Option<NodeId>) -> Result<Option<NodeId>, Error> {
        while let Some(id) = node {
            let view = self.checker.ast(id)?;
            let read = view.node(id)?;
            if read.kind() == K::SourceFile || ts_ast::is_ambient_module(view, id)? {
                return Ok(node);
            }
            node = read.parent();
        }
        Ok(None)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.getNameOfSymbolFromNameType
    fn symbol_name_from_name_type(&self, symbol: SymbolId) -> Result<Option<JsString>, Error> {
        let Some(name_type) = self
            .checker
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.name_type)
        else {
            return Ok(None);
        };
        let flags = self.checker.types.flags(name_type)?;
        if flags & (tf::STRING_LITERAL | tf::NUMBER_LITERAL) != 0 {
            let value = match &self.checker.types.literal(name_type)?.value {
                LiteralValue::String(text) => crate::enums::EnumValue::String(text.clone()),
                LiteralValue::Number(value) => crate::enums::EnumValue::Number(*value),
                _ => return Err(Error::MissingLink("symbol literal name value")),
            };
            let name = value.text();
            let numeric = ts_jsnum::from_string(name.as_bytes())
                .to_string()
                .as_bytes()
                == name.as_bytes();
            if !numeric
                && !ts_scanner::is_identifier_text(name.as_bytes(), LanguageVariant::STANDARD)
            {
                return Ok(Some(value.diagnostic_text()));
            }
            if numeric && name.as_bytes().starts_with(b"-") {
                let mut text = vec![b'['];
                text.extend_from_slice(name.as_bytes());
                text.push(b']');
                return Ok(Some(JsString::from_bytes(text)));
            }
            return Ok((!name.is_empty()).then_some(name));
        }
        if flags & tf::UNIQUE_ES_SYMBOL != 0 {
            let target = self
                .checker
                .types
                .get(name_type)?
                .symbol
                .ok_or(Error::MissingLink("symbol name unique symbol"))?;
            let name = self.symbol_name(target)?;
            let mut text = vec![b'['];
            text.extend_from_slice(name.as_bytes());
            text.push(b']');
            return Ok(Some(JsString::from_bytes(text)));
        }
        Ok(None)
    }

    /// The entity name of a type symbol: an identifier or qualified name built
    /// over the accessible symbol chain. With no enclosing declaration or
    /// UseFullyQualifiedType, upstream's lookupSymbolChain returns the symbol
    /// itself, regardless of its parent.
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.symbolToTypeNode
    pub(crate) fn symbol_node(&mut self, symbol: SymbolId) -> Result<NodeId, Error> {
        self.track_symbol(symbol, sf::TYPE)?;
        let chain = self.type_symbol_chain(symbol, sf::TYPE)?;
        self.access_from_symbol_chain(&chain, chain.len() - 1, 0, None)
    }

    fn list(&mut self, nodes: Vec<NodeId>) -> Result<NodeListId, Error> {
        let nodes = self.ast.node_slice(nodes.into_iter().map(Some).collect())?;
        Ok(self.ast.new_list(TextRange::new(-1, -1), nodes)?)
    }

    fn optional_node_list(&mut self, nodes: Vec<Option<NodeId>>) -> Result<NodeListId, Error> {
        let nodes = self.ast.node_slice(nodes)?;
        Ok(self.ast.new_list(TextRange::new(-1, -1), nodes)?)
    }

    fn keyword(&mut self, kind: K, length: usize) -> NodeId {
        self.approximate_length += length;
        self.ast.new_keyword_type_node(kind.into())
    }

    fn string_literal(&mut self, text: JsString) -> NodeId {
        let flags = if self.flags & nf::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE != 0 {
            token_flags::SINGLE_QUOTE
        } else {
            0
        };
        self.ast.new_string_literal(text, flags)
    }

    fn type_reference(&mut self, symbol: SymbolId, arguments: &[TypeId]) -> Result<NodeId, Error> {
        let arguments = if arguments.is_empty() {
            None
        } else {
            Some(self.type_list(arguments, false)?)
        };
        self.track_symbol(symbol, sf::TYPE)?;
        self.symbol_type_node_from_chain(symbol, sf::TYPE, arguments)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.checkTruncationLength
    fn check_truncation(&mut self) -> bool {
        let limit = if self.flags & nf::NO_TRUNCATION != 0 {
            crate::NO_TRUNCATION_MAXIMUM_TRUNCATION_LENGTH
        } else {
            crate::DEFAULT_MAXIMUM_TRUNCATION_LENGTH
        };
        self.truncating |= self.approximate_length > limit;
        self.truncating
    }

    fn elision(&mut self, text: &[u8]) -> Result<NodeId, Error> {
        if self.flags & nf::NO_TRUNCATION != 0 {
            return Err(Error::Unsupported(
                "node builder synthetic elision comments",
            ));
        }
        let name = self.ast.new_identifier(JsString::from_bytes(text));
        Ok(self.ast.new_type_reference_node(Some(name), None))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.mapToTypeNodes
    fn type_list(&mut self, types: &[TypeId], bare: bool) -> Result<NodeListId, Error> {
        if self.check_truncation() {
            if !bare {
                let node = self.elision(b"...")?;
                return self.list(vec![node]);
            }
            if types.len() > 2 {
                let first = self.type_node(types[0])?;
                let last = self.type_node(types[types.len() - 1])?;
                let elision =
                    self.elision(format!("... {} more ...", types.len() - 2).as_bytes())?;
                return self.list(vec![first, elision, last]);
            }
        }
        let nodes = self.type_nodes(types)?;
        self.list(nodes)
    }

    /// The element loop of mapToTypeNodes, shared with tuple element lists.
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.mapToTypeNodes
    pub(super) fn type_nodes(&mut self, types: &[TypeId]) -> Result<Vec<NodeId>, Error> {
        let mut nodes = Vec::new();
        // To avoid printing types like `[Foo, Foo]` or `Bar & Bar` where occurrences
        // of the same name come from different namespaces, single-identifier
        // references are regenerated fully qualified when a name is not homogeneous.
        let may_have_name_collisions = self.flags & nf::USE_FULLY_QUALIFIED_TYPE == 0;
        let mut seen_names: Vec<(JsString, TypeId, usize)> = Vec::new();
        for (index, &ty) in types.iter().enumerate() {
            let display_index = index + 1;
            if self.check_truncation() && display_index + 2 < types.len().saturating_sub(1) {
                nodes.push(
                    self.elision(
                        format!("... {} more ...", types.len() - display_index).as_bytes(),
                    )?,
                );
                nodes.push(self.type_node(types[types.len() - 1])?);
                break;
            }
            self.approximate_length += 2;
            let node = self.type_node(ty)?;
            let view = self.ast.view();
            if may_have_name_collisions {
                if let Some(reference) = view.node(node)?.data_source().as_type_reference_node() {
                    if let Some(name) = reference.type_name() {
                        if view.node(name)?.kind() == K::Identifier {
                            let name = JsString::from_bytes(view.node_text(name)?.as_bytes());
                            seen_names.push((name, ty, nodes.len()));
                        }
                    }
                }
            }
            nodes.push(node);
        }
        if may_have_name_collisions && !seen_names.is_empty() {
            let saved = self.flags;
            self.flags |= nf::USE_FULLY_QUALIFIED_TYPE;
            let result: Result<(), Error> = (|| {
                let mut names: Vec<JsString> = Vec::new();
                for (name, _, _) in &seen_names {
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
                }
                for name in names {
                    let group: Vec<(TypeId, usize)> = seen_names
                        .iter()
                        .filter(|(seen, _, _)| *seen == name)
                        .map(|(_, ty, index)| (*ty, *index))
                        .collect();
                    let first = group[0].0;
                    let mut homogeneous = true;
                    for &(ty, _) in &group[1..] {
                        if !self.types_are_same_reference(first, ty)? {
                            homogeneous = false;
                            break;
                        }
                    }
                    if !homogeneous {
                        for (ty, index) in group {
                            nodes[index] = self.type_node(ty)?;
                        }
                    }
                }
                Ok(())
            })();
            self.flags = saved;
            result?;
        }
        Ok(nodes)
    }

    /// Raw symbol parents, without the accessibility/alias selection of symbolToName.
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.symbolToEntityNameNode
    fn raw_symbol_entity_name(&mut self, symbol: SymbolId) -> Result<NodeId, Error> {
        let mut identifiers = Vec::new();
        let mut current = Some(symbol);
        while let Some(symbol) = current {
            let record = self.checker.symbol(symbol)?;
            let name = record.name_to_owned();
            current = record.parent();
            let identifier = self.ast.new_identifier(name);
            self.id_to_symbol.insert(identifier, Some(symbol));
            identifiers.push(identifier);
        }
        let mut name = identifiers.pop().expect("initial symbol was present");
        while let Some(right) = identifiers.pop() {
            name = self.ast.new_qualified_name(Some(name), Some(right));
        }
        Ok(name)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeToTypeNode
    pub(crate) fn type_node(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || self.type_node_worker(ty))
    }

    fn type_node_worker(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        let in_alias = self.flags & nf::IN_TYPE_ALIAS != 0;
        self.flags &= !nf::IN_TYPE_ALIAS;
        let ty = if self.flags & nf::NO_TYPE_REDUCTION == 0 {
            self.checker.get_reduced_type(ty)?
        } else {
            ty
        };
        let record = *self.checker.types.get(ty)?;
        if record.flags & tf::ANY != 0 {
            if let Some(alias) = self.checker.types.alias_of(ty)?.cloned() {
                // TypeAlias.ToTypeReferenceNode uses raw entity-name symbols,
                // separately from the ordinary alias accessibility path below.
                let name = self.raw_symbol_entity_name(alias.symbol)?;
                let arguments = if alias.type_arguments.is_empty() {
                    None
                } else {
                    Some(self.type_list(&alias.type_arguments, false)?)
                };
                return Ok(self.ast.new_type_reference_node(Some(name), arguments));
            }
            if ty == self.checker.builtins.unresolved_type {
                let node = self.ast.new_keyword_type_node(K::AnyKeyword.into());
                return Ok(self.emit.add_synthetic_leading_comment(
                    node,
                    K::MultiLineCommentTrivia,
                    JsString::from_bytes(b"unresolved".as_slice()),
                    false,
                ));
            }
            return Ok(self.keyword(
                if ty == self.checker.builtins.intrinsic_marker_type {
                    K::IntrinsicKeyword
                } else {
                    K::AnyKeyword
                },
                3,
            ));
        }
        for (flag, kind, length) in [
            (tf::UNKNOWN, K::UnknownKeyword, 0),
            (tf::STRING, K::StringKeyword, 6),
            (tf::NUMBER, K::NumberKeyword, 6),
            (tf::BIG_INT, K::BigIntKeyword, 6),
        ] {
            if record.flags & flag != 0 {
                return Ok(self.keyword(kind, length));
            }
        }
        if record.flags & tf::BOOLEAN != 0 && record.alias.is_none() {
            return Ok(self.keyword(K::BooleanKeyword, 7));
        }
        if record.flags & tf::ENUM_LIKE != 0 {
            return self
                .enum_type_node(ty, false)?
                .ok_or(Error::MissingLink("enum display"));
        }
        if record.flags & tf::LITERAL != 0 {
            let value = self.checker.types.literal(ty)?.value.clone();
            let literal = match value {
                LiteralValue::String(text) => {
                    self.approximate_length += text.len() + 2;
                    let literal = self.string_literal(text);
                    self.emit
                        .add_emit_flags(literal, emit_flags::NO_ASCII_ESCAPING);
                    literal
                }
                LiteralValue::Number(value) => {
                    let text = value.to_string();
                    self.approximate_length += text.len();
                    if value.value() < 0.0 {
                        let operand = self
                            .ast
                            .new_numeric_literal(JsString::from_bytes(&text.as_bytes()[1..]), 0);
                        self.ast
                            .new_prefix_unary_expression(K::MinusToken.into(), Some(operand))
                    } else {
                        self.ast
                            .new_numeric_literal(JsString::from_bytes(text.into_bytes()), 0)
                    }
                }
                LiteralValue::BigInt(value) => {
                    let mut text = value.to_text();
                    text.push(b'n');
                    self.approximate_length += text.len();
                    self.ast.new_big_int_literal(JsString::from_bytes(text), 0)
                }
                LiteralValue::Boolean(value) => {
                    self.approximate_length += if value { 4 } else { 5 };
                    self.ast.new_keyword_expression(
                        if value {
                            K::TrueKeyword
                        } else {
                            K::FalseKeyword
                        }
                        .into(),
                    )
                }
                LiteralValue::ComputedEnum => {
                    return Err(Error::Unsupported("typeToTypeNode: computed enum"))
                }
            };
            return Ok(self.ast.new_literal_type_node(Some(literal)));
        }
        if record.flags & tf::UNIQUE_ES_SYMBOL != 0 {
            if self.flags & nf::ALLOW_UNIQUE_ES_SYMBOL_TYPE == 0 {
                let symbol = record
                    .symbol
                    .ok_or(Error::MissingLink("unique symbol identity"))?;
                if self.value_symbol_accessible(symbol)? {
                    self.approximate_length += 6;
                    return self.symbol_type_node_with_meaning(symbol, sf::VALUE);
                }
                self.report(
                    ts_printer::emit_resolver::DeclarationTrackerEvent::InaccessibleUniqueSymbol,
                );
            }
            self.approximate_length += 13;
            let keyword = self.ast.new_keyword_type_node(K::SymbolKeyword.into());
            return Ok(self
                .ast
                .new_type_operator_node(K::UniqueKeyword.into(), Some(keyword)));
        }
        if record.flags & tf::NULL != 0 {
            self.approximate_length += 4;
            let literal = self.ast.new_keyword_expression(K::NullKeyword.into());
            return Ok(self.ast.new_literal_type_node(Some(literal)));
        }
        for (flag, kind, length) in [
            (tf::VOID, K::VoidKeyword, 4),
            (tf::UNDEFINED, K::UndefinedKeyword, 9),
            (tf::NEVER, K::NeverKeyword, 5),
            (tf::ES_SYMBOL, K::SymbolKeyword, 6),
            (tf::NON_PRIMITIVE, K::ObjectKeyword, 6),
        ] {
            if record.flags & flag != 0 {
                return Ok(self.keyword(kind, length));
            }
        }
        if record.flags & tf::TYPE_PARAMETER != 0
            && self.checker.types.type_parameter(ty)?.is_this_type
        {
            if self.flags & nf::IN_OBJECT_TYPE_LITERAL != 0 {
                if self.flags & nf::ALLOW_THIS_IN_OBJECT_LITERAL == 0 {
                    self.encountered_error = true;
                }
                self.report(ts_printer::emit_resolver::DeclarationTrackerEvent::InaccessibleThis);
            }
            self.approximate_length += 4;
            return Ok(self.ast.new_this_type_node());
        }
        if !in_alias {
            let alias = self.checker.types.alias_of(ty)?.cloned();
            if let Some(symbol) = crate::type_display::alias_symbol(alias.as_ref()) {
                if self.flags & nf::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE != 0
                    || self
                        .checker
                        .type_symbol_accessible(symbol, self.enclosing)?
                {
                    return self.type_reference(
                        symbol,
                        crate::type_display::alias_type_arguments(alias.as_ref()),
                    );
                }
            }
        }
        if record.object_flags & of::REFERENCE != 0 {
            return if self.checker.types.type_reference(ty)?.node.is_some() {
                self.visit_transform_type(ty, Self::reference_type_node)
            } else {
                self.reference_type_node(ty)
            };
        }
        if record.flags & tf::TYPE_PARAMETER != 0 && self.infer_parameters.contains(&ty) {
            let mut constraint_node = None;
            if let Some(constraint) = self.checker.constraint_of_type_parameter(ty)? {
                let inferred = self.checker.inferred_parameter_constraint(ty, true)?;
                if !match inferred {
                    Some(inferred) => self.checker.is_type_related_to(
                        constraint,
                        inferred,
                        crate::RelationKind::Identity,
                    )?,
                    None => false,
                } {
                    constraint_node = Some(self.type_node(constraint)?);
                }
            }
            let parameter = self.type_parameter_node_with_constraint(ty, constraint_node)?;
            return Ok(self.ast.new_infer_type_node(Some(parameter)));
        }
        if record.object_flags & of::CLASS_OR_INTERFACE != 0
            || record.flags & tf::TYPE_PARAMETER != 0
        {
            if record.flags & tf::TYPE_PARAMETER != 0
                && self.flags & nf::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS != 0
            {
                let name = self.type_parameter_name(ty)?;
                let text = self.ast.view().node_text(name)?.into_js_string();
                self.approximate_length += text.len();
                let name = self.ast.new_identifier(text);
                self.id_to_symbol.insert(name, record.symbol);
                return Ok(self.ast.new_type_reference_node(Some(name), None));
            }
            if let Some(symbol) = record.symbol {
                return self.type_reference(symbol, &[]);
            }
            let marker_parameter = self.checker.variance.checked_parameter.filter(|_| {
                ty == self.checker.builtins.marker_sub_type_for_check
                    || ty == self.checker.builtins.marker_super_type_for_check
            });
            let name = if let Some(parameter) = marker_parameter {
                if let Some(symbol) = self.checker.types.get(parameter)?.symbol {
                    let mut name = if ty == self.checker.builtins.marker_sub_type_for_check {
                        b"sub-".to_vec()
                    } else {
                        b"super-".to_vec()
                    };
                    name.extend_from_slice(self.checker.symbol(symbol)?.name_bytes());
                    JsString::from_bytes(name)
                } else {
                    JsString::from_bytes(b"?".as_slice())
                }
            } else {
                JsString::from_bytes(b"?".as_slice())
            };
            let name = self.ast.new_identifier(name);
            return Ok(self.ast.new_type_reference_node(Some(name), None));
        }
        // An origin can also be an index type (`keyof`). Dispatch on the
        // substituted type so unported families reach Unsupported rather than
        // being read through a union/intersection payload accessor.
        let ty = if record.flags & tf::UNION != 0 {
            self.checker.types.union(ty)?.origin.unwrap_or(ty)
        } else {
            ty
        };
        let record = *self.checker.types.get(ty)?;
        if record.flags & tf::UNION_OR_INTERSECTION != 0 {
            let is_union = record.flags & tf::UNION != 0;
            let constituents = self.checker.types.compound_types(ty)?.clone();
            let types = if is_union {
                self.format_union(&constituents)?
            } else {
                constituents.to_vec()
            };
            if types.len() == 1 {
                return self.type_node(types[0]);
            }
            let nodes = self.type_list(&types, true)?;
            return Ok(if is_union {
                self.ast.new_union_type_node(Some(nodes))
            } else {
                self.ast.new_intersection_type_node(Some(nodes))
            });
        }
        if record.object_flags & (of::ANONYMOUS | of::MAPPED) != 0 {
            return self.object_type(ty);
        }

        if record.flags & tf::SUBSTITUTION != 0 {
            let base = self.checker.types.substitution(ty)?.base;
            if self.checker.is_no_infer_type(ty)? {
                let symbol = self
                    .checker
                    .resolve_name(None, b"NoInfer", sf::TYPE, None, false)?;
                if let Some(symbol) = symbol {
                    if self.checker.symbol(symbol)?.flags() & sf::TYPE_ALIAS != 0
                        && self.checker.get_local_type_parameters(symbol)?.len() == 1
                    {
                        return self.type_reference(symbol, &[base]);
                    }
                }
            }
            return self.type_node(base);
        }
        if record.flags & tf::CONDITIONAL != 0 {
            return self.visit_transform_type(ty, Self::conditional_type_node);
        }
        if record.flags & tf::TEMPLATE_LITERAL != 0 {
            let data = self.checker.types.template_literal(ty)?;
            let texts = data.texts.clone();
            let types = data.types.clone();
            let head = self
                .ast
                .new_template_head(texts[0].clone(), JsString::default(), 0);
            self.emit
                .add_emit_flags(head, emit_flags::NO_ASCII_ESCAPING);
            let mut spans = Vec::new();
            for (index, &ty) in types.iter().enumerate() {
                let literal = if index + 1 < types.len() {
                    self.ast
                        .new_template_middle(texts[index + 1].clone(), JsString::default(), 0)
                } else {
                    self.ast
                        .new_template_tail(texts[index + 1].clone(), JsString::default(), 0)
                };
                self.emit
                    .add_emit_flags(literal, emit_flags::NO_ASCII_ESCAPING);
                let annotation = self.type_node(ty)?;
                spans.push(
                    self.ast
                        .new_template_literal_type_span(Some(annotation), Some(literal)),
                );
            }
            self.approximate_length += 2;
            let spans = self.list(spans)?;
            return Ok(self
                .ast
                .new_template_literal_type_node(Some(head), Some(spans)));
        }
        if record.flags & tf::STRING_MAPPING != 0 {
            return self.type_reference(
                record
                    .symbol
                    .ok_or(Error::MissingLink("string mapping display symbol"))?,
                &[self.checker.types.target(ty)?],
            );
        }
        if record.flags & tf::INDEX != 0 {
            let target = self.checker.types.index_type(ty)?.target;
            let operand = self.type_node(target)?;
            self.approximate_length += 6;
            return Ok(self
                .ast
                .new_type_operator_node(K::KeyOfKeyword.into(), Some(operand)));
        }
        if record.flags & tf::INDEXED_ACCESS != 0 {
            let data = *self.checker.types.indexed_access(ty)?;
            let object = self.type_node(data.object_type)?;
            let index = self.type_node(data.index_type)?;
            self.approximate_length += 2;
            return Ok(self
                .ast
                .new_indexed_access_type_node(Some(object), Some(index)));
        }
        Err(Error::Unsupported(
            "typeToTypeNode: unsupported type family",
        ))
    }

    // port: tsc/internal/checker/printer.go:Checker.formatUnionTypes
    fn format_union(&mut self, types: &[TypeId]) -> Result<Vec<TypeId>, Error> {
        self.format_union_with_enums(types, false)
    }

    fn object_type(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        self.anonymous_type_node(ty)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.createTypeNodeFromObjectType
    fn object_type_members_node(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        if self.checker.is_generic_mapped_type(ty)?
            || self.checker.types.get(ty)?.object_flags & of::MAPPED != 0
                && self.checker.types.mapped(ty)?.contains_error
        {
            return self.mapped_type_node(ty);
        }
        self.checker.resolve_type_members(ty)?;
        let members = self.checker.types.structured(ty)?;
        let signatures = members.signatures.clone().unwrap_or_default();
        let call_count = members.call_signature_count as usize;
        let indexes = members.index_infos.clone().unwrap_or_default();
        let properties = members.properties.clone().unwrap_or_default();
        if properties.is_empty() && indexes.is_empty() && signatures.len() == 1 {
            return self.signature_node(
                signatures[0],
                if call_count == 1 {
                    K::FunctionType
                } else {
                    K::ConstructorType
                },
                None,
                None,
            );
        }
        let mut abstract_signatures = Vec::new();
        for &signature in &signatures[call_count..] {
            if self.checker.signatures.get(signature)?.flags & crate::signature_flags::ABSTRACT != 0
            {
                abstract_signatures.push(signature);
            }
        }
        if !abstract_signatures.is_empty() {
            let mut types = Vec::new();
            for signature in &abstract_signatures {
                types.push(self.checker.isolated_signature_type(*signature)?);
            }
            let property_count = if self.flags & nf::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL != 0 {
                let mut count = 0;
                for &property in properties.iter() {
                    if self.checker.symbol(property)?.flags() & sf::PROTOTYPE == 0 {
                        count += 1;
                    }
                }
                count
            } else {
                properties.len()
            };
            let element_count =
                signatures.len() - abstract_signatures.len() + indexes.len() + property_count;
            if element_count != 0 {
                types.push(self.without_abstract_constructors(ty)?);
            }
            let intersection = self.checker.get_intersection_type(&types)?;
            return self.type_node(intersection);
        }
        let saved_flags = self.flags;
        self.flags |= nf::IN_OBJECT_TYPE_LITERAL;
        let result = (|| {
            let mut nodes = Vec::new();
            for (index, &signature) in signatures.iter().enumerate() {
                nodes.push(self.signature_node(
                    signature,
                    if index < call_count {
                        K::CallSignature
                    } else {
                        K::ConstructSignature
                    },
                    None,
                    None,
                )?);
            }
            for &index in indexes.iter() {
                if self.checker.types.object_flags(ty)? & of::REVERSE_MAPPED != 0 {
                    let placeholder = self.elided_type();
                    nodes.extend(self.object_index_nodes(index, Some(placeholder))?);
                } else {
                    nodes.extend(self.object_index_nodes(index, None)?);
                }
            }
            nodes.extend(self.object_members(&properties)?);
            let members = self.list(nodes)?;
            let node = self.ast.new_type_literal_node(Some(members));
            self.approximate_length += 2;
            if properties.is_empty() && signatures.is_empty() && indexes.is_empty()
                || saved_flags & nf::MULTILINE_OBJECT_LITERALS == 0
            {
                self.emit.set_emit_flags(node, emit_flags::SINGLE_LINE);
            }
            Ok(node)
        })();
        self.flags = saved_flags;
        result
    }

    fn elided_property(&mut self, text: &[u8]) -> Result<NodeId, Error> {
        if self.flags & nf::NO_TRUNCATION != 0 {
            return Err(Error::Unsupported(
                "node builder synthetic property elision comments",
            ));
        }
        let name = self.ast.new_identifier(JsString::from_bytes(text));
        Ok(self
            .ast
            .new_property_signature_declaration(None, Some(name), None, None, None))
    }

    fn elided_type(&mut self) -> NodeId {
        self.approximate_length += 3;
        if self.flags & nf::NO_TRUNCATION != 0 {
            let node = self.ast.new_keyword_type_node(K::AnyKeyword.into());
            return self.emit.add_synthetic_leading_comment(
                node,
                K::MultiLineCommentTrivia,
                JsString::from_bytes(b"elided".as_slice()),
                false,
            );
        }
        let name = self
            .ast
            .new_identifier(JsString::from_bytes(b"...".as_slice()));
        self.ast.new_type_reference_node(Some(name), None)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.createTypeNodesFromResolvedType
    fn object_members(&mut self, properties: &[SymbolId]) -> Result<Vec<NodeId>, Error> {
        if !properties.is_empty() && self.check_truncation() {
            return Ok(vec![self.elided_property(b"...")?]);
        }
        let mut members = Vec::new();
        for (index, &property) in properties.iter().enumerate() {
            if !self.class_expansion_property(property)? {
                continue;
            }
            let display_index = index + 1;
            if self.check_truncation() && display_index + 2 < properties.len().saturating_sub(1) {
                members.push(self.elided_property(
                    format!("... {} more ...", properties.len() - display_index).as_bytes(),
                )?);
                members.extend(self.property_elements(properties[properties.len() - 1])?);
                break;
            }
            members.extend(self.property_elements(property)?);
        }
        Ok(members)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.addPropertyToElementList
    fn property(&mut self, symbol: SymbolId) -> Result<NodeId, Error> {
        let read = self.checker.symbol(symbol)?;
        let optional = read.flags() & sf::OPTIONAL != 0;
        let reverse = read.check_flags() & check_flags::REVERSE_MAPPED != 0;
        let readonly = self.checker.is_readonly_symbol(symbol)?;
        let property_name = self.property_name_node(symbol)?;
        let placeholder = self.reverse_property_placeholder(symbol)?;
        let mut ty = if placeholder {
            self.checker.builtins.any_type
        } else {
            self.checker.get_type_of_symbol(symbol)?
        };
        // getNonMissingTypeOfSymbol -> removeMissingType. Undefined remains
        // visible when exactOptionalPropertyTypes is disabled.
        if optional && self.checker.options.exact_optional_property_types {
            let missing = self.checker.builtins.missing_type;
            ty = self
                .checker
                .map_type(ty, &mut |_, member| {
                    Ok((member != missing).then_some(member))
                })?
                .unwrap_or(self.checker.builtins.never_type);
        }
        let type_node = if placeholder {
            self.elided_type()
        } else {
            if reverse {
                self.reverse_mapped_stack.push(symbol);
            }
            let result = self.serialize_declaration_type(None, Some(ty), Some(symbol), true);
            if reverse {
                self.reverse_mapped_stack.pop();
            }
            result?
        };
        let question = optional.then(|| self.ast.new_token(K::QuestionToken.into()));
        let modifiers = if readonly {
            self.approximate_length += 9;
            let modifier = self.ast.new_modifier(K::ReadonlyKeyword.into());
            Some(self.list(vec![modifier])?)
        } else {
            None
        };
        Ok(self.ast.new_property_signature_declaration(
            modifiers,
            Some(property_name),
            question,
            Some(type_node),
            None,
        ))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.getPropertyNameNodeForSymbol
    fn property_name_node(&mut self, symbol: SymbolId) -> Result<NodeId, Error> {
        let read = self.checker.symbol(symbol)?;
        if let Some(declaration) = read.value_declaration() {
            let view = self.checker.ast(declaration)?;
            if let Some(name) = view.node(declaration)?.name() {
                if view.node(name)?.kind() == K::PrivateIdentifier {
                    let text = view.node_text(name)?.into_js_string();
                    return Ok(self.ast.new_private_identifier(text));
                }
            }
        }
        let is_method = read.flags() & sf::METHOD != 0;
        let name_type = self
            .checker
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.name_type);
        let raw_name = read.name_to_owned();
        let enclosing = match read.value_declaration() {
            Some(declaration) => Some(declaration),
            None => self
                .checker
                .symbol_declarations(symbol)?
                .iter()
                .flatten()
                .next()
                .or(self.enclosing),
        };
        if let Some(ty) = name_type {
            if self.checker.types.flags(ty)? & tf::ENUM_LITERAL != 0 {
                if let Some(context) = self.enclosing {
                    let member = self
                        .checker
                        .types
                        .get(ty)?
                        .symbol
                        .ok_or(Error::MissingLink("enum name symbol"))?;
                    let enum_symbol = self.checker.symbol(member)?.parent().unwrap_or(member);
                    let accessible = self.checker.emit_symbol_accessible(
                        Some(enum_symbol),
                        Some(context),
                        sf::VALUE,
                        false,
                        false,
                    )?;
                    if accessible.accessibility
                        == ts_printer::emit_resolver::SymbolAccessibility::Accessible
                    {
                        let expression = self.symbol_expression(member, Some(context))?;
                        return Ok(self.ast.new_computed_property_name(Some(expression)));
                    }
                }
            }
            if self.checker.types.flags(ty)? & tf::UNIQUE_ES_SYMBOL != 0 {
                let symbol = self
                    .checker
                    .types
                    .get(ty)?
                    .symbol
                    .ok_or(Error::MissingLink("unique name symbol"))?;
                let expression = self.symbol_expression(symbol, enclosing)?;
                return Ok(self.ast.new_computed_property_name(Some(expression)));
            }
        }
        let name = match name_type {
            Some(ty) => self
                .checker
                .index_property_name(ty)?
                .unwrap_or(raw_name.clone()),
            None => raw_name,
        };
        let (string_named, single_quote) = self.property_name_style(symbol)?;
        if name
            .as_bytes()
            .starts_with(ts_ast::INTERNAL_SYMBOL_NAME_PREFIX)
        {
            return Err(Error::Unsupported(
                "getPropertyNameNodeForSymbol: late/private name",
            ));
        }
        let is_identifier =
            ts_scanner::is_identifier_text(name.as_bytes(), LanguageVariant::STANDARD);
        let is_numeric_name = ts_jsnum::from_string(name.as_bytes())
            .to_string()
            .as_bytes()
            == name.as_bytes();
        let property_name =
            if name_type.is_some() && !is_identifier && (string_named || !is_numeric_name) {
                // A string-named or non-numeric literal name type is always a string literal.
                self.ast.new_string_literal(
                    name.clone(),
                    if single_quote {
                        token_flags::SINGLE_QUOTE
                    } else {
                        0
                    },
                )
            } else if is_identifier && !(is_method && name.as_bytes() == b"new") {
                self.ast.new_identifier(name.clone())
            } else if name_type.is_some() && is_numeric_name && name.as_bytes().starts_with(b"-") {
                let number = self
                    .ast
                    .new_numeric_literal(JsString::from_bytes(&name.as_bytes()[1..]), 0);
                let negative = self
                    .ast
                    .new_prefix_unary_expression(K::MinusToken.into(), Some(number));
                self.ast.new_computed_property_name(Some(negative))
            } else if !string_named
                && ts_jsnum::from_string(name.as_bytes())
                    .to_string()
                    .as_bytes()
                    == name.as_bytes()
                && ts_jsnum::from_string(name.as_bytes()).value() >= 0.0
            {
                self.ast.new_numeric_literal(name.clone(), 0)
            } else {
                self.ast.new_string_literal(
                    name.clone(),
                    if single_quote {
                        token_flags::SINGLE_QUOTE
                    } else {
                        0
                    },
                )
            };
        Ok(property_name)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.shouldUsePlaceholderForProperty
    fn reverse_property_placeholder(&self, symbol: SymbolId) -> Result<bool, Error> {
        if self.checker.symbol(symbol)?.check_flags() & check_flags::REVERSE_MAPPED == 0 {
            return Ok(false);
        }
        if self.reverse_mapped_stack.contains(&symbol) {
            return Ok(true);
        }
        if let Some(&last) = self.reverse_mapped_stack.last() {
            if let Some((property_type, _, _)) = self.checker.reverse_symbol_parts(last) {
                if self.checker.types.object_flags(property_type)? & of::ANONYMOUS == 0 {
                    return Ok(true);
                }
            }
        }
        if self.reverse_mapped_stack.len() < 3 {
            return Ok(false);
        }
        let Some((_, mapped, _)) = self.checker.reverse_symbol_parts(symbol) else {
            return Ok(false);
        };
        let Some(mapped_symbol) = self.checker.types.get(mapped)?.symbol else {
            return Ok(false);
        };
        // The pinned loop includes offsets 0..=3, despite the depth name.
        for &property in self.reverse_mapped_stack.iter().rev().take(4) {
            if let Some((_, mapped, _)) = self.checker.reverse_symbol_parts(property) {
                if self.checker.types.get(mapped)?.symbol == Some(mapped_symbol) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.isStringNamed
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.isSingleQuotedStringNamed
    fn property_name_style(&mut self, symbol: SymbolId) -> Result<(bool, bool), Error> {
        let declarations: Vec<_> = self
            .checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect();
        let mut string_named = !declarations.is_empty();
        for &declaration in &declarations {
            let view = self.checker.ast(declaration)?;
            let Some(name) = ts_ast::get_name_of_declaration(view, Some(declaration))? else {
                string_named = false;
                break;
            };
            let node = view.node(name)?;
            let is_string = node.kind() == K::StringLiteral;
            let expression = if node.kind() == K::ComputedPropertyName {
                node.expression()
            } else if node.kind() == K::ElementAccessExpression {
                node.data_source()
                    .as_element_access_expression()
                    .and_then(|data| data.argument_expression())
            } else {
                None
            };
            string_named = if let Some(expression) = expression {
                let ty = self.checker.check_expression(expression)?;
                self.checker.types.flags(ty)? & tf::STRING_LIKE != 0
            } else {
                is_string
            };
            if !string_named {
                break;
            }
        }
        let mut single_quote = !declarations.is_empty();
        for declaration in declarations {
            let view = self.checker.ast(declaration)?;
            let Some(name) = ts_ast::get_name_of_declaration(view, Some(declaration))? else {
                single_quote = false;
                break;
            };
            let node = view.node(name)?;
            single_quote = node.kind() == K::StringLiteral
                && node
                    .data_source()
                    .as_string_literal()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .token_flags()
                    & token_flags::SINGLE_QUOTE
                    != 0;
            if !single_quote {
                break;
            }
        }
        Ok((string_named, single_quote))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:typesAreSameReference
    fn types_are_same_reference(&self, a: TypeId, b: TypeId) -> Result<bool, Error> {
        if a == b {
            return Ok(true);
        }
        let left = self.checker.types.get(a)?;
        let right = self.checker.types.get(b)?;
        Ok(left.symbol.is_some() && left.symbol == right.symbol
            || left.alias.is_some() && left.alias == right.alias)
    }
}

#[cfg(all(test, not(any(miri, target_family = "wasm"))))]
#[path = "node_builder_stack_tests.rs"]
mod stack_tests;
