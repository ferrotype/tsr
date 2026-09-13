//! One ordered assignment walk per containing function/source. Results publish
//! only after the scan succeeds; an unsupported edge cannot become a cache hit.
use crate::{types::Map, CheckerState, Error, LinkStore};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, node_flags as nf, symbol_flags as sf, SyntaxKind as K};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AssignmentKind {
    None,
    Definite,
    Compound,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct MarkedAssignment {
    pub(crate) last_assignment_pos: i32,
    pub(crate) has_definite_assignment: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum MarkingStatus {
    Visiting,
    Complete,
    Failed(Error),
}

#[derive(Default)]
pub(crate) struct FlowAssignments {
    pub(crate) symbols: LinkStore<SymbolId, MarkedAssignment>,
    pub(crate) roots: Map<NodeId, MarkingStatus>,
}

fn required<T>(value: Option<T>, context: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(context))
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.isSomeSymbolAssigned
    pub(crate) fn some_binding_symbol_assigned(&mut self, root: NodeId) -> Result<bool, Error> {
        let name = self
            .ast(root)?
            .node(root)?
            .name()
            .ok_or(Error::MissingLink("assigned binding name"))?;
        self.some_binding_symbol_assigned_worker(name)
    }
    // port: tsc/internal/checker/checker.go:Checker.isSomeSymbolAssignedWorker
    fn some_binding_symbol_assigned_worker(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() == K::Identifier {
            let parent = read
                .parent()
                .ok_or(Error::MissingLink("assigned binding declaration"))?;
            let symbol = self
                .get_symbol_of_declaration(parent)?
                .ok_or(Error::MissingLink("assigned binding symbol"))?;
            return self.is_symbol_assigned(symbol);
        }
        let elements = self.source_list(node, read.element_list())?;
        for element in elements {
            if let Some(name) = self.ast(element)?.node(element)?.name() {
                if self.some_binding_symbol_assigned_worker(name)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/utilities.go:getAssignmentTargetKind
    pub(crate) fn assignment_target_kind(&self, node: NodeId) -> Result<AssignmentKind, Error> {
        let view = self.ast(node)?;
        let Some(target) = ts_ast::get_assignment_target(view, node)? else {
            return Ok(AssignmentKind::None);
        };
        let target = view.node(target)?;
        match target.kind().known() {
            Some(K::BinaryExpression) => {
                let binary = target
                    .data_source()
                    .as_binary_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let operator = view
                    .node(required(binary.operator_token(), "assignment operator")?)?
                    .kind();
                Ok(
                    if operator == K::EqualsToken
                        || ts_ast::is_logical_or_coalescing_assignment_operator(operator)
                    {
                        AssignmentKind::Definite
                    } else {
                        AssignmentKind::Compound
                    },
                )
            }
            Some(K::PrefixUnaryExpression | K::PostfixUnaryExpression) => {
                Ok(AssignmentKind::Compound)
            }
            Some(K::ForInStatement | K::ForOfStatement) => Ok(AssignmentKind::Definite),
            _ => Err(ts_arena::Error::InvalidGraph.into()),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getControlFlowContainer
    pub(crate) fn control_flow_container(&self, node: NodeId) -> Result<NodeId, Error> {
        self.control_flow_container_or_none(node)?
            .ok_or(Error::MissingLink("control-flow container"))
    }

    pub(crate) fn control_flow_container_or_none(
        &self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let mut current = self.ast(node)?.node(node)?.parent();
        while let Some(id) = current {
            let view = self.ast(id)?;
            let read = view.node(id)?;
            if matches!(
                read.kind().known(),
                Some(K::ModuleBlock | K::SourceFile | K::PropertyDeclaration)
            ) || ts_ast::utilities::is_function_like(Some(&read))
                && ts_ast::get_immediately_invoked_function_expression(view, id)?.is_none()
            {
                return Ok(Some(id));
            }
            current = read.parent();
        }
        Ok(None)
    }

    // port: tsc/internal/checker/utilities.go:Checker.isParameterOrMutableLocalVariable
    pub(crate) fn is_parameter_or_mutable_local_variable(
        &self,
        symbol: SymbolId,
    ) -> Result<bool, Error> {
        let Some(declaration) = self.symbol(symbol)?.value_declaration() else {
            return Ok(false);
        };
        let view = self.ast(declaration)?;
        let root = ts_ast::utilities::get_root_declaration(view, declaration)?;
        let read = view.node(root)?;
        if read.kind() == K::Parameter {
            return Ok(true);
        }
        if read.kind() != K::VariableDeclaration {
            return Ok(false);
        }
        let parent = required(read.parent(), "variable declaration parent")?;
        if view.node(parent)?.kind() == K::CatchClause {
            return Ok(true);
        }
        self.is_mutable_local_variable_declaration(root)
    }

    // port: tsc/internal/checker/utilities.go:Checker.isMutableLocalVariableDeclaration
    pub(crate) fn is_mutable_local_variable_declaration(
        &self,
        declaration: NodeId,
    ) -> Result<bool, Error> {
        let view = self.ast(declaration)?;
        let parent = required(view.node(declaration)?.parent(), "local declaration list")?;
        let parent = view.node(parent)?;
        if parent.flags() & nf::LET == 0
            || ts_ast::utilities::get_combined_modifier_flags(view, declaration)? & mf::EXPORT != 0
        {
            return Ok(false);
        }
        let statement = required(parent.parent(), "declaration statement")?;
        let statement = view.node(statement)?;
        if statement.kind() != K::VariableStatement {
            return Ok(true);
        }
        let container = required(statement.parent(), "variable statement container")?;
        Ok(!ts_ast::utilities_middle::is_global_source_file(
            view, container,
        )?)
    }

    // port: tsc/internal/checker/flow.go:Checker.isSymbolAssignedDefinitely
    pub(crate) fn is_symbol_assigned_definitely(
        &mut self,
        symbol: SymbolId,
    ) -> Result<bool, Error> {
        self.ensure_assignments_marked(symbol)?;
        Ok(self
            .flow
            .assignments
            .symbols
            .get_or_default(symbol)
            .has_definite_assignment)
    }

    // port: tsc/internal/checker/flow.go:Checker.isSymbolAssigned
    pub(crate) fn is_symbol_assigned(&mut self, symbol: SymbolId) -> Result<bool, Error> {
        self.ensure_assignments_marked(symbol)?;
        Ok(self
            .flow
            .assignments
            .symbols
            .get_or_default(symbol)
            .last_assignment_pos
            != 0)
    }

    // port: tsc/internal/checker/flow.go:Checker.isPastLastAssignment
    pub(crate) fn is_past_last_assignment(
        &mut self,
        symbol: SymbolId,
        location: Option<NodeId>,
    ) -> Result<bool, Error> {
        self.ensure_assignments_marked(symbol)?;
        let position = self
            .flow
            .assignments
            .symbols
            .get_or_default(symbol)
            .last_assignment_pos;
        Ok(position == 0
            || match location {
                Some(location) => position < self.ast(location)?.node(location)?.pos(),
                None => false,
            })
    }

    fn assignment_function(&self, node: Option<NodeId>) -> Result<Option<NodeId>, Error> {
        let Some(node) = node else {
            return Ok(None);
        };
        Ok(ts_ast::utilities::find_ancestor(
            self.ast(node)?,
            Some(node),
            |read| ts_ast::utilities::is_function_or_source_file(read),
        )?)
    }

    // port: tsc/internal/checker/flow.go:Checker.ensureAssignmentsMarked
    // port: tsc/internal/checker/flow.go:Checker.hasParentWithAssignmentsMarked
    fn ensure_assignments_marked(&mut self, symbol: SymbolId) -> Result<(), Error> {
        let Some(root) = self.assignment_function(self.symbol(symbol)?.value_declaration())? else {
            return Ok(());
        };
        match self.flow.assignments.roots.get(&root).copied() {
            Some(MarkingStatus::Complete) => return Ok(()),
            Some(MarkingStatus::Failed(error)) => return Err(error),
            Some(MarkingStatus::Visiting) => {
                return Err(Error::Unsupported(
                    "ensureAssignmentsMarked: reentrant assignment scan",
                ))
            }
            None => {}
        }
        let mut parent = self.ast(root)?.node(root)?.parent();
        while let Some(function) = self.assignment_function(parent)? {
            match self.flow.assignments.roots.get(&function).copied() {
                Some(MarkingStatus::Complete) => {
                    self.flow
                        .assignments
                        .roots
                        .insert(root, MarkingStatus::Complete);
                    return Ok(());
                }
                Some(MarkingStatus::Failed(error)) => return Err(error),
                Some(MarkingStatus::Visiting) => {
                    return Err(Error::Unsupported(
                        "ensureAssignmentsMarked: ancestor assignment scan",
                    ))
                }
                None => {}
            }
            parent = self.ast(function)?.node(function)?.parent();
        }
        self.flow
            .assignments
            .roots
            .insert(root, MarkingStatus::Visiting);
        let result = self.mark_node_assignments(root);
        match result {
            Ok(marked) => {
                for (symbol, links) in marked {
                    *self.flow.assignments.symbols.get_or_default(symbol) = links;
                }
                self.flow
                    .assignments
                    .roots
                    .insert(root, MarkingStatus::Complete);
                Ok(())
            }
            Err(error) => {
                self.flow
                    .assignments
                    .roots
                    .insert(root, MarkingStatus::Failed(error));
                Err(error)
            }
        }
    }

    fn previous_assignment(&self, symbol: SymbolId) -> MarkedAssignment {
        self.flow
            .assignments
            .symbols
            .try_get(symbol)
            .copied()
            .unwrap_or_default()
    }

    // port: tsc/internal/checker/flow.go:Checker.markNodeAssignmentsWorker
    fn mark_node_assignments(
        &mut self,
        root: NodeId,
    ) -> Result<Map<SymbolId, MarkedAssignment>, Error> {
        let mut marked: Map<SymbolId, MarkedAssignment> = Map::default();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let view = self.ast(node)?;
            let read = view.node(node)?;
            match read.kind().known() {
                Some(K::Identifier) => {
                    let kind = self.assignment_target_kind(node)?;
                    if kind != AssignmentKind::None {
                        let symbol = self.resolved_value_symbol(node)?;
                        if self.is_parameter_or_mutable_local_variable(symbol)? {
                            let declaration = required(
                                self.symbol(symbol)?.value_declaration(),
                                "assigned declaration",
                            )?;
                            let entry = marked
                                .entry(symbol)
                                .or_insert_with(|| self.previous_assignment(symbol));
                            if entry.last_assignment_pos != i32::MAX {
                                entry.last_assignment_pos = if self
                                    .assignment_function(Some(node))?
                                    == self.assignment_function(Some(declaration))?
                                {
                                    self.extend_assignment_position(node, declaration)?
                                } else {
                                    i32::MAX
                                };
                            }
                            entry.has_definite_assignment |= kind == AssignmentKind::Definite;
                        }
                    }
                    continue;
                }
                Some(K::ExportSpecifier) => {
                    let specifier = read
                        .data_source()
                        .as_export_specifier()
                        .ok_or(ts_arena::Error::InvalidGraph)?;
                    let name = required(
                        specifier.property_name().or(read.name()),
                        "export specifier name",
                    )?;
                    let parent = required(read.parent(), "named exports")?;
                    let export = required(view.node(parent)?.parent(), "export declaration")?;
                    let export = view.node(export)?;
                    let export = export
                        .data_source()
                        .as_export_declaration()
                        .ok_or(ts_arena::Error::InvalidGraph)?;
                    if !specifier.is_type_only()
                        && !export.is_type_only()
                        && export.module_specifier().is_none()
                        && view.node(name)?.kind() != K::StringLiteral
                    {
                        // Existing resolver explicitly rejects alias/type-only
                        // resolution; never silently dereference an alias here.
                        if let Some(symbol) = self.resolve_entity_name(name, sf::VALUE, true)? {
                            if self.is_parameter_or_mutable_local_variable(symbol)? {
                                let entry = marked
                                    .entry(symbol)
                                    .or_insert_with(|| self.previous_assignment(symbol));
                                entry.last_assignment_pos = i32::MAX;
                            }
                        }
                    }
                    continue;
                }
                Some(
                    K::InterfaceDeclaration
                    | K::TypeAliasDeclaration
                    | K::JSTypeAliasDeclaration
                    | K::EnumDeclaration,
                ) => continue,
                _ => {}
            }
            if ts_ast::utilities::is_type_node(&read) {
                continue;
            }
            stack.extend(self.source_children(node)?.into_iter().rev());
        }
        Ok(marked)
    }

    // port: tsc/internal/checker/flow.go:Checker.extendAssignmentPosition
    fn extend_assignment_position(&self, node: NodeId, declaration: NodeId) -> Result<i32, Error> {
        let declaration_pos = self.ast(declaration)?.node(declaration)?.pos();
        let mut position = self.ast(node)?.node(node)?.pos();
        let mut current = Some(node);
        while let Some(node) = current {
            let read = self.ast(node)?.node(node)?;
            if read.pos() <= declaration_pos {
                break;
            }
            if matches!(
                read.kind().known(),
                Some(
                    K::VariableStatement
                        | K::ExpressionStatement
                        | K::IfStatement
                        | K::DoStatement
                        | K::WhileStatement
                        | K::ForStatement
                        | K::ForInStatement
                        | K::ForOfStatement
                        | K::WithStatement
                        | K::SwitchStatement
                        | K::TryStatement
                        | K::ClassDeclaration
                )
            ) {
                position = read.end();
            }
            current = read.parent();
        }
        Ok(position)
    }
}
