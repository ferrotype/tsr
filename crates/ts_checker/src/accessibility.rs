//! Declaration accessibility shares the name-chain and container algorithms
//! used by name serialization, including alias and module ordering.
use crate::{node_builder::NodeBuilder, CheckerState, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, symbol_flags as sf, JsString, SyntaxKind as K};
use ts_printer::emit_resolver::{
    SymbolAccessibility as Access, SymbolAccessibilityResult as ResultInfo,
};

impl CheckerState {
    // port: tsc/internal/checker/symbolaccessibility.go:Checker.isSymbolAccessibleWorker
    pub(crate) fn emit_symbol_accessible(
        &mut self,
        symbol: Option<SymbolId>,
        enclosing: Option<NodeId>,
        meaning: u32,
        compute_aliases: bool,
        allow_modules: bool,
    ) -> Result<ResultInfo, Error> {
        let (Some(symbol), Some(enclosing)) = (symbol, enclosing) else {
            return Ok(ResultInfo::accessible());
        };
        // One scratch name context spans the recursive search. Its visited
        // tables and alias ordering are the same ones used by serialization.
        let mut builder = NodeBuilder::new(self, ts_nodebuilder::flags::IGNORE_ERRORS);
        if let Some(result) = builder.any_symbol_accessible(
            &[symbol],
            enclosing,
            symbol,
            meaning,
            compute_aliases,
            allow_modules,
        )? {
            return Ok(result);
        }
        let declarations: Vec<_> = builder
            .checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect();
        let mut external = None;
        for declaration in declarations {
            external = builder.accessibility_external_container(declaration)?;
            if external.is_some() {
                break;
            }
        }
        if let Some(external) = external {
            if Some(external) != builder.accessibility_external_container(enclosing)? {
                return Ok(ResultInfo {
                    accessibility: Access::CannotBeNamed,
                    error_symbol_name: builder.checker.emit_symbol_name(
                        symbol,
                        Some(enclosing),
                        meaning,
                    )?,
                    error_module_name: builder.checker.symbol_to_string(external)?,
                    error_node: (builder.checker.ast(enclosing)?.node(enclosing)?.flags()
                        & nf::JAVA_SCRIPT_FILE
                        != 0)
                        .then_some(enclosing),
                    ..ResultInfo::accessible()
                });
            }
        }
        Ok(ResultInfo {
            accessibility: Access::NotAccessible,
            error_symbol_name: builder.checker.emit_symbol_name(
                symbol,
                Some(enclosing),
                meaning,
            )?,
            ..ResultInfo::accessible()
        })
    }

    // port: tsc/internal/checker/emitresolver.go:getMeaningOfEntityNameReference
    pub(crate) fn emit_entity_meaning(&self, node: NodeId) -> Result<u32, Error> {
        let read = self.ast(node)?.node(node)?;
        let Some(parent) = read.parent() else {
            return Ok(sf::TYPE);
        };
        let p = self.ast(parent)?.node(parent)?;
        let kind = p.kind();
        let predicate_parameter = p
            .data_source()
            .as_type_predicate_node()
            .is_some_and(|d| d.parameter_name() == Some(node));
        if kind == K::TypeQuery
            || kind == K::ExpressionWithTypeArguments && !self.is_part_of_type_node(parent)?
            || kind == K::ComputedPropertyName
            || predicate_parameter
            || kind == K::BinaryExpression
        {
            return Ok(sf::VALUE | sf::EXPORT_VALUE);
        }
        if matches!(
            read.kind().known(),
            Some(K::QualifiedName | K::PropertyAccessExpression)
        ) || kind == K::ImportEqualsDeclaration
            || p.data_source()
                .as_qualified_name()
                .is_some_and(|d| d.left() == Some(node))
            || matches!(
                kind.known(),
                Some(K::PropertyAccessExpression | K::ElementAccessExpression)
            ) && p.expression() == Some(node)
        {
            return Ok(sf::NAMESPACE);
        }
        Ok(sf::TYPE)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.isEntityNameVisible
    pub(crate) fn emit_entity_visible(
        &mut self,
        node: NodeId,
        enclosing: NodeId,
    ) -> Result<ResultInfo, Error> {
        self.emit_entity_visible_ex(node, enclosing, true)
    }

    pub(crate) fn emit_entity_visible_ex(
        &mut self,
        node: NodeId,
        enclosing: NodeId,
        compute_aliases: bool,
    ) -> Result<ResultInfo, Error> {
        if !self.emit_parse_node(node)? {
            return Ok(ResultInfo {
                accessibility: Access::NotAccessible,
                ..ResultInfo::accessible()
            });
        }
        let meaning = self.emit_entity_meaning(node)?;
        let first = ts_ast::utilities_middle::get_first_identifier(self.ast(node)?, node)?;
        let name = self.ast(first)?.node_text(first)?.into_js_string();
        let symbol = self.resolve_name(Some(enclosing), name.as_bytes(), meaning, None, false)?;
        if let Some(symbol) = symbol {
            if self.symbol(symbol)?.flags() & sf::TYPE_PARAMETER != 0 && meaning & sf::TYPE != 0 {
                return Ok(ResultInfo::accessible());
            }
            if let Some(result) = self.emit_visible_declarations(symbol, compute_aliases)? {
                return Ok(result);
            }
        } else if name.as_bytes() == b"this" {
            let container = ts_ast::get_this_container(self.ast(first)?, first, false, false)?;
            let symbol = self.get_symbol_of_declaration(container)?;
            if self
                .emit_symbol_accessible(symbol, Some(enclosing), meaning, false, true)?
                .accessibility
                == Access::Accessible
            {
                return Ok(ResultInfo::accessible());
            }
        }
        Ok(ResultInfo {
            accessibility: if symbol.is_some() {
                Access::NotAccessible
            } else {
                Access::NotResolved
            },
            error_symbol_name: name,
            error_node: Some(first),
            ..ResultInfo::accessible()
        })
    }

    // port: tsc/internal/checker/printer.go:Checker.symbolToStringEx
    pub(crate) fn emit_symbol_name(
        &mut self,
        symbol: SymbolId,
        enclosing: Option<NodeId>,
        meaning: u32,
    ) -> Result<JsString, Error> {
        use ts_printer::EmitTextWriter;
        let mut builder = NodeBuilder::new(self, ts_nodebuilder::flags::IGNORE_ERRORS);
        let node = builder.symbol_expression_with_meaning(symbol, enclosing, meaning)?;
        let printer = ts_printer::Printer::new(
            ts_printer::PrinterOptions {
                remove_comments: true,
                omit_trailing_semicolon: true,
                ..Default::default()
            },
            &builder.emit,
        );
        let mut writer = ts_printer::SingleLineStringWriter::new();
        printer.write(builder.ast.view(), node, None, &mut writer)?;
        Ok(JsString::from_bytes(writer.text().to_vec()))
    }
}

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/symbolaccessibility.go:Checker.IsAnySymbolAccessible
    fn any_symbol_accessible(
        &mut self,
        symbols: &[SymbolId],
        enclosing: NodeId,
        initial: SymbolId,
        meaning: u32,
        compute_aliases: bool,
        allow_modules: bool,
    ) -> Result<Option<ResultInfo>, Error> {
        let mut had_chain = None;
        let mut early_module = false;
        for &symbol in symbols {
            let chain = self.accessibility_chain(symbol, enclosing, meaning)?;
            if let Some(&first) = chain.first() {
                had_chain = Some(symbol);
                if let Some(result) = self
                    .checker
                    .emit_visible_declarations(first, compute_aliases)?
                {
                    return Ok(Some(result));
                }
            }
            if allow_modules && self.name_external_module(symbol)? {
                if compute_aliases {
                    early_module = true;
                    continue;
                }
                return Ok(Some(ResultInfo::accessible()));
            }
            let containers = self.accessibility_containers(symbol, enclosing, meaning)?;
            let next = if symbol == initial {
                if meaning == sf::VALUE {
                    sf::VALUE
                } else {
                    sf::NAMESPACE
                }
            } else {
                meaning
            };
            if let Some(result) = self.any_symbol_accessible(
                &containers,
                enclosing,
                initial,
                next,
                compute_aliases,
                allow_modules,
            )? {
                return Ok(Some(result));
            }
        }
        if early_module {
            return Ok(Some(ResultInfo::accessible()));
        }
        if let Some(symbol) = had_chain {
            let module = if symbol == initial {
                JsString::default()
            } else {
                self.checker
                    .emit_symbol_name(symbol, Some(enclosing), sf::NAMESPACE)?
            };
            return Ok(Some(ResultInfo {
                accessibility: Access::NotAccessible,
                error_symbol_name: self.checker.emit_symbol_name(
                    initial,
                    Some(enclosing),
                    meaning,
                )?,
                error_module_name: module,
                ..ResultInfo::accessible()
            }));
        }
        Ok(None)
    }
}
