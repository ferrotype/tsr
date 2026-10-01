//! Declaration emit uses the binder's reference algorithm with checker hooks.
//! The one mutating hook is evaluated before lending the host's immutable views;
//! errors keep their checker identity across the binder's arena-error boundary.

use crate::{CheckerState, Error, RelationKind};
use std::cell::Cell;
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{
    symbol_flags as sf, AstView, DeclarationRead, Factory, FactoryMethods, JsString, NodeBinding,
    SymbolFlags, SymbolRef, SymbolTableId, SymbolTableRead, SyntaxKind as K,
};
use tsr_binder::name_resolver::{Hook, ResolverHost, ResolverOptions};
use tsr_binder::reference_resolver::{ReferenceResolver, ReferenceResolverHooks};
use tsr_diagnostics::Message;

struct ReferenceHost<'a> {
    state: &'a CheckerState,
    failure: &'a Cell<Option<Error>>,
}
impl ReferenceHost<'_> {
    fn capture<T>(&self, result: Result<T, Error>) -> Result<T, tsr_arena::Error> {
        result.map_err(|error| {
            self.failure.set(Some(error.clone()));
            match error {
                Error::Arena(error) => error,
                _ => tsr_arena::Error::InvalidGraph,
            }
        })
    }
}
impl ResolverHost for ReferenceHost<'_> {
    fn ast(&self, node: NodeId) -> Result<AstView<'_>, tsr_arena::Error> {
        self.capture(self.state.ast(node))
    }
    fn binding(&self, node: NodeId) -> Result<Option<NodeBinding>, tsr_arena::Error> {
        self.capture(self.state.checker_node_binding(node))
    }
    fn symbol(&self, symbol: SymbolId) -> Result<SymbolRef<'_>, tsr_arena::Error> {
        self.capture(self.state.symbol(symbol))
    }
    fn table(&self, table: SymbolTableId) -> Result<SymbolTableRead<'_>, tsr_arena::Error> {
        self.capture(self.state.table(table))
    }
    fn declarations(&self, symbol: SymbolId) -> Result<DeclarationRead<'_>, tsr_arena::Error> {
        self.capture(self.state.symbol_declarations(symbol))
    }
    fn new_transient_symbol(
        &mut self,
        _flags: SymbolFlags,
        _name: JsString,
    ) -> Result<SymbolId, tsr_arena::Error> {
        // Both entry points supply ResolveName. Taking its fallback would be a
        // change to the binder callback contract, not an alternate resolution.
        self.capture(Err(Error::MissingLink(
            "checker reference resolver bypassed ResolveName hook",
        )))
    }
}

struct NameAnswer {
    location: NodeId,
    name: JsString,
    symbol: Option<SymbolId>,
}
struct ReferenceHooks<'a> {
    host: ReferenceHost<'a>,
    node: NodeId,
    cached: Option<SymbolId>,
    name: Option<NameAnswer>,
}
impl ReferenceResolverHooks for ReferenceHooks<'_> {
    fn resolve_name(
        &mut self,
        location: Option<NodeId>,
        name: &[u8],
        meaning: SymbolFlags,
        message: Option<&'static Message>,
        is_use: bool,
        exclude_globals: bool,
    ) -> Result<Hook<Option<SymbolId>>, tsr_arena::Error> {
        self.host.capture((|| {
            let answer = self
                .name
                .as_ref()
                .ok_or(Error::MissingLink("prepared reference name callback"))?;
            if location != Some(answer.location)
                || name != answer.name.as_bytes()
                || meaning != sf::VALUE | sf::EXPORT_VALUE | sf::ALIAS
                || message.is_some()
                || is_use
                || exclude_globals
            {
                return Err(Error::MissingLink(
                    "reference resolver name callback contract",
                ));
            }
            Ok(Hook::Value(answer.symbol))
        })())
    }
    fn get_resolved_symbol(
        &mut self,
        node: NodeId,
    ) -> Result<Hook<Option<SymbolId>>, tsr_arena::Error> {
        self.host.capture(if node == self.node {
            Ok(Hook::Value(self.cached))
        } else {
            Err(Error::MissingLink("reference resolver source callback"))
        })
    }
    fn get_merged_symbol(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Hook<Option<SymbolId>>, tsr_arena::Error> {
        Ok(Hook::Value(Some(self.host.state.get_merged_symbol(symbol))))
    }
    // `GetParentOfSymbol: r.checker.getParentOfSymbol`
    fn get_parent_of_symbol(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Hook<Option<SymbolId>>, tsr_arena::Error> {
        let state = self.host.state;
        self.host.capture((|| {
            let Some(parent) = state.symbol(symbol)?.parent() else {
                return Ok(Hook::Value(None));
            };
            let parent = late_bound_symbol(state, parent)?;
            Ok(Hook::Value(Some(state.get_merged_symbol(parent))))
        })())
    }
    // `GetSymbolOfDeclaration: r.checker.getSymbolOfDeclaration`
    fn get_symbol_of_declaration(
        &mut self,
        node: NodeId,
    ) -> Result<Hook<Option<SymbolId>>, tsr_arena::Error> {
        let state = self.host.state;
        self.host.capture((|| {
            let Some(symbol) = state.raw_declaration_symbol(node)? else {
                return Ok(Hook::Value(None));
            };
            let symbol = late_bound_symbol(state, symbol)?;
            Ok(Hook::Value(Some(state.get_merged_symbol(symbol))))
        })())
    }
    fn get_export_symbol_of_value_symbol_if_exported(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Hook<Option<SymbolId>>, tsr_arena::Error> {
        self.host.capture(
            self.host
                .state
                .get_export_symbol_of_value_symbol_if_exported(symbol)
                .map(|symbol| Hook::Value(Some(symbol))),
        )
    }
}

/// `getLateBoundSymbol` for the hooks, which hold the checker immutably. A
/// symbol other than a computed class member is its own late-bound symbol, and
/// a computed member answers from the late-bound link; one whose binding has
/// not run yet fails the query rather than binding members from inside it.
fn late_bound_symbol(state: &CheckerState, symbol: SymbolId) -> Result<SymbolId, Error> {
    let data = state.symbol(symbol)?;
    if data.flags() & sf::CLASS_MEMBER == 0
        || data.name_bytes() != tsr_ast::internal_symbol_names::COMPUTED
    {
        return Ok(symbol);
    }
    match state.late_members.symbols.try_get(symbol) {
        Some(Some(late)) => Ok(*late),
        _ => Err(Error::MissingLink(
            "late-bound symbol in a reference resolver hook",
        )),
    }
}

/// A query of the binder's reference resolver (`binder.ReferenceResolver`).
#[derive(Clone, Copy)]
pub(crate) enum ReferenceQuery {
    Value,
    Member,
    ExportContainer { prefix_locals: bool },
    ValueDeclarations,
}

pub(crate) enum ReferenceAnswer {
    Node(Option<NodeId>),
    Nodes(Option<Vec<NodeId>>),
}

impl CheckerState {
    /// Where `getReferencedValueSymbol` resolves the name of a module or enum
    /// declaration for `GetReferencedExportContainer`: at the declaration's
    /// container, so an exported member of the same name is not found.
    fn reference_lookup_location(&self, node: NodeId) -> Result<NodeId, Error> {
        let Some(parent) = self.node(node)?.parent() else {
            return Ok(node);
        };
        let read = self.node(parent)?;
        if matches!(
            read.kind().known(),
            Some(K::ModuleDeclaration | K::EnumDeclaration)
        ) && read.name() == Some(node)
            && tsr_ast::is_declaration(&read)
        {
            if let Some(container) = tsr_ast::get_declaration_container(self.ast(parent)?, parent)?
            {
                return Ok(container);
            }
        }
        Ok(node)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.getReferenceResolver
    // port: tsc/internal/checker/emitresolver.go:EmitResolver.GetReferencedValueDeclarationUnsafe
    pub(crate) fn emit_referenced_value_declaration(
        &mut self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        self.emit_reference_declaration(node, false)
    }
    // port: tsc/internal/checker/emitresolver.go:EmitResolver.GetReferencedMemberValueDeclaration
    pub(crate) fn emit_referenced_member_value_declaration(
        &mut self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        if !self.emit_parse_node(node)? {
            return Ok(None);
        }
        self.emit_reference_declaration(node, true)
    }
    fn emit_reference_declaration(
        &mut self,
        node: NodeId,
        member: bool,
    ) -> Result<Option<NodeId>, Error> {
        let query = if member {
            ReferenceQuery::Member
        } else {
            ReferenceQuery::Value
        };
        match self.emit_reference_query(node, query)? {
            ReferenceAnswer::Node(node) => Ok(node),
            ReferenceAnswer::Nodes(_) => Err(Error::MissingLink("reference query answer")),
        }
    }

    /// One query of the binder's reference resolver with the checker's hooks.
    pub(crate) fn emit_reference_query(
        &mut self,
        node: NodeId,
        query: ReferenceQuery,
    ) -> Result<ReferenceAnswer, Error> {
        // getResolvedSymbolOrNil creates the native symbol-node link even when
        // the cached value is nil. Preserve that before sharing the host read.
        self.node(node)?;
        let cached = *self.query.resolved_symbols.get_or_default(node);
        let name = if !matches!(query, ReferenceQuery::Member) && cached.is_none() {
            let name = self.node_text(node)?.into_js_string();
            let location = match query {
                ReferenceQuery::ExportContainer { .. } => self.reference_lookup_location(node)?,
                _ => node,
            };
            let symbol = self.resolve_name_ex(
                Some(location),
                name.as_bytes(),
                sf::VALUE | sf::EXPORT_VALUE | sf::ALIAS,
                None,
                false,
                false,
            )?;
            Some(NameAnswer {
                location,
                name,
                symbol,
            })
        } else {
            None
        };
        let options = self.program()?.host.options();
        let mut resolver = ReferenceResolver::new(ResolverOptions {
            emit_script_target: options.emit_script_target(),
            isolated_modules: options.isolated_modules(),
            verbatim_module_syntax: options.verbatim_module_syntax.is_true(),
            emit_standard_class_fields: options.emit_standard_class_fields(),
        });
        let failure = Cell::new(None);
        let mut host = ReferenceHost {
            state: self,
            failure: &failure,
        };
        let mut hooks = ReferenceHooks {
            host: ReferenceHost {
                state: self,
                failure: &failure,
            },
            node,
            cached,
            name,
        };
        let result = match query {
            ReferenceQuery::Member => resolver
                .get_referenced_member_value_declaration(&host, &mut hooks, node)
                .map(ReferenceAnswer::Node),
            ReferenceQuery::Value => resolver
                .get_referenced_value_declaration(&mut host, &mut hooks, node)
                .map(ReferenceAnswer::Node),
            ReferenceQuery::ExportContainer { prefix_locals } => resolver
                .get_referenced_export_container(&mut host, &mut hooks, node, prefix_locals)
                .map(ReferenceAnswer::Node),
            ReferenceQuery::ValueDeclarations => resolver
                .get_referenced_value_declarations(&mut host, &mut hooks, node)
                .map(ReferenceAnswer::Nodes),
        };
        match failure.take() {
            Some(error) => Err(error),
            None => result.map_err(Error::from),
        }
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.GetElementAccessExpressionName
    pub(crate) fn emit_element_access_expression_name(
        &mut self,
        node: NodeId,
    ) -> Result<JsString, Error> {
        if !self.emit_parse_node(node)? {
            return Ok(JsString::default());
        }
        if self.node(node)?.kind() != K::ElementAccessExpression {
            return Err(Error::MissingLink("element access emit callback"));
        }
        Ok(self.flow_property_name(node)?.unwrap_or_default())
    }

    /// `GetReferencedExportContainer` of an identifier `name` that is not in
    /// the parse tree but whose parent is the parse-tree node `parent`: the
    /// JSX transform's namespace identifier (`createReactNamespace` clears
    /// `Synthesized` and wires the parent so the scope chain can be walked).
    /// The lookup identifier is built in the checker's factory, as below.
    pub(crate) fn emit_referenced_export_container_of_name(
        &mut self,
        name: JsString,
        parent: Option<NodeId>,
        prefix_locals: bool,
    ) -> Result<Option<NodeId>, Error> {
        if let Some(parent) = parent {
            self.node(parent)?;
            if parent.arena() != self.factory.id().arena() {
                self.retain_flow_source(parent)?;
            }
        }
        let node = self.factory.new_identifier(name);
        let flags = self.factory.view().node(node)?.flags() & !tsr_ast::node_flags::SYNTHESIZED;
        self.factory.set_node_flags(node, flags);
        self.factory.set_node_parent(node, parent);
        match self.emit_reference_query(node, ReferenceQuery::ExportContainer { prefix_locals })? {
            ReferenceAnswer::Node(node) => Ok(node),
            ReferenceAnswer::Nodes(_) => Err(Error::MissingLink("export container answer")),
        }
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.getNameExpressionPreferringIdentifier
    // Lookup-only syntax uses the checker factory because the caller's output
    // factory is not a member of the checker's retained owner set.
    pub(crate) fn emit_referenced_name_declaration(
        &mut self,
        name: JsString,
        parent: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        self.node(parent)?;
        if parent.arena() != self.factory.id().arena() {
            self.retain_flow_source(parent)?;
        }
        let node = self.factory.new_identifier(name);
        let flags = self.factory.view().node(node)?.flags() & !tsr_ast::node_flags::SYNTHESIZED;
        self.factory.set_node_flags(node, flags);
        self.factory.set_node_parent(node, Some(parent));
        self.emit_referenced_value_declaration(node)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.IsThisPropertyAssignmentDeclarationRedundant
    pub(crate) fn emit_redundant_this_property_assignment(
        &mut self,
        node: NodeId,
    ) -> Result<bool, Error> {
        let Some(symbol) = self.get_symbol_of_declaration(node)? else {
            return Ok(false);
        };
        let Some(parent) = self.symbol(symbol)?.parent() else {
            return Ok(false);
        };
        let parent = self.get_declared_type_of_symbol(parent)?;
        let name = self.symbol(symbol)?.name_to_owned();
        for &base in self.interface_base_types(parent)?.iter() {
            let Some(property) = self.constituent_property(base, name.as_bytes(), false)? else {
                continue;
            };
            let flags = self.symbol(property)?.flags();
            if flags & (sf::ACCESSOR | sf::METHOD | sf::FUNCTION) != 0 {
                return Ok(true);
            }
            if self.is_readonly_symbol(property)? == self.is_readonly_symbol(symbol)?
                && self.symbol(symbol)?.flags() & sf::OPTIONAL == flags & sf::OPTIONAL
            {
                let left = self.get_type_of_symbol(symbol)?;
                let right = self.get_type_of_symbol(property)?;
                if self.is_type_related_to(left, right, RelationKind::Identity)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.IsDefinitelyReferenceToGlobalSymbolObject
    pub(crate) fn emit_definitely_global_symbol_object(
        &mut self,
        node: NodeId,
    ) -> Result<bool, Error> {
        let read = self.node(node)?;
        if read.kind() != K::PropertyAccessExpression {
            return Ok(false);
        }
        let name = read
            .name()
            .ok_or(Error::MissingLink("global Symbol property name"))?;
        if self.node(name)?.kind() != K::Identifier {
            return Ok(false);
        }
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("global Symbol receiver"))?;
        let read = self.node(expression)?;
        if read.kind() == K::Identifier {
            if self.node_text(expression)?.as_bytes() != b"Symbol" {
                return Ok(false);
            }
            let resolved = self.resolved_value_symbol(expression)?;
            let global =
                self.resolve_name(None, b"Symbol", sf::VALUE | sf::EXPORT_VALUE, None, false)?;
            return Ok(Some(resolved) == global);
        }
        if read.kind() != K::PropertyAccessExpression {
            return Ok(false);
        }
        let name = read
            .name()
            .ok_or(Error::MissingLink("globalThis Symbol name"))?;
        let receiver = read
            .expression()
            .ok_or(Error::MissingLink("globalThis Symbol receiver"))?;
        if self.node(receiver)?.kind() != K::Identifier
            || self.node_text(receiver)?.as_bytes() != b"globalThis"
            || self.node_text(name)?.as_bytes() != b"Symbol"
        {
            return Ok(false);
        }
        Ok(self.resolved_value_symbol(receiver)? == self.builtins.global_this_symbol)
    }
}
