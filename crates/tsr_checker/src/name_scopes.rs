//! Value declaration ordering and deferred uses of block-scoped names.

use crate::{CheckerState, Error};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{node_flags as nf, symbol_flags as sf, utilities as ast, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkResolvedBlockScopedVariable
    pub(crate) fn check_resolved_block_scoped_variable(
        &mut self,
        symbol: SymbolId,
        usage: NodeId,
    ) -> Result<(), Error> {
        let flags = self.symbol(symbol)?.flags();
        if flags & sf::CLASS != 0
            && flags & (sf::FUNCTION | sf::FUNCTION_SCOPED_VARIABLE | sf::ASSIGNMENT) != 0
        {
            return Ok(());
        }
        let mut declaration = None;
        for node in self.symbol_declarations(symbol)?.iter().flatten() {
            let view = self.ast(node)?;
            if ast::is_block_or_catch_scoped(view, node)?
                || ast::is_class_like(&view.node(node)?)
                || view.node(node)?.kind() == K::EnumDeclaration
            {
                declaration = Some(node);
                break;
            }
        }
        let declaration = declaration.ok_or(Error::MissingLink("block-scoped declaration"))?;
        if self.node(declaration)?.flags() & nf::AMBIENT != 0
            || self.name_declared_before_use(declaration, usage)?
        {
            return Ok(());
        }
        let message = if flags & sf::BLOCK_SCOPED_VARIABLE != 0 {
            Some(tsr_diagnostics::Block_scoped_variable_0_used_before_its_declaration)
        } else if flags & sf::CLASS != 0 {
            Some(tsr_diagnostics::Class_0_used_before_its_declaration)
        } else if flags & sf::REGULAR_ENUM != 0 || self.program()?.host.options().isolated_modules()
        {
            Some(tsr_diagnostics::Enum_0_used_before_its_declaration)
        } else {
            None
        };
        if let Some(message) = message {
            let name = tsr_scanner::declaration_name_to_string(
                self.ast(declaration)?,
                self.node(declaration)?.name(),
            )?;
            let mut diagnostic =
                self.diagnostic_for_node(Some(usage), message, vec![name.clone()])?;
            diagnostic
                .related_information
                .push(std::sync::Arc::new(self.diagnostic_for_node(
                    Some(declaration),
                    tsr_diagnostics::X_0_is_declared_here,
                    vec![name],
                )?));
            self.add_diagnostic(diagnostic)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.isInAmbientOrTypeNode
    pub(crate) fn in_ambient_or_type_node(&self, node: NodeId) -> Result<bool, Error> {
        if self.node(node)?.flags() & nf::AMBIENT != 0 {
            return Ok(true);
        }
        let mut ancestor = Some(node);
        while let Some(node) = ancestor {
            let read = self.node(node)?;
            if matches!(
                read.kind().known(),
                Some(
                    K::InterfaceDeclaration
                        | K::TypeAliasDeclaration
                        | K::JSTypeAliasDeclaration
                        | K::TypeLiteral
                )
            ) {
                return Ok(true);
            }
            ancestor = read.parent();
        }
        Ok(false)
    }

    // port: tsc/internal/checker/utilities.go:IsInTypeQuery
    pub(crate) fn in_type_query(&self, node: NodeId) -> Result<bool, Error> {
        let mut ancestor = Some(node);
        while let Some(node) = ancestor {
            let read = self.node(node)?;
            if read.kind() == K::TypeQuery {
                return Ok(true);
            }
            if !matches!(read.kind().known(), Some(K::Identifier | K::QualifiedName)) {
                return Ok(false);
            }
            ancestor = read.parent();
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isBlockScopedNameDeclaredBeforeUse
    pub(crate) fn name_declared_before_use(
        &mut self,
        declaration: NodeId,
        usage: NodeId,
    ) -> Result<bool, Error> {
        let declared_file =
            ast::get_source_file_of_node(self.ast(declaration)?, Some(declaration))?;
        let used_file = ast::get_source_file_of_node(self.ast(usage)?, Some(usage))?;
        let scope = self.declaration_block_scope(declaration)?;
        if declared_file != used_file
            || self.node(usage)?.flags() & nf::JS_DOC != 0
            || self.in_type_query(usage)?
            || self.in_ambient_or_type_node(usage)?
        {
            return Ok(true);
        }
        let read = self.node(declaration)?;
        let kind = read.kind();
        let property = kind == K::PropertyDeclaration;
        let property_without_initializer =
            if property && self.usage_is_this_property(usage)? && read.initializer().is_none() {
                read.postfix_token()
                    .map(|token| {
                        self.ast(token)?
                            .node(token)
                            .map(|read| read.kind() != K::ExclamationToken)
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(true)
            } else {
                false
            };
        if read.pos() <= self.node(usage)?.pos() && !property_without_initializer {
            match kind.known() {
                Some(K::BindingElement) => {
                    let view = self.ast(usage)?;
                    if let Some(element) =
                        ast::find_ancestor_kind(view, Some(usage), K::BindingElement.into())?
                    {
                        let left =
                            ast::find_ancestor_kind(view, Some(element), K::BindingElement.into())?;
                        let right = ast::find_ancestor_kind(
                            self.ast(declaration)?,
                            Some(declaration),
                            K::BindingElement.into(),
                        )?;
                        return Ok(left != right
                            || self.node(declaration)?.pos() < self.node(element)?.pos());
                    }
                    let root = ast::find_ancestor_kind(
                        self.ast(declaration)?,
                        Some(declaration),
                        K::VariableDeclaration.into(),
                    )?
                    .ok_or(Error::MissingLink("binding variable declaration"))?;
                    return self.name_declared_before_use(root, usage);
                }
                Some(K::VariableDeclaration) => {
                    let parent = read
                        .parent()
                        .ok_or(Error::MissingLink("declaration list"))?;
                    let statement = self
                        .ast(parent)?
                        .node(parent)?
                        .parent()
                        .ok_or(Error::MissingLink("declaration statement"))?;
                    let kind = self.node(statement)?.kind();
                    if matches!(
                        kind.known(),
                        Some(K::VariableStatement | K::ForStatement | K::ForOfStatement)
                    ) && self.same_scope_descendant(usage, Some(declaration), scope)?
                    {
                        return Ok(false);
                    }
                    if matches!(kind.known(), Some(K::ForInStatement | K::ForOfStatement))
                        && self.same_scope_descendant(
                            usage,
                            self.node(statement)?.expression(),
                            scope,
                        )?
                    {
                        return Ok(false);
                    }
                    return Ok(true);
                }
                Some(K::ClassDeclaration | K::ClassExpression) => {
                    return self.class_name_before_use(declaration, usage)
                }
                Some(K::PropertyDeclaration) => {
                    return Ok(!self.property_immediately_referenced(declaration, usage, false)?)
                }
                Some(K::Parameter) => {
                    let parent = read
                        .parent()
                        .ok_or(Error::MissingLink("parameter parent"))?;
                    if ast::is_parameter_property_declaration(
                        self.ast(declaration)?,
                        declaration,
                        parent,
                    )? {
                        return Ok(
                            !(self.program()?.host.options().emit_standard_class_fields()
                                && self.containing_name_class(declaration)?
                                    == self.containing_name_class(usage)?
                                && self.used_in_function_or_instance_property(
                                    usage,
                                    declaration,
                                    scope,
                                )?),
                        );
                    }
                }
                _ => {}
            }
            return Ok(true);
        }
        if let Some(parent) = self.node(usage)?.parent() {
            let read = self.node(parent)?;
            if read.kind() == K::ExportSpecifier
                || read
                    .data_source()
                    .as_export_assignment()
                    .is_some_and(|data| data.is_export_equals())
            {
                return Ok(true);
            }
        }
        if self
            .ast(usage)?
            .node(usage)?
            .data_source()
            .as_export_assignment()
            .is_some_and(|data| data.is_export_equals())
        {
            return Ok(true);
        }
        if self.used_in_function_or_instance_property(usage, declaration, scope)? {
            let parent = self.node(declaration)?.parent();
            let parameter_property = parent
                .map(|parent| {
                    ast::is_parameter_property_declaration(
                        self.ast(declaration)?,
                        declaration,
                        parent,
                    )
                    .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or(false);
            if self.program()?.host.options().emit_standard_class_fields()
                && self.containing_name_class(declaration)?.is_some()
                && (property || parameter_property)
            {
                return Ok(!self.property_immediately_referenced(declaration, usage, true)?);
            }
            return Ok(true);
        }
        Ok(false)
    }

    // port: tsc/internal/ast/utilities.go:GetEnclosingBlockScopeContainer
    // port: tsc/internal/ast/utilities.go:IsBlockScope
    fn declaration_block_scope(&self, node: NodeId) -> Result<NodeId, Error> {
        let mut current = self.node(node)?.parent();
        while let Some(node) = current {
            let read = self.node(node)?;
            match read.kind().known() {
                Some(
                    K::SourceFile
                    | K::CaseBlock
                    | K::CatchClause
                    | K::ModuleDeclaration
                    | K::ForStatement
                    | K::ForInStatement
                    | K::ForOfStatement
                    | K::Constructor
                    | K::MethodDeclaration
                    | K::GetAccessor
                    | K::SetAccessor
                    | K::FunctionDeclaration
                    | K::FunctionExpression
                    | K::ArrowFunction
                    | K::PropertyDeclaration
                    | K::ClassStaticBlockDeclaration,
                ) => return Ok(node),
                Some(K::Block) => {
                    let parent = read.parent().ok_or(Error::MissingLink("block parent"))?;
                    let parent = self.node(parent)?;
                    if !ast::is_function_like(Some(&parent))
                        && parent.kind() != K::ClassStaticBlockDeclaration
                    {
                        return Ok(node);
                    }
                }
                _ => {}
            }
            current = read.parent();
        }
        Err(Error::MissingLink("declaration block scope"))
    }
    fn containing_name_class(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        Ok(ast::get_containing_class(self.ast(node)?, node)?)
    }
    fn usage_is_this_property(&self, usage: NodeId) -> Result<bool, Error> {
        let Some(parent) = self.node(usage)?.parent() else {
            return Ok(false);
        };
        let read = self.node(parent)?;
        if read.kind() != K::PropertyAccessExpression {
            return Ok(false);
        }
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("this property receiver"))?;
        Ok(self.node(expression)?.kind() == K::ThisKeyword)
    }
    fn class_name_before_use(&self, declaration: NodeId, usage: NodeId) -> Result<bool, Error> {
        let legacy = self
            .program()?
            .host
            .options()
            .experimental_decorators
            .is_true();
        let mut container = Some(usage);
        while let Some(node) = container {
            if node == declaration {
                return Ok(true);
            }
            let read = self.node(node)?;
            if let Some(parent) = read.parent() {
                let parent_read = self.node(parent)?;
                let grandparent = parent_read.parent();
                if read.kind() == K::ComputedPropertyName && grandparent == Some(declaration) {
                    return Ok(false);
                }
                if !legacy && read.kind() == K::Decorator {
                    let class_member = matches!(
                        parent_read.kind().known(),
                        Some(
                            K::MethodDeclaration
                                | K::GetAccessor
                                | K::SetAccessor
                                | K::PropertyDeclaration
                        )
                    ) && grandparent == Some(declaration);
                    let parameter = parent_read.kind() == K::Parameter
                        && grandparent
                            .map(|p| {
                                self.ast(p)?
                                    .node(p)
                                    .map(|p| p.parent() == Some(declaration))
                                    .map_err(Error::from)
                            })
                            .transpose()?
                            .unwrap_or(false);
                    if parent == declaration || class_member || parameter {
                        let mut nested = Some(usage);
                        while let Some(current) = nested {
                            if current == node {
                                return Ok(false);
                            }
                            let read = self.node(current)?;
                            if ast::is_function_like(Some(&read))
                                && tsr_ast::get_immediately_invoked_function_expression(
                                    self.ast(current)?,
                                    current,
                                )?
                                .is_none()
                            {
                                return Ok(true);
                            }
                            nested = read.parent();
                        }
                        return Ok(false);
                    }
                }
            }
            container = read.parent();
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:isSameScopeDescendentOf
    fn same_scope_descendant(
        &self,
        usage: NodeId,
        parent: Option<NodeId>,
        stop: NodeId,
    ) -> Result<bool, Error> {
        let Some(parent) = parent else {
            return Ok(false);
        };
        let mut ancestor = Some(usage);
        while let Some(node) = ancestor {
            if node == parent {
                return Ok(true);
            }
            let read = self.node(node)?;
            if node == stop
                || ast::is_function_like(Some(&read))
                    && (tsr_ast::get_immediately_invoked_function_expression(
                        self.ast(node)?,
                        node,
                    )?
                    .is_none()
                        || self.body_function_flags(node)? != (false, false))
            {
                return Ok(false);
            }
            ancestor = read.parent();
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:isPropertyImmediatelyReferencedWithinDeclaration
    fn property_immediately_referenced(
        &self,
        declaration: NodeId,
        usage: NodeId,
        stop_at_any: bool,
    ) -> Result<bool, Error> {
        if self.node(usage)?.end() > self.node(declaration)?.end() {
            return Ok(false);
        }
        let mut ancestor = Some(usage);
        while let Some(node) = ancestor {
            if node == declaration {
                break;
            }
            let read = self.node(node)?;
            match read.kind().known() {
                Some(K::ArrowFunction) => return Ok(false),
                Some(K::PropertyDeclaration) => {
                    if !stop_at_any {
                        return Ok(false);
                    }
                    let declared = self.node(declaration)?;
                    let parent = declared
                        .parent()
                        .ok_or(Error::MissingLink("property declaration parent"))?;
                    return Ok(declared.kind() == K::PropertyDeclaration
                        && read.parent() == Some(parent)
                        || ast::is_parameter_property_declaration(
                            self.ast(declaration)?,
                            declaration,
                            parent,
                        )? && read.parent() == self.node(parent)?.parent());
                }
                Some(K::Block) => {
                    if let Some(parent) = read.parent() {
                        if matches!(
                            self.node(parent)?.kind().known(),
                            Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor)
                        ) {
                            return Ok(false);
                        }
                    }
                }
                _ => {}
            }
            ancestor = read.parent();
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.isUsedInFunctionOrInstanceProperty
    fn used_in_function_or_instance_property(
        &mut self,
        usage: NodeId,
        declaration: NodeId,
        scope: NodeId,
    ) -> Result<bool, Error> {
        let mut ancestor = Some(usage);
        while let Some(node) = ancestor {
            if node == scope {
                return Ok(false);
            }
            let read = self.node(node)?;
            if ast::is_function_like(Some(&read))
                && tsr_ast::get_immediately_invoked_function_expression(self.ast(node)?, node)?
                    .is_none()
            {
                return Ok(true);
            }
            if read.kind() == K::ClassStaticBlockDeclaration {
                return Ok(self.node(declaration)?.pos() < self.node(usage)?.pos());
            }
            let parent = read.parent();
            if let Some(parent) = parent {
                let read = self.node(parent)?;
                if read.kind() == K::PropertyDeclaration && read.initializer() == Some(node) {
                    if read.modifier_flags(self.ast(parent)?)? & tsr_ast::modifier_flags::STATIC
                        != 0
                    {
                        let declared = self.node(declaration)?;
                        if declared.kind() == K::MethodDeclaration {
                            return Ok(true);
                        }
                        if declared.kind() == K::PropertyDeclaration
                            && self.containing_name_class(usage)?
                                == self.containing_name_class(declaration)?
                        {
                            let name = declared
                                .name()
                                .ok_or(Error::MissingLink("static property name"))?;
                            let class = declared
                                .parent()
                                .ok_or(Error::MissingLink("static property class"))?;
                            if matches!(
                                self.node(name)?.kind().known(),
                                Some(K::Identifier | K::PrivateIdentifier)
                            ) {
                                let symbol = self
                                    .get_symbol_of_declaration(declaration)?
                                    .ok_or(Error::MissingLink("static property symbol"))?;
                                let ty = self.get_type_of_symbol(symbol)?;
                                let blocks =
                                    self.source_list(class, self.node(class)?.member_list())?
                                        .into_iter()
                                        .filter_map(|member| {
                                            match self.ast(member).and_then(|view| {
                                                view.node(member).map_err(Error::from)
                                            }) {
                                                Ok(read)
                                                    if read.kind()
                                                        == K::ClassStaticBlockDeclaration =>
                                                {
                                                    Some(Ok(member))
                                                }
                                                Ok(_) => None,
                                                Err(error) => Some(Err(error)),
                                            }
                                        })
                                        .collect::<Result<Vec<_>, Error>>()?;
                                if self.property_initialized_in_static_blocks(
                                    name,
                                    ty,
                                    &blocks,
                                    self.node(class)?.pos(),
                                    self.node(node)?.pos(),
                                )? {
                                    return Ok(true);
                                }
                            }
                        }
                    } else {
                        let declared = self.node(declaration)?;
                        let instance = declared.kind() == K::PropertyDeclaration
                            && declared.modifier_flags(self.ast(declaration)?)?
                                & tsr_ast::modifier_flags::STATIC
                                == 0;
                        if !instance
                            || self.containing_name_class(usage)?
                                != self.containing_name_class(declaration)?
                        {
                            return Ok(true);
                        }
                    }
                }
                let read = self.node(parent)?;
                if read.kind() == K::Decorator && read.expression() == Some(node) {
                    let decorated = read.parent().ok_or(Error::MissingLink("decorated node"))?;
                    let decorated = self.node(decorated)?;
                    if matches!(
                        decorated.kind().known(),
                        Some(K::Parameter | K::MethodDeclaration)
                    ) {
                        let mut outer = decorated
                            .parent()
                            .ok_or(Error::MissingLink("decorated container"))?;
                        if decorated.kind() == K::Parameter {
                            outer = self
                                .ast(outer)?
                                .node(outer)?
                                .parent()
                                .ok_or(Error::MissingLink("parameter decorated class"))?;
                        }
                        return self.used_in_function_or_instance_property(
                            outer,
                            declaration,
                            scope,
                        );
                    }
                }
            }
            ancestor = parent;
        }
        Ok(false)
    }
}
