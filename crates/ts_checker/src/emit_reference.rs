//! Declaration emit uses the binder's reference algorithm with checker hooks.
//! The one mutating hook is evaluated before lending the host's immutable views;
//! errors keep their checker identity across the binder's arena-error boundary.

use crate::{CheckerState, Error, RelationKind};
use std::cell::Cell;
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    symbol_flags as sf, AstView, DeclarationRead, Factory, FactoryMethods, JsString, NodeBinding,
    SymbolFlags, SymbolRef, SymbolTableId, SymbolTableRead, SyntaxKind as K,
};
use ts_binder::name_resolver::{Hook, ResolverHost, ResolverOptions};
use ts_binder::reference_resolver::{ReferenceResolver, ReferenceResolverHooks};
use ts_diagnostics::Message;

struct ReferenceHost<'a> {
    state: &'a CheckerState,
    failure: &'a Cell<Option<Error>>,
}
impl ReferenceHost<'_> {
    fn capture<T>(&self, result: Result<T, Error>) -> Result<T, ts_arena::Error> {
        result.map_err(|error| {
            self.failure.set(Some(error));
            match error {
                Error::Arena(error) => error,
                _ => ts_arena::Error::InvalidGraph,
            }
        })
    }
}
impl ResolverHost for ReferenceHost<'_> {
    fn ast(&self, node: NodeId) -> Result<AstView<'_>, ts_arena::Error> {
        self.capture(self.state.ast(node))
    }
    fn binding(&self, node: NodeId) -> Result<Option<NodeBinding>, ts_arena::Error> {
        self.capture(self.state.checker_node_binding(node))
    }
    fn symbol(&self, symbol: SymbolId) -> Result<SymbolRef<'_>, ts_arena::Error> {
        self.capture(self.state.symbol(symbol))
    }
    fn table(&self, table: SymbolTableId) -> Result<SymbolTableRead<'_>, ts_arena::Error> {
        self.capture(self.state.table(table))
    }
    fn declarations(&self, symbol: SymbolId) -> Result<DeclarationRead<'_>, ts_arena::Error> {
        self.capture(self.state.symbol_declarations(symbol))
    }
    fn new_transient_symbol(
        &mut self,
        _flags: SymbolFlags,
        _name: JsString,
    ) -> Result<SymbolId, ts_arena::Error> {
        // Both entry points supply ResolveName. Taking its fallback would be a
        // change to the binder callback contract, not an alternate resolution.
        self.capture(Err(Error::MissingLink(
            "checker reference resolver bypassed ResolveName hook",
        )))
    }
}

struct NameAnswer {
    node: NodeId,
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
    ) -> Result<Hook<Option<SymbolId>>, ts_arena::Error> {
        self.host.capture((|| {
            let answer = self
                .name
                .as_ref()
                .ok_or(Error::MissingLink("prepared reference name callback"))?;
            if location != Some(answer.node)
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
    ) -> Result<Hook<Option<SymbolId>>, ts_arena::Error> {
        self.host.capture(if node == self.node {
            Ok(Hook::Value(self.cached))
        } else {
            Err(Error::MissingLink("reference resolver source callback"))
        })
    }
    fn get_merged_symbol(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Hook<Option<SymbolId>>, ts_arena::Error> {
        Ok(Hook::Value(Some(self.host.state.get_merged_symbol(symbol))))
    }
    fn get_export_symbol_of_value_symbol_if_exported(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Hook<Option<SymbolId>>, ts_arena::Error> {
        self.host.capture(
            self.host
                .state
                .get_export_symbol_of_value_symbol_if_exported(symbol)
                .map(|symbol| Hook::Value(Some(symbol))),
        )
    }
}

impl CheckerState {
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
        // getResolvedSymbolOrNil creates the native symbol-node link even when
        // the cached value is nil. Preserve that before sharing the host read.
        self.ast(node)?.node(node)?;
        let cached = *self.query.resolved_symbols.get_or_default(node);
        let name = if !member && cached.is_none() {
            let name = self.ast(node)?.node_text(node)?.into_js_string();
            let symbol = self.resolve_name_ex(
                Some(node),
                name.as_bytes(),
                sf::VALUE | sf::EXPORT_VALUE | sf::ALIAS,
                None,
                false,
                false,
            )?;
            Some(NameAnswer { node, name, symbol })
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
        let result = if member {
            resolver.get_referenced_member_value_declaration(&host, &mut hooks, node)
        } else {
            resolver.get_referenced_value_declaration(&mut host, &mut hooks, node)
        };
        match failure.get() {
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
        if self.ast(node)?.node(node)?.kind() != K::ElementAccessExpression {
            return Err(Error::MissingLink("element access emit callback"));
        }
        Ok(self.flow_property_name(node)?.unwrap_or_default())
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.getNameExpressionPreferringIdentifier
    // Lookup-only syntax uses the checker factory because the caller's output
    // factory is not a member of the checker's retained owner set.
    pub(crate) fn emit_referenced_name_declaration(
        &mut self,
        name: JsString,
        parent: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        self.ast(parent)?.node(parent)?;
        if parent.arena() != self.factory.id().arena() {
            self.retain_flow_source(parent)?;
        }
        let node = self.factory.new_identifier(name);
        let flags = self.factory.view().node(node)?.flags() & !ts_ast::node_flags::SYNTHESIZED;
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
        let read = self.ast(node)?.node(node)?;
        if read.kind() != K::PropertyAccessExpression {
            return Ok(false);
        }
        let name = read
            .name()
            .ok_or(Error::MissingLink("global Symbol property name"))?;
        if self.ast(name)?.node(name)?.kind() != K::Identifier {
            return Ok(false);
        }
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("global Symbol receiver"))?;
        let read = self.ast(expression)?.node(expression)?;
        if read.kind() == K::Identifier {
            if self.ast(expression)?.node_text(expression)?.as_bytes() != b"Symbol" {
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
        if self.ast(receiver)?.node(receiver)?.kind() != K::Identifier
            || self.ast(receiver)?.node_text(receiver)?.as_bytes() != b"globalThis"
            || self.ast(name)?.node_text(name)?.as_bytes() != b"Symbol"
        {
            return Ok(false);
        }
        Ok(self.resolved_value_symbol(receiver)? == self.builtins.global_this_symbol)
    }
}
