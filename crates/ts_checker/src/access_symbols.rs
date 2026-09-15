//! Symbol visibility, declaration ordering, and reference effects for value
//! accesses. Checks report independently from the property's resulting type.
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    check_flags as cf, modifier_flags as mf, node_flags as nf, symbol_flags as sf, SyntaxKind as K,
};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getDeclaringClass
    pub(crate) fn property_declaring_class(
        &mut self,
        property: SymbolId,
    ) -> Result<Option<TypeId>, Error> {
        let Some(parent) = self.parent_of_symbol(property)? else {
            return Ok(None);
        };
        if self.symbol(parent)?.flags() & sf::CLASS != 0 {
            self.get_declared_type_of_symbol(parent).map(Some)
        } else {
            Ok(None)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.forEachProperty
    pub(crate) fn underlying_access_properties(
        &mut self,
        property: SymbolId,
    ) -> Result<Vec<SymbolId>, Error> {
        if self.symbol(property)?.check_flags() & cf::SYNTHETIC == 0 {
            return Ok(vec![property]);
        }
        let ty = self
            .value_symbol_links
            .try_get(property)
            .and_then(|links| links.containing_type)
            .ok_or(Error::MissingLink("synthetic property containing type"))?;
        let name = self.symbol(property)?.name_to_owned();
        let types = self.types.compound_types(ty)?.clone();
        let mut result = Vec::new();
        for &ty in types.iter() {
            if let Some(property) = self.constituent_property(ty, name.as_bytes(), false)? {
                result.extend(self.underlying_access_properties(property)?);
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.isClassDerivedFromDeclaringClasses
    fn class_derived_from_property_owners(
        &mut self,
        class: TypeId,
        property: SymbolId,
        writing: bool,
    ) -> Result<bool, Error> {
        for property in self.underlying_access_properties(property)? {
            if self.property_modifiers_ex(property, writing)? & mf::PROTECTED != 0 {
                let Some(owner) = self.property_declaring_class(property)? else {
                    return Ok(false);
                };
                if !self.has_base_type(class, owner)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.isPropertyAccessible
    pub(crate) fn is_access_property_accessible(
        &mut self,
        node: NodeId,
        is_super: bool,
        writing: bool,
        containing: TypeId,
        property: SymbolId,
    ) -> Result<bool, Error> {
        if self.types.flags(containing)? & tf::ANY != 0 {
            return Ok(true);
        }
        if let Some(declaration) = self.symbol(property)?.value_declaration() {
            let view = self.ast(declaration)?;
            if ts_ast::utilities::is_private_identifier_class_element_declaration(
                view,
                declaration,
            )? {
                let class = ts_ast::utilities::get_containing_class(view, declaration)?
                    .ok_or(Error::MissingLink("private property class"))?;
                return Ok(
                    self.ast(node)?.node(node)?.flags() & nf::OPTIONAL_CHAIN == 0
                        && ts_ast::utilities::is_node_descendant_of(
                            self.ast(node)?,
                            Some(node),
                            Some(class),
                        )?,
                );
            }
        }
        self.check_access_property_accessibility(
            node, is_super, writing, containing, property, None,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.checkPropertyAccessibilityAtLocation
    pub(crate) fn check_access_property_accessibility(
        &mut self,
        node: NodeId,
        is_super: bool,
        writing: bool,
        mut containing: TypeId,
        property: SymbolId,
        error_node: Option<NodeId>,
    ) -> Result<bool, Error> {
        let flags = self.property_modifiers_ex(property, writing)?;
        if is_super {
            if flags & mf::ABSTRACT != 0 {
                if let Some(error_node) = error_node {
                    let name = self.symbol_to_string(property)?;
                    let owner = self
                        .property_declaring_class(property)?
                        .ok_or(Error::MissingLink("abstract property class"))?;
                    let class = self.type_to_string(owner, crate::type_display::DEFAULT_FLAGS)?;
                    self.error_at(
                        Some(error_node),
                        d::Abstract_method_0_in_class_1_cannot_be_accessed_via_super_expression,
                        vec![name, class],
                    )?;
                }
                return Ok(false);
            }
            if flags & mf::STATIC == 0 {
                for declaration in self
                    .symbol_declarations(property)?
                    .iter()
                    .flatten()
                    .collect::<Vec<_>>()
                {
                    if self.is_class_instance_property(declaration)? {
                        if let Some(error_node) = error_node {
                            let name = self.symbol_to_string(property)?;
                            self.error_at(Some(error_node),d::Class_field_0_defined_by_the_parent_class_is_not_accessible_in_the_child_class_via_super,vec![name])?;
                        }
                        return Ok(false);
                    }
                }
            }
        }
        if flags & mf::ABSTRACT != 0 {
            let mut nonmethod = false;
            for property in self.underlying_access_properties(property)? {
                nonmethod |= self.symbol(property)?.flags() & sf::METHOD == 0;
            }
            // Referencing abstract properties within their own constructors is not allowed
            let this = self.is_this_property(node)?
                || self.is_this_initialized_object_binding_expression(node)?
                || self.is_binding_element_of_this_initialized_declaration(node)?;
            if nonmethod && this {
                if let Some(parent) = self.parent_of_symbol(property)? {
                    if self.symbol(parent)?.flags() & sf::CLASS != 0
                        && self.access_used_during_class_initialization(node)?
                    {
                        if let Some(error_node) = error_node {
                            let name = self.symbol_to_string(property)?;
                            let class = self.symbol_to_string(parent)?;
                            self.error_at(Some(error_node),d::Abstract_property_0_in_class_1_cannot_be_accessed_in_the_constructor,vec![name,class])?;
                        }
                        return Ok(false);
                    }
                }
            }
        }
        if flags & mf::NON_PUBLIC_ACCESSIBILITY_MODIFIER == 0 {
            return Ok(true);
        }
        if flags & mf::PRIVATE != 0 {
            let declaration = match self.parent_of_symbol(property)? {
                Some(parent) => self.class_declaration(parent)?,
                None => None,
            };
            let within = declaration
                .map(|class| self.node_within_class(node, class))
                .transpose()?
                .unwrap_or(false);
            if !within {
                if let Some(error_node) = error_node {
                    let owner = self
                        .property_declaring_class(property)?
                        .unwrap_or(containing);
                    let name = self.symbol_to_string(property)?;
                    let class = self.type_to_string(owner, crate::type_display::DEFAULT_FLAGS)?;
                    self.error_at(
                        Some(error_node),
                        d::Property_0_is_private_and_only_accessible_within_class_1,
                        vec![name, class],
                    )?;
                }
                return Ok(false);
            }
            return Ok(true);
        }
        if is_super {
            return Ok(true);
        }
        let mut enclosing = None;
        let mut container = ts_ast::utilities::get_containing_class(self.ast(node)?, node)?;
        while let Some(current) = container {
            let symbol = self
                .get_symbol_of_declaration(current)?
                .ok_or(Error::MissingLink("enclosing class symbol"))?;
            let class = self.get_declared_type_of_symbol(symbol)?;
            if self.class_derived_from_property_owners(class, property, writing)? {
                enclosing = Some(class);
                break;
            }
            container = ts_ast::utilities::get_containing_class(self.ast(current)?, current)?;
        }
        if enclosing.is_none() {
            if let Some(class) = self.access_enclosing_class_from_this_parameter(node)? {
                if self.class_derived_from_property_owners(class, property, writing)? {
                    enclosing = Some(class);
                }
            }
            if flags & mf::STATIC != 0 || enclosing.is_none() {
                if let Some(error_node) = error_node {
                    let owner = self
                        .property_declaring_class(property)?
                        .unwrap_or(containing);
                    let name = self.symbol_to_string(property)?;
                    let class = self.type_to_string(owner, crate::type_display::DEFAULT_FLAGS)?;
                    self.error_at(Some(error_node),d::Property_0_is_protected_and_only_accessible_within_class_1_and_its_subclasses,vec![name,class])?;
                }
                return Ok(false);
            }
        }
        if flags & mf::STATIC != 0 {
            return Ok(true);
        }
        let enclosing = enclosing.ok_or(Error::MissingLink("protected enclosing class"))?;
        if self.types.flags(containing)? & tf::TYPE_PARAMETER != 0 {
            let constraint = if self.types.type_parameter(containing)?.is_this_type {
                self.constraint_of_type_parameter(containing)?
            } else {
                self.base_constraint_of_type(containing)?
            };
            let Some(constraint) = constraint else {
                return Ok(false);
            };
            containing = constraint;
        }
        if !self.has_base_type(containing, enclosing)? {
            if let Some(error_node) = error_node {
                let name = self.symbol_to_string(property)?;
                let enclosing =
                    self.type_to_string(enclosing, crate::type_display::DEFAULT_FLAGS)?;
                let containing =
                    self.type_to_string(containing, crate::type_display::DEFAULT_FLAGS)?;
                self.error_at(Some(error_node),d::Property_0_is_protected_and_only_accessible_through_an_instance_of_class_1_This_is_an_instance_of_class_2,vec![name,enclosing,containing])?;
            }
            return Ok(false);
        }
        Ok(true)
    }

    // port: tsc/internal/checker/utilities.go:isClassInstanceProperty
    fn is_class_instance_property(&self, node: NodeId) -> Result<bool, Error> {
        use ts_ast::{
            get_element_or_property_access_name, is_bindable_static_access_expression,
            is_bindable_static_name_expression,
        };
        let view = self.ast(node)?;
        let read = view.node(node)?;
        if read.flags() & nf::JAVA_SCRIPT_FILE != 0
            && ts_ast::utilities_tail::is_expando_property_declaration(Some(&read))
        {
            let left = read
                .data_source()
                .as_binary_expression()
                .expect("expando declarations are binary expressions")
                .left()
                .ok_or(Error::MissingLink("expando left"))?;
            let mut prototype = false;
            if is_bindable_static_access_expression(view, left, false)? {
                let receiver = view
                    .node(left)?
                    .expression()
                    .ok_or(Error::MissingLink("expando receiver"))?;
                if is_bindable_static_access_expression(view, receiver, false)? {
                    if let Some(name) = get_element_or_property_access_name(view, receiver)? {
                        prototype = view.node_text(name)?.as_bytes() == b"prototype";
                    }
                }
            }
            return Ok(!prototype && !is_bindable_static_name_expression(view, left, true)?);
        }
        let Some(parent) = read.parent() else {
            return Ok(false);
        };
        Ok(matches!(
            view.node(parent)?.kind().known(),
            Some(K::ClassDeclaration | K::ClassExpression)
        ) && read.kind() == K::PropertyDeclaration
            && read.modifier_flags(view)? & mf::ACCESSOR == 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.getEnclosingClassFromThisParameter
    fn access_enclosing_class_from_this_parameter(
        &mut self,
        node: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        let container = ts_ast::get_this_container(self.ast(node)?, node, false, false)?;
        let read = self.ast(container)?.node(container)?;
        if !ts_ast::utilities::is_function_like(Some(&read)) {
            return Ok(None);
        }
        let signature = self.signature_from_declaration(container)?;
        let parameter = self.signatures.get(signature)?.this_parameter;
        let mut ty = None;
        if let Some(parameter) = parameter {
            if let Some(declaration) = self.symbol(parameter)?.value_declaration() {
                if let Some(annotation) = self.ast(declaration)?.node(declaration)?.type_node() {
                    ty = Some(self.get_type_from_type_node(annotation)?);
                }
            }
        }
        if let Some(current) = ty {
            if self.types.flags(current)? & tf::TYPE_PARAMETER != 0 {
                ty = self.constraint_of_type_parameter(current)?;
            }
        } else {
            ty = self.contextual_this_parameter_type(container)?;
        }
        match ty {
            Some(ty)
                if self.types.object_flags(ty)? & (of::CLASS_OR_INTERFACE | of::REFERENCE) != 0 =>
            {
                Ok(Some(if self.types.object_flags(ty)? & of::REFERENCE != 0 {
                    self.types.target(ty)?
                } else {
                    ty
                }))
            }
            _ => Ok(None),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.isNodeUsedDuringClassInitialization
    fn access_used_during_class_initialization(&self, mut node: NodeId) -> Result<bool, Error> {
        loop {
            let read = self.ast(node)?.node(node)?;
            if read.kind() == K::PropertyDeclaration
                || read.kind() == K::Constructor && read.body().is_some()
            {
                return Ok(true);
            }
            if ts_ast::utilities::is_class_like(&read)
                || ts_ast::utilities::is_function_like(Some(&read))
            {
                return Ok(false);
            }
            let Some(parent) = read.parent() else {
                return Ok(false);
            };
            node = parent;
        }
    }

    /// Property access passes the receiver's resolved symbol as `parent`;
    /// element access passes the apparent object type's symbol.
    // port: tsc/internal/checker/checker.go:Checker.markPropertyAsReferenced
    pub(crate) fn mark_access_property_referenced(
        &mut self,
        property: SymbolId,
        node: NodeId,
        left: NodeId,
        parent: Option<SymbolId>,
    ) -> Result<(), Error> {
        // port: tsc/internal/checker/checker.go:Checker.isSelfTypeAccess
        let view = self.ast(left)?;
        let self_access = view.node(left)?.kind() == K::ThisKeyword
            || match parent {
                Some(parent) if is_entity_name_expression(view, left)? => {
                    let first = ts_ast::utilities_middle::get_first_identifier(view, left)?;
                    parent == self.resolved_value_symbol(first)?
                }
                _ => false,
            };
        self.mark_property_as_referenced(property, Some(node), self_access)
    }

    // port: tsc/internal/checker/checker.go:Checker.markPropertyAsReferenced
    pub(crate) fn mark_property_as_referenced(
        &mut self,
        property: SymbolId,
        node: Option<NodeId>,
        self_access: bool,
    ) -> Result<(), Error> {
        let read = self.symbol(property)?;
        let flags = read.flags();
        let Some(declaration) = read.value_declaration() else {
            return Ok(());
        };
        if flags & sf::CLASS_MEMBER == 0 {
            return Ok(());
        }
        let read = self.ast(declaration)?.node(declaration)?;
        let private = read.modifier_flags(self.ast(declaration)?)? & mf::PRIVATE != 0
            || read
                .name()
                .map(|name| {
                    self.ast(name)?
                        .node(name)
                        .map(|read| read.kind() == K::PrivateIdentifier)
                        .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or(false);
        if !private {
            return Ok(());
        }
        let write_only = match node {
            Some(node) => ts_ast::utilities::is_write_only_access(self.ast(node)?, node)?,
            None => false,
        };
        if write_only && flags & sf::SET_ACCESSOR == 0 {
            return Ok(());
        }
        if self_access {
            let mut current = node;
            while let Some(node) = current {
                let read = self.ast(node)?.node(node)?;
                // FindAncestor(node, IsFunctionLikeDeclaration): signatures and function types pass.
                if ts_ast::utilities::is_function_like_declaration(Some(&read)) {
                    if self.get_symbol_of_declaration(node)? == Some(property) {
                        return Ok(());
                    }
                    break;
                }
                current = read.parent();
            }
        }
        let target = self.target_symbol(property)?;
        *self.query.references.get_or_default(target) |= sf::ALL;
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkPropertyNotUsedBeforeDeclaration
    pub(crate) fn check_property_use_before_declaration(
        &mut self,
        property: SymbolId,
        node: NodeId,
        right: NodeId,
    ) -> Result<(), Error> {
        let Some(declaration) = self.symbol(property)?.value_declaration() else {
            return Ok(());
        };
        let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
            .ok_or(Error::MissingLink("property access source"))?;
        if self.ast(source)?.source_file(source)?.is_declaration_file {
            return Ok(());
        }
        let read = self.ast(declaration)?.node(declaration)?;
        let optional = read.kind() == K::PropertyDeclaration
            && read.modifier_flags(self.ast(declaration)?)? & mf::ACCESSOR == 0
            && read.question_token(self.ast(declaration)?)?.is_some();
        let method = read.kind() == K::MethodDeclaration
            && read.modifier_flags(self.ast(declaration)?)? & mf::STATIC != 0;
        let kind = read.kind();
        let ambient = read.flags() & nf::AMBIENT != 0;
        let left = self.access_receiver(node)?;
        let chained = left
            .map(|left| {
                self.ast(left)?
                    .node(left)
                    .map(|read| {
                        matches!(
                            read.kind().known(),
                            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
                        )
                    })
                    .map_err(Error::from)
            })
            .transpose()?
            .unwrap_or(false);
        let mut message = None;
        if self.access_in_property_initializer(node)?
            && !optional
            && !chained
            && !self.name_declared_before_use(declaration, right)?
            && !method
        {
            let mut inherited = false;
            if let Some(parent) = self.parent_of_symbol(property)? {
                if self.symbol(parent)?.flags() & sf::CLASS != 0 {
                    let class = self.get_declared_type_of_symbol(parent)?;
                    let bases = self.interface_base_types(class)?;
                    if let Some(&base) = bases.first() {
                        let name = self.symbol(property)?.name_to_owned();
                        if let Some(property) =
                            self.constituent_property(base, name.as_bytes(), false)?
                        {
                            inherited = self.symbol(property)?.value_declaration().is_some();
                        }
                    }
                }
            }
            if self.program()?.host.options().use_define_for_class_fields() || !inherited {
                message = Some(d::Property_0_is_used_before_its_initialization);
            }
        } else if kind == K::ClassDeclaration
            && !ambient
            && !self.name_declared_before_use(declaration, right)?
        {
            let parent = self.ast(node)?.node(node)?.parent();
            if !parent
                .map(|parent| {
                    self.ast(parent)?
                        .node(parent)
                        .map(|read| read.kind() == K::TypeReference)
                        .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or(false)
            {
                message = Some(d::Class_0_used_before_its_declaration);
            }
        }
        if let Some(message) = message {
            let name = self.ast(right)?.node_text(right)?.into_js_string();
            let mut diagnostic =
                self.diagnostic_for_node(Some(right), message, vec![name.clone()])?;
            diagnostic
                .related_information
                .push(std::sync::Arc::new(self.diagnostic_for_node(
                    Some(declaration),
                    d::X_0_is_declared_here,
                    vec![name],
                )?));
            self.add_diagnostic(diagnostic)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.isInPropertyInitializerOrClassStaticBlock
    fn access_in_property_initializer(&self, mut node: NodeId) -> Result<bool, Error> {
        loop {
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(K::PropertyDeclaration | K::ClassStaticBlockDeclaration) => return Ok(true),
                Some(K::TypeQuery | K::JsxClosingElement | K::ArrowFunction) => return Ok(false),
                Some(K::Block) => {
                    if let Some(parent) = read.parent() {
                        let parent = self.ast(parent)?.node(parent)?;
                        if ts_ast::utilities::is_function_like(Some(&parent))
                            && parent.kind() != K::ArrowFunction
                        {
                            return Ok(false);
                        }
                    }
                }
                _ => {}
            }
            let Some(parent) = read.parent() else {
                return Ok(false);
            };
            node = parent;
        }
    }
}

impl CheckerState {
    // port: tsc/internal/checker/utilities.go:isThisProperty
    fn is_this_property(&self, node: NodeId) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        if !matches!(
            read.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            return Ok(false);
        }
        Ok(read
            .expression()
            .map(|expression| Ok::<_, Error>(view.node(expression)?.kind() == K::ThisKeyword))
            .transpose()?
            .unwrap_or(false))
    }

    // port: tsc/internal/checker/utilities.go:isThisInitializedObjectBindingExpression
    fn is_this_initialized_object_binding_expression(&self, node: NodeId) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        if !matches!(
            read.kind().known(),
            Some(K::ShorthandPropertyAssignment | K::PropertyAssignment)
        ) {
            return Ok(false);
        }
        let Some(literal) = read.parent() else {
            return Ok(false);
        };
        let Some(assignment) = view.node(literal)?.parent() else {
            return Ok(false);
        };
        let assignment_read = view.node(assignment)?;
        let Some(data) = assignment_read.data_source().as_binary_expression() else {
            return Ok(false);
        };
        let operator = data
            .operator_token()
            .map(|token| view.node(token).map(|read| read.kind()))
            .transpose()?;
        let right = data
            .right()
            .map(|right| view.node(right).map(|read| read.kind()))
            .transpose()?;
        Ok(operator == Some(K::EqualsToken.into()) && right == Some(K::ThisKeyword.into()))
    }

    /// `IsObjectBindingPattern(location.Parent) && isThisInitializedDeclaration(location.Parent.Parent)`
    // port: tsc/internal/checker/utilities.go:isThisInitializedDeclaration
    fn is_binding_element_of_this_initialized_declaration(
        &self,
        node: NodeId,
    ) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let Some(pattern) = view.node(node)?.parent() else {
            return Ok(false);
        };
        let pattern_read = view.node(pattern)?;
        if pattern_read.kind() != K::ObjectBindingPattern {
            return Ok(false);
        }
        let Some(declaration) = pattern_read.parent() else {
            return Ok(false);
        };
        let declaration_read = view.node(declaration)?;
        if declaration_read.kind() != K::VariableDeclaration {
            return Ok(false);
        }
        Ok(declaration_read
            .initializer()
            .map(|initializer| Ok::<_, Error>(view.node(initializer)?.kind() == K::ThisKeyword))
            .transpose()?
            .unwrap_or(false))
    }
}

// port: tsc/internal/ast/utilities.go:IsEntityNameExpression
fn is_entity_name_expression(view: ts_ast::AstView<'_>, mut node: NodeId) -> Result<bool, Error> {
    loop {
        let read = view.node(node)?;
        match read.kind().known() {
            Some(K::Identifier) => return Ok(true),
            Some(K::PropertyAccessExpression) => {
                let name = read
                    .name()
                    .ok_or(Error::MissingLink("property access name"))?;
                if view.node(name)?.kind() != K::Identifier {
                    return Ok(false);
                }
                node = read
                    .expression()
                    .ok_or(Error::MissingLink("property access receiver"))?;
            }
            _ => return Ok(false),
        }
    }
}
