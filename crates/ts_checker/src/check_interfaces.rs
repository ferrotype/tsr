//! Interface inheritance is checked once per merged symbol, before checking
//! each declaration's heritage references and members.

use crate::{ternary as tr, CheckerState, Error, RelationKind, TypeId};
use std::sync::Arc;
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, Diagnostic, SyntaxKind as K};
use ts_diagnostics as messages;

impl CheckerState {
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarInterfaceDeclaration
    pub(crate) fn check_interface_heritage_grammar(&mut self, node: NodeId) -> Result<bool, Error> {
        let clauses = self
            .ast(node)?
            .node(node)?
            .data_source()
            .as_interface_declaration()
            .ok_or(Error::MissingLink("interface payload"))?
            .heritage_clauses();
        let mut seen_extends = false;
        for clause in self.source_list(node, clauses)? {
            let token = self
                .ast(clause)?
                .node(clause)?
                .data_source()
                .as_heritage_clause()
                .ok_or(Error::MissingLink("interface heritage clause"))?
                .token();
            match token.known() {
                Some(K::ExtendsKeyword) => {
                    if seen_extends {
                        return self.grammar_error_first_token(
                            clause,
                            messages::X_extends_clause_already_seen,
                            vec![],
                        );
                    }
                    seen_extends = true;
                }
                Some(K::ImplementsKeyword) => {
                    return self.grammar_error_first_token(
                        clause,
                        messages::Interface_declaration_cannot_have_implements_clause,
                        vec![],
                    )
                }
                _ => return Err(ts_arena::Error::InvalidGraph.into()),
            }
            // Go continues to the next clause after a clause-local error.
            self.check_heritage_clause_grammar(clause)?;
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkInterfaceDeclaration
    pub(crate) fn check_interface_inheritance(
        &mut self,
        name: NodeId,
        symbol: SymbolId,
    ) -> Result<(), Error> {
        self.check_class_or_interface_type_parameters_identical(symbol)?;
        if !self.query.interfaces_checked.insert(symbol) {
            return Ok(());
        }
        let result = (|| {
            let ty = self.get_declared_type_of_symbol(symbol)?;
            let this = self.types.interface(ty)?.this_type;
            // Go's nil this argument selects the target's own this type.
            let with_this = self.get_type_with_optional_this_argument(ty, None, false)?;
            if self.check_inherited_properties_identical(ty, name, this)? {
                for base in self.interface_base_types(ty)?.to_vec() {
                    let base = self.get_type_with_optional_this_argument(base, this, false)?;
                    let (_, diagnostic) = self.check_type_related_ex(
                        with_this,
                        base,
                        RelationKind::Assignable,
                        Some(name),
                        Some(messages::Interface_0_incorrectly_extends_interface_1),
                    )?;
                    if let Some(diagnostic) = diagnostic {
                        self.add_diagnostic(diagnostic)?;
                    }
                }
                self.check_index_constraints(ty, false)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.query.interfaces_checked.remove(&symbol);
        }
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.checkInheritedPropertiesAreIdentical
    fn check_inherited_properties_identical(
        &mut self,
        ty: TypeId,
        name: NodeId,
        this: Option<TypeId>,
    ) -> Result<bool, Error> {
        let bases = self.interface_base_types(ty)?;
        if bases.len() < 2 {
            return Ok(true);
        }
        self.resolve_declared_members(ty)?;
        let members = self.types.interface(ty)?.declared_members;
        let mut seen = crate::types::Map::default();
        for &property in self
            .get_named_members(members, None)?
            .as_deref()
            .unwrap_or_default()
        {
            seen.insert(self.symbol(property)?.name_to_owned(), (property, ty));
        }
        let mut identical = true;
        for &base in bases.iter() {
            let base_with_this = self.get_type_with_optional_this_argument(base, this, false)?;
            for property in self.get_properties_of_type(base_with_this)? {
                let property_name = self.symbol(property)?.name_to_owned();
                let Some(&(existing, containing_type)) = seen.get(&property_name) else {
                    seen.insert(property_name, (property, base));
                    continue;
                };
                // A declared override is checked by assignability below; only
                // two inherited properties must have identical declarations.
                if containing_type != ty && !self.is_property_identical_to(existing, property)? {
                    identical = false;
                    let first =
                        self.type_to_string(containing_type, crate::type_display::DEFAULT_FLAGS)?;
                    let second = self.type_to_string(base, crate::type_display::DEFAULT_FLAGS)?;
                    let property_name = self.symbol_to_string(property)?;
                    let detail = self.diagnostic_for_node(
                        Some(name),
                        messages::Named_property_0_of_types_1_and_2_are_not_identical,
                        vec![property_name, first.clone(), second.clone()],
                    )?;
                    let derived = self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)?;
                    self.add_diagnostic(Diagnostic::chain(
                        Some(Arc::new(detail)),
                        messages::Interface_0_cannot_simultaneously_extend_types_1_and_2,
                        vec![derived, first, second],
                    ))?;
                }
            }
        }
        Ok(identical)
    }

    // port: tsc/internal/checker/checker.go:Checker.isPropertyIdenticalTo
    fn is_property_identical_to(
        &mut self,
        source: SymbolId,
        target: SymbolId,
    ) -> Result<bool, Error> {
        Ok(
            self.compare_properties(source, target, &mut |state, source, target| {
                Ok(
                    if state.is_type_related_to(source, target, RelationKind::Identity)? {
                        tr::TRUE
                    } else {
                        tr::FALSE
                    },
                )
            })? != tr::FALSE,
        )
    }

    pub(crate) fn check_interface_heritage(&mut self, node: NodeId) -> Result<(), Error> {
        for heritage in self.interface_base_nodes(node)? {
            if self.ast(heritage)?.node(heritage)?.kind() == K::ExpressionWithTypeArguments {
                let expression = self
                    .ast(heritage)?
                    .node(heritage)?
                    .expression()
                    .ok_or(Error::MissingLink("interface extends expression"))?;
                if !ts_ast::is_entity_name_expression(self.ast(expression)?, expression)?
                    || self.ast(expression)?.node(expression)?.flags() & nf::OPTIONAL_CHAIN != 0
                {
                    self.error_at(Some(expression), messages::An_interface_can_only_extend_an_identifier_Slashqualified_name_with_optional_type_arguments, vec![])?;
                }
            }
            self.check_type_reference_node(heritage)?;
        }
        Ok(())
    }
}
