//! Expandable hover (`tsc/internal/checker/nodebuilder_hover.go`): declaration
//! nodes for a class, interface, enum or module symbol, built directly for
//! hover rather than through declaration emit, with the builder's expansion
//! truncation. The entry points are `NodeBuilder.ExpandSymbolForHover`
//! (`nodebuilder.go`) and `Checker.ExpandSymbolForHover` (`printer.go`).

use super::NodeBuilder;
use crate::{signature_flags, CheckerState, Error, IndexInfoId, SignatureId, TypeId};
use tsr_arena::SymbolId;
use tsr_ast::{
    modifier_flags as mf, node_flags, symbol_flags as sf, Factory, FactoryMethods, JsString,
    NodeId, NodeListId, SyntaxKind as K,
};
use tsr_printer::EmitTextWriter;

// port: tsc/internal/checker/nodebuilder_hover.go:isHashPrivate
fn is_hash_private(checker: &CheckerState, symbol: SymbolId) -> Result<bool, Error> {
    let Some(declaration) = checker.symbol(symbol)?.value_declaration() else {
        return Ok(false);
    };
    let Some(name) = checker.node(declaration)?.name() else {
        return Ok(false);
    };
    Ok(checker.node(name)?.kind() == K::PrivateIdentifier)
}

/// `ast.SymbolName`: a private class element's `#name`, else the stored name.
fn hover_symbol_name(checker: &CheckerState, symbol: SymbolId) -> Result<JsString, Error> {
    let read = checker.symbol(symbol)?;
    match read.value_declaration() {
        Some(declaration) => {
            let view = checker.ast(declaration)?;
            let name = tsr_ast::symbol_name(&read, view)?;
            Ok(JsString::from_bytes(name.as_bytes()))
        }
        None => Ok(read.name_to_owned()),
    }
}

/// `getTargetType`: the target of a type reference, else the type itself.
fn target_type(checker: &CheckerState, ty: TypeId) -> Result<TypeId, Error> {
    if checker.types.object_flags(ty)? & crate::object_flags::REFERENCE != 0 {
        return checker.types.target(ty);
    }
    Ok(ty)
}

/// `ast.IsClassLike`, read through the checker.
fn is_class_like_node(checker: &CheckerState, node: NodeId) -> Result<bool, Error> {
    Ok(tsr_ast::utilities::is_class_like(&checker.node(node)?))
}

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/nodebuilder_hover.go:isExpanding
    pub(crate) fn is_expanding(&self) -> bool {
        self.max_expansion_depth != -1
    }

    fn node_list(&mut self, nodes: Vec<NodeId>) -> Result<NodeListId, Error> {
        self.list(nodes)
    }

    fn modifier_list(&mut self, flags: u32) -> Result<NodeListId, Error> {
        let nodes =
            tsr_ast::utilities_middle::create_modifiers_from_modifier_flags(flags, |kind| {
                Some(self.ast.new_modifier(kind))
            })
            .unwrap_or_default();
        self.modifiers_list(nodes.into_iter().flatten().collect())
    }

    fn builder_modifier_flags(&self, node: NodeId) -> Result<u32, Error> {
        let view = self.ast.view();
        Ok(view.node(node)?.modifier_flags(view)?)
    }

    fn replace_builder_modifiers(&mut self, node: NodeId, flags: u32) -> Result<NodeId, Error> {
        let modifiers = self.modifier_list(flags)?;
        Ok(tsr_ast::utilities_class::replace_modifiers(
            &mut self.ast,
            node,
            Some(modifiers),
        ))
    }

    fn clone_source_node(&mut self, node: NodeId) -> Result<NodeId, Error> {
        self.retain_source_node(node)?;
        tsr_ast::deep_clone_node(&mut self.ast, Some(node))
            .ok_or_else(|| tsr_arena::Error::InvalidGraph.into())
    }

    fn identifier(&mut self, text: &[u8]) -> NodeId {
        self.ast.new_identifier(JsString::from_bytes(text))
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.expandSymbolForHover
    pub(crate) fn expand_symbol_for_hover(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Vec<NodeId>, Error> {
        let flags = self.checker.symbol(symbol)?.flags();
        let mut results = Vec::new();
        if flags & sf::ENUM != 0 {
            results.push(self.expand_enum_declaration(symbol)?);
        }
        if flags & sf::CLASS != 0 {
            results.push(self.expand_class_declaration(symbol)?);
        }
        // Module/namespace before interface (matching Strada ordering for merged declarations)
        if flags & (sf::VALUE_MODULE | sf::NAMESPACE_MODULE) != 0 {
            results.push(self.expand_module_declaration(symbol)?);
        }
        if flags & sf::INTERFACE != 0 && flags & sf::CLASS == 0 {
            results.push(self.expand_interface_declaration(symbol)?);
        }
        Ok(results)
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.expandEnumDecl
    fn expand_enum_declaration(&mut self, symbol: SymbolId) -> Result<NodeId, Error> {
        let name = hover_symbol_name(self.checker, symbol)?;
        self.approximate_length += 9 + name.len();
        let ty = self.checker.get_type_of_symbol(symbol)?;
        let mut properties = Vec::new();
        for property in self.checker.get_properties_of_type(ty)? {
            if self.checker.symbol(property)?.flags() & sf::ENUM_MEMBER != 0 {
                properties.push(property);
            }
        }
        let mut members = Vec::new();
        for (index, &property) in properties.iter().enumerate() {
            if self.check_truncation_if_expanding() && index + 3 < properties.len() - 1 {
                self.expansion_truncated = true;
                let text = format!(" ... {} more ... ", properties.len() - index - 1);
                let literal = self
                    .ast
                    .new_string_literal(JsString::from_bytes(text.into_bytes()), 0);
                members.push(self.ast.new_enum_member(Some(literal), None));
                let last = properties[properties.len() - 1];
                let last_name = self.checker.symbol(last)?.name_to_owned();
                let last_name = self.ast.new_identifier(last_name);
                let initializer = self.enum_member_initializer(last)?;
                members.push(self.ast.new_enum_member(Some(last_name), initializer));
                break;
            }
            let declaration = self.enum_member_declaration(property)?;
            let source_initializer = match declaration {
                Some(declaration) => self.checker.node(declaration)?.initializer(),
                None => None,
            };
            let initializer = match source_initializer {
                Some(initializer) => Some(self.clone_source_node(initializer)?),
                None => self.enum_member_initializer(property)?,
            };
            let member_name = self.checker.symbol(property)?.name_to_owned();
            self.approximate_length += 4 + member_name.len();
            if initializer.is_some() {
                self.approximate_length += 5; // " = " + value estimate
            }
            let member_name = self.ast.new_identifier(member_name);
            members.push(self.ast.new_enum_member(Some(member_name), initializer));
        }
        let modifiers = if self.checker.symbol(symbol)?.flags() & sf::CONST_ENUM != 0 {
            Some(self.modifier_list(mf::CONST)?)
        } else {
            None
        };
        let name = self.ast.new_identifier(name);
        let members = self.node_list(members)?;
        Ok(self
            .ast
            .new_enum_declaration(modifiers, Some(name), Some(members)))
    }

    fn enum_member_declaration(&self, symbol: SymbolId) -> Result<Option<NodeId>, Error> {
        for declaration in self.checker.symbol_declarations(symbol)?.iter().flatten() {
            if self.checker.node(declaration)?.kind() == K::EnumMember {
                return Ok(Some(declaration));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.enumMemberInitializer
    fn enum_member_initializer(&mut self, symbol: SymbolId) -> Result<Option<NodeId>, Error> {
        let Some(declaration) = self.enum_member_declaration(symbol)? else {
            return Ok(None);
        };
        Ok(match self.checker.enum_member_constant(declaration)? {
            Some(tsr_printer::emit_resolver::ConstantValue::String(text)) => {
                Some(self.ast.new_string_literal(text, 0))
            }
            Some(tsr_printer::emit_resolver::ConstantValue::Number(number)) => Some(
                self.ast
                    .new_numeric_literal(JsString::from_bytes(number.to_string().into_bytes()), 0),
            ),
            None => None,
        })
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.expandClassDecl
    fn expand_class_declaration(&mut self, symbol: SymbolId) -> Result<NodeId, Error> {
        let name = hover_symbol_name(self.checker, symbol)?;
        self.approximate_length += 9 + name.len();
        let mut class_like_declarations = Vec::new();
        for declaration in self.checker.symbol_declarations(symbol)?.iter().flatten() {
            if is_class_like_node(self.checker, declaration)? {
                class_like_declarations.push(declaration);
            }
        }
        let old_enclosing = self.enclosing;
        if let Some(&original) = class_like_declarations.first() {
            self.enclosing = Some(original);
        }
        let result = self.expand_class_declaration_worker(symbol, name, &class_like_declarations);
        self.enclosing = old_enclosing;
        result
    }

    fn expand_class_declaration_worker(
        &mut self,
        symbol: SymbolId,
        name: JsString,
        class_like_declarations: &[NodeId],
    ) -> Result<NodeId, Error> {
        let local_parameters = self.checker.get_local_type_parameters(symbol)?.to_vec();
        let mut type_parameters = Vec::new();
        for parameter in local_parameters {
            type_parameters.push(self.type_parameter_node(parameter)?);
        }
        let declared = self.checker.declared_interface_type(symbol)?;
        let class_type = self
            .checker
            .get_type_with_optional_this_argument(declared, None, false)?;
        let target = target_type(self.checker, class_type)?;
        let base_types = self.checker.interface_base_types(target)?.to_vec();
        let static_type = self.checker.get_type_of_symbol(symbol)?;
        let is_class = match self.checker.types.get(static_type)?.symbol {
            Some(static_symbol) => match self.checker.symbol(static_symbol)?.value_declaration() {
                Some(declaration) => is_class_like_node(self.checker, declaration)?,
                None => false,
            },
            None => false,
        };
        let static_base_type = if is_class {
            Some(self.checker.class_base_constructor_type(declared)?)
        } else {
            Some(self.checker.builtins.any_type)
        };
        // Heritage clauses
        let heritage_clauses = self.hover_heritage_clauses(class_like_declarations)?;
        // Instance members via addPropertyToElementList (reusing existing serialization),
        // then convert TypeElements to ClassElements and add class-specific modifiers
        let all_properties = self.checker.get_properties_of_type(class_type)?;
        let symbol_properties =
            self.filter_inherited_properties(class_type, &base_types, all_properties)?;
        let mut public_properties = Vec::new();
        let mut private_properties = Vec::new();
        for &property in &symbol_properties {
            if is_hash_private(self.checker, property)? {
                private_properties.push(property);
            } else {
                public_properties.push(property);
            }
        }
        let instance = self.serialize_properties_with_truncation(&public_properties, Vec::new())?;
        let instance = self.type_elements_to_class_elements(instance)?;
        let instance = self.add_class_modifiers(instance, false)?;
        // Static members
        let mut static_properties = Vec::new();
        for property in self.checker.get_properties_of_type(static_type)? {
            let read = self.checker.symbol(property)?;
            if read.flags() & sf::PROTOTYPE == 0
                && read.name_bytes() != b"prototype"
                && !self.is_namespace_member(property)?
            {
                static_properties.push(property);
            }
        }
        let statics = self.serialize_properties_with_truncation(&static_properties, Vec::new())?;
        let statics = self.type_elements_to_class_elements(statics)?;
        let statics = self.add_class_modifiers(statics, true)?;
        // Hash-private members
        let privates = if private_properties.is_empty() {
            Vec::new()
        } else {
            let privates =
                self.serialize_properties_with_truncation(&private_properties, Vec::new())?;
            self.type_elements_to_class_elements(privates)?
        };
        // Constructors
        let constructors =
            self.serialize_constructors(static_type, static_base_type, is_class, symbol)?;
        // Index signatures
        let indexes =
            self.serialize_index_signatures_of_type(class_type, base_types.first().copied())?;
        let mut members = indexes;
        members.extend(statics);
        members.extend(constructors);
        members.extend(instance);
        members.extend(privates);
        let name = self.ast.new_identifier(name);
        let type_parameters = self.node_list(type_parameters)?;
        let heritage_clauses = self.node_list(heritage_clauses)?;
        let members = self.node_list(members)?;
        Ok(self.ast.new_class_declaration(
            None,
            Some(name),
            Some(type_parameters),
            Some(heritage_clauses),
            Some(members),
        ))
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.addClassModifiers
    fn add_class_modifiers(
        &mut self,
        mut members: Vec<NodeId>,
        is_static: bool,
    ) -> Result<Vec<NodeId>, Error> {
        for member in &mut members {
            // Find the symbol for this member by matching the property name
            let name = self.ast.view().node(*member)?.name();
            let Some(member_symbol) =
                name.and_then(|name| self.id_to_symbol.get(&name).copied().flatten())
            else {
                continue;
            };
            let mut flags = self.checker.property_modifiers(member_symbol)? & !mf::ASYNC;
            if is_static {
                flags |= mf::STATIC;
            }
            if flags != 0 && tsr_ast::utilities::can_have_modifiers(&self.ast.view().node(*member)?)
            {
                let existing = self.builder_modifier_flags(*member)?;
                if flags != existing {
                    *member = self.replace_builder_modifiers(*member, flags | existing)?;
                }
            }
        }
        Ok(members)
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:typeElementsToClassElements
    fn type_elements_to_class_elements(
        &mut self,
        mut members: Vec<NodeId>,
    ) -> Result<Vec<NodeId>, Error> {
        for member in &mut members {
            let read = self.ast.view().node(*member)?;
            match read.kind().known() {
                Some(K::PropertySignature) => {
                    let (modifiers, name, question, ty) = (
                        read.modifiers(),
                        read.name(),
                        read.postfix_token(),
                        read.type_node(),
                    );
                    *member = self
                        .ast
                        .new_property_declaration(modifiers, name, question, ty, None);
                }
                Some(K::MethodSignature) => {
                    let (modifiers, name, question, parameters, type_parameters, ty) = (
                        read.modifiers(),
                        read.name(),
                        read.postfix_token(),
                        read.parameter_list(),
                        read.type_parameter_list(),
                        read.type_node(),
                    );
                    *member = self.ast.new_method_declaration(
                        modifiers,
                        None,
                        name,
                        question,
                        type_parameters,
                        parameters,
                        ty,
                        None,
                        None,
                    );
                }
                _ => {}
            }
        }
        Ok(members)
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.expandInterfaceDecl
    fn expand_interface_declaration(&mut self, symbol: SymbolId) -> Result<NodeId, Error> {
        let name = hover_symbol_name(self.checker, symbol)?;
        self.approximate_length += 14 + name.len();
        let interface_type = self.checker.declared_interface_type(symbol)?;
        let mut interface_declarations = Vec::new();
        for declaration in self.checker.symbol_declarations(symbol)?.iter().flatten() {
            if self.checker.node(declaration)?.kind() == K::InterfaceDeclaration {
                interface_declarations.push(declaration);
            }
        }
        let local_parameters = self.checker.get_local_type_parameters(symbol)?.to_vec();
        let mut type_parameters = Vec::new();
        for parameter in local_parameters {
            type_parameters.push(self.type_parameter_node(parameter)?);
        }
        let base_types = self.checker.interface_base_types(interface_type)?.to_vec();
        let base_type = if base_types.is_empty() {
            None
        } else {
            Some(self.checker.get_intersection_type(&base_types)?)
        };
        // Members: reuse existing serialization functions
        self.checker.resolve_type_members(interface_type)?;
        let (constructs, calls, properties) = self
            .checker
            .resolved_signatures_and_properties(interface_type)?;
        let mut members = Vec::new();
        // Index signatures, filtering those identical to base
        members.extend(self.serialize_index_signatures_of_type(interface_type, base_type)?);
        // Construct signatures (skip abstract)
        for signature in constructs {
            if self.checker.signatures.get(signature)?.flags & signature_flags::ABSTRACT != 0 {
                continue;
            }
            members.push(self.signature_node(signature, K::ConstructSignature, None, None)?);
        }
        // Call signatures
        for signature in calls {
            members.push(self.signature_node(signature, K::CallSignature, None, None)?);
        }
        // Properties, filtering inherited
        let filtered = self.filter_inherited_properties(interface_type, &base_types, properties)?;
        let members = self.serialize_properties_with_truncation(&filtered, members)?;
        // Heritage clauses
        let heritage_clauses = self.hover_heritage_clauses(&interface_declarations)?;
        let name = self.ast.new_identifier(name);
        let type_parameters = self.node_list(type_parameters)?;
        let heritage_clauses = self.node_list(heritage_clauses)?;
        let members = self.node_list(members)?;
        Ok(self.ast.new_interface_declaration(
            None,
            Some(name),
            Some(type_parameters),
            Some(heritage_clauses),
            Some(members),
        ))
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.hoverHeritageClauses
    fn hover_heritage_clauses(&mut self, declarations: &[NodeId]) -> Result<Vec<NodeId>, Error> {
        let mut extends = Vec::new();
        let mut implements = Vec::new();
        for &declaration in declarations {
            let view = self.checker.ast(declaration)?;
            let extends_elements =
                tsr_ast::utilities_class::get_extends_heritage_clause_elements(view, declaration)?;
            let implements_elements =
                tsr_ast::utilities_class::get_implements_heritage_clause_elements(
                    view,
                    declaration,
                )?;
            for element in extends_elements {
                extends.push(self.clone_source_node(element)?);
            }
            for element in implements_elements {
                implements.push(self.clone_source_node(element)?);
            }
        }
        let mut clauses = Vec::new();
        if !extends.is_empty() {
            let types = self.node_list(extends)?;
            clauses.push(
                self.ast
                    .new_heritage_clause(K::ExtendsKeyword.into(), Some(types)),
            );
        }
        if !implements.is_empty() {
            let types = self.node_list(implements)?;
            clauses.push(
                self.ast
                    .new_heritage_clause(K::ImplementsKeyword.into(), Some(types)),
            );
        }
        Ok(clauses)
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.serializePropertiesWithTruncation
    fn serialize_properties_with_truncation(
        &mut self,
        properties: &[SymbolId],
        mut elements: Vec<NodeId>,
    ) -> Result<Vec<NodeId>, Error> {
        let mut kept = Vec::with_capacity(properties.len());
        for &property in properties {
            if self.checker.symbol(property)?.flags() & sf::PROTOTYPE == 0 {
                kept.push(property);
            }
        }
        for (index, &property) in kept.iter().enumerate() {
            if self.check_truncation_if_expanding() && index + 3 < kept.len() - 1 {
                self.expansion_truncated = true;
                let text = format!("... {} more ...", kept.len() - index - 1);
                let name = self.identifier(text.as_bytes());
                elements.push(self.ast.new_property_signature_declaration(
                    None,
                    Some(name),
                    None,
                    None,
                    None,
                ));
                elements.extend(self.property_elements(kept[kept.len() - 1])?);
                break;
            }
            elements.extend(self.property_elements(property)?);
        }
        Ok(elements)
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.serializeConstructors
    fn serialize_constructors(
        &mut self,
        static_type: TypeId,
        static_base_type: Option<TypeId>,
        is_class: bool,
        symbol: SymbolId,
    ) -> Result<Vec<NodeId>, Error> {
        let signatures = self.checker.signatures_of_type(static_type, true)?;
        let value_declaration = self.checker.symbol(symbol)?.value_declaration();
        let in_js = match value_declaration {
            Some(declaration) => {
                self.checker.node(declaration)?.flags() & node_flags::JAVA_SCRIPT_FILE != 0
            }
            None => false,
        };
        if !is_class && value_declaration.is_some() && in_js && signatures.is_empty() {
            self.approximate_length += 21;
            let modifiers = self.modifier_list(mf::PRIVATE)?;
            let parameters = self.node_list(Vec::new())?;
            return Ok(vec![self.ast.new_constructor_declaration(
                Some(modifiers),
                None,
                Some(parameters),
                None,
                None,
                None,
            )]);
        }
        let parameterless =
            |checker: &CheckerState, signatures: &[SignatureId]| -> Result<bool, Error> {
                for &signature in signatures {
                    if checker
                        .signatures
                        .get(signature)?
                        .parameters
                        .as_ref()
                        .is_some_and(|parameters| !parameters.is_empty())
                    {
                        return Ok(false);
                    }
                }
                Ok(true)
            };
        if let Some(static_base_type) = static_base_type {
            let base_signatures = self.checker.signatures_of_type(static_base_type, true)?;
            if base_signatures.is_empty() && parameterless(self.checker, &signatures)? {
                return Ok(Vec::new());
            }
            if base_signatures.len() == signatures.len() {
                let mut all_match = true;
                for (&signature, &base) in signatures.iter().zip(&base_signatures) {
                    let identical = self.checker.compare_signatures_identical(
                        signature,
                        base,
                        false,
                        false,
                        true,
                        &mut |state, source, target| {
                            state
                                .is_type_related_to(source, target, crate::RelationKind::Identity)
                                .map(|yes| {
                                    if yes {
                                        crate::ternary::TRUE
                                    } else {
                                        crate::ternary::FALSE
                                    }
                                })
                        },
                    )?;
                    if identical != crate::ternary::TRUE {
                        all_match = false;
                        break;
                    }
                }
                if all_match {
                    return Ok(Vec::new());
                }
            }
            let mut private_protected = 0;
            for &signature in &signatures {
                if let Some(declaration) = self.checker.signatures.get(signature)?.declaration {
                    let view = self.checker.ast(declaration)?;
                    private_protected |= view.node(declaration)?.modifier_flags(view)?
                        & (mf::PRIVATE | mf::PROTECTED);
                }
            }
            if private_protected != 0 {
                let modifiers = self.modifier_list(private_protected)?;
                let parameters = self.node_list(Vec::new())?;
                return Ok(vec![self.ast.new_constructor_declaration(
                    Some(modifiers),
                    None,
                    Some(parameters),
                    None,
                    None,
                    None,
                )]);
            }
        } else if parameterless(self.checker, &signatures)? {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for signature in signatures {
            self.approximate_length += 1;
            result.push(self.signature_node(signature, K::Constructor, None, None)?);
        }
        Ok(result)
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.serializeIndexSignaturesOfType
    fn serialize_index_signatures_of_type(
        &mut self,
        input: TypeId,
        base_type: Option<TypeId>,
    ) -> Result<Vec<NodeId>, Error> {
        let infos: Vec<IndexInfoId> = self.checker.index_infos_of_type(input)?.clone();
        let mut result = Vec::new();
        for info in infos {
            if let Some(base_type) = base_type {
                let (key, value) = {
                    let read = self.checker.signatures.index_info(info)?;
                    (read.key_type, read.value_type)
                };
                if let Some(base_info) = self.checker.index_info_of_type(base_type, key)? {
                    let base_value = self.checker.signatures.index_info(base_info)?.value_type;
                    if self.checker.is_type_related_to(
                        value,
                        base_value,
                        crate::RelationKind::Identity,
                    )? {
                        continue;
                    }
                }
            }
            result.push(self.index_signature_node(info)?);
        }
        Ok(result)
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.serializeNamespaceMember
    fn serialize_namespace_member(
        &mut self,
        resolved: SymbolId,
        name: &JsString,
    ) -> Result<NodeId, Error> {
        let flags = self.checker.symbol(resolved)?.flags();
        if flags & sf::TYPE_ALIAS != 0 {
            return self.serialize_type_alias_for_namespace(resolved, name);
        }
        if flags & sf::ENUM != 0 {
            return self.expand_enum_declaration(resolved);
        }
        if flags & sf::CLASS != 0 {
            return self.expand_class_declaration(resolved);
        }
        if flags & sf::INTERFACE != 0 {
            return self.expand_interface_declaration(resolved);
        }
        if flags & (sf::VALUE_MODULE | sf::NAMESPACE_MODULE) != 0 {
            return self.expand_module_declaration(resolved);
        }
        let ty = self.checker.get_type_of_symbol(resolved)?;
        let ty = self.checker.widened_type(ty)?;
        self.approximate_length += name.len() + 5;
        self.let_statement(name.clone(), ty, resolved)
    }

    /// `let name: T` for a value member of a hover namespace.
    fn let_statement(
        &mut self,
        name: JsString,
        ty: TypeId,
        symbol: SymbolId,
    ) -> Result<NodeId, Error> {
        let type_node = self.serialize_declaration_type(None, Some(ty), Some(symbol), true)?;
        let name = self.ast.new_identifier(name);
        let declaration =
            self.ast
                .new_variable_declaration(Some(name), None, Some(type_node), None);
        let declarations = self.node_list(vec![declaration])?;
        let list = self
            .ast
            .new_variable_declaration_list(Some(declarations), node_flags::LET);
        Ok(self.ast.new_variable_statement(None, Some(list)))
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.expandModuleDecl
    fn expand_module_declaration(&mut self, symbol: SymbolId) -> Result<NodeId, Error> {
        let exports = self.checker.module_exports_of_symbol(symbol)?;
        let mut members = Vec::new();
        if let Some(exports) = exports {
            let entries: Vec<SymbolId> = self.checker.table(exports)?.symbols().flatten().collect();
            for member in entries {
                // Filter to namespace-relevant members
                if !self.is_namespace_member(member)? {
                    continue;
                }
                let name = self.checker.symbol(member)?.name_to_owned();
                if !tsr_scanner::is_identifier_text(
                    name.as_bytes(),
                    tsr_core::LanguageVariant::STANDARD,
                ) {
                    continue;
                }
                members.push(member);
            }
        }
        self.checker.sort_symbols(&mut members)?;
        self.approximate_length += 14;
        // Use the same name as symbol display.
        let old_flags = self.flags;
        self.flags |= tsr_nodebuilder::flags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME
            | tsr_nodebuilder::Flags::from(crate::symbol_format_flags::USE_ONLY_EXTERNAL_ALIASING);
        let local_name = self.symbol_display_node(symbol, sf::ALL, true);
        self.flags = old_flags;
        let local_name = local_name?;
        let result = self.expand_module_body(symbol, local_name, &members);
        self.flags = old_flags;
        result
    }

    fn expand_module_body(
        &mut self,
        symbol: SymbolId,
        local_name: NodeId,
        members: &[SymbolId],
    ) -> Result<NodeId, Error> {
        // (node, isLocal): local declarations (e.g. alias targets) get no export modifier
        let mut body: Vec<(NodeId, bool)> = Vec::new();
        let mut emitted_locals = crate::types::Set::default();
        let mut index = 0;
        while index < members.len() {
            let member = members[index];
            if self.check_truncation_if_expanding() && index + 3 < members.len() - 1 {
                self.expansion_truncated = true;
                let text = format!("... ({} more) ...", members.len() - index - 1);
                let identifier = self.identifier(text.as_bytes());
                body.push((self.ast.new_expression_statement(Some(identifier)), false));
                index = members.len() - 1; // skip to last member
                continue;
            }
            let read = self.checker.symbol(member)?;
            let member_name = read.name_to_owned();
            // Handle alias/re-export symbols
            if read.flags() & sf::ALIAS != 0 {
                let declaration = self.checker.alias_declaration(member)?;
                let target = self.checker.target_of_alias_declaration(declaration)?;
                if let Some(target) = target.map(|target| self.checker.get_merged_symbol(target)) {
                    // If the alias target is a local symbol (not itself an export), emit its declaration first
                    let target_read = self.checker.symbol(target)?;
                    let target_flags = target_read.flags();
                    let target_name = target_read.name_to_owned();
                    if target_flags
                        & (sf::BLOCK_SCOPED_VARIABLE | sf::FUNCTION_SCOPED_VARIABLE | sf::PROPERTY)
                        != 0
                        && emitted_locals.insert(target)
                    {
                        let ty = self.checker.get_type_of_symbol(target)?;
                        let ty = self.checker.widened_type(ty)?;
                        self.approximate_length += target_name.len() + 5;
                        let statement = self.let_statement(target_name.clone(), ty, target)?;
                        body.push((statement, true));
                    }
                    self.approximate_length += 16 + member_name.len();
                    let property_name =
                        (member_name != target_name).then(|| self.ast.new_identifier(target_name));
                    let name = self.ast.new_identifier(member_name);
                    let specifier = self
                        .ast
                        .new_export_specifier(false, property_name, Some(name));
                    let specifiers = self.node_list(vec![specifier])?;
                    let exports = self.ast.new_named_exports(Some(specifiers));
                    body.push((
                        self.ast
                            .new_export_declaration(None, false, Some(exports), None, None),
                        false,
                    ));
                    index += 1;
                    continue;
                }
            }
            // resolveSymbol: a non-local alias resolves to its target.
            let resolved = if tsr_ast::is_non_local_alias(
                Some(&self.checker.symbol(member)?),
                sf::VALUE | sf::TYPE | sf::NAMESPACE,
            ) {
                self.checker.resolve_alias(member)?
            } else {
                member
            };
            // Handle functions as function declarations
            if self.checker.symbol(resolved)?.flags() & (sf::FUNCTION | sf::METHOD) != 0 {
                let ty = self.checker.get_type_of_symbol(resolved)?;
                for signature in self.checker.signatures_of_type(ty, false)? {
                    self.approximate_length += 1;
                    let name = self.ast.new_identifier(member_name.clone());
                    body.push((
                        self.signature_node(signature, K::FunctionDeclaration, Some(name), None)?,
                        false,
                    ));
                }
                // If the function also has namespace characteristics, emit an empty namespace.
                let merged = self.checker.get_merged_symbol(resolved);
                let merged_read = self.checker.symbol(merged)?;
                let has_module_exports =
                    merged_read.flags() & (sf::VALUE_MODULE | sf::NAMESPACE_MODULE) != 0
                        && match merged_read.exports() {
                            Some(exports) => !self.checker.table(exports)?.is_empty(),
                            None => false,
                        };
                if !has_module_exports {
                    let name = self.ast.new_identifier(member_name);
                    let statements = self.node_list(Vec::new())?;
                    let block = self.ast.new_module_block(Some(statements));
                    body.push((
                        self.ast.new_module_declaration(
                            None,
                            K::NamespaceKeyword.into(),
                            Some(name),
                            None,
                            Some(block),
                        ),
                        false,
                    ));
                }
                index += 1;
                continue;
            }
            // Handle remaining member kinds (type alias, enum, class, interface, namespace, variable)
            body.push((
                self.serialize_namespace_member(resolved, &member_name)?,
                false,
            ));
            index += 1;
        }
        // Add export modifier to exported statements (skip local declarations and ExportDeclarations).
        for (node, is_local) in &mut body {
            if *is_local || self.ast.view().node(*node)?.kind() == K::ExportDeclaration {
                continue;
            }
            if tsr_ast::utilities::can_have_modifiers(&self.ast.view().node(*node)?) {
                let flags = self.builder_modifier_flags(*node)? | mf::EXPORT;
                *node = self.replace_builder_modifiers(*node, flags)?;
            }
        }
        // Collect nodes, stripping export if all statements are exported.
        let mut statements: Vec<NodeId> = body.into_iter().map(|(node, _)| node).collect();
        let mut all_exported = !statements.is_empty();
        for &statement in &statements {
            if self.builder_modifier_flags(statement)? & mf::EXPORT == 0 {
                all_exported = false;
                break;
            }
        }
        if all_exported {
            for statement in &mut statements {
                if tsr_ast::utilities::can_have_modifiers(&self.ast.view().node(*statement)?) {
                    let flags = self.builder_modifier_flags(*statement)? & !mf::EXPORT;
                    *statement = self.replace_builder_modifiers(*statement, flags)?;
                }
            }
        }
        let keyword = if self.ast.view().node(local_name)?.kind() == K::Identifier {
            K::NamespaceKeyword
        } else {
            K::ModuleKeyword
        };
        let mut source_attributes = None;
        for declaration in self.checker.symbol_declarations(symbol)?.iter().flatten() {
            let read = self.checker.node(declaration)?;
            if read.kind() == K::ModuleDeclaration && read.attributes().is_some() {
                source_attributes = read.attributes();
                break;
            }
        }
        let attributes = match source_attributes {
            Some(source_attributes) => {
                let cloned = self.clone_source_node(source_attributes)?;
                self.emit
                    .set_emit_flags(cloned, tsr_printer::emit_flags::SINGLE_LINE);
                Some(cloned)
            }
            None => None,
        };
        let statements = self.node_list(statements)?;
        let block = self.ast.new_module_block(Some(statements));
        Ok(self.ast.new_module_declaration(
            None,
            keyword.into(),
            Some(local_name),
            attributes,
            Some(block),
        ))
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.serializeTypeAliasForNamespace
    fn serialize_type_alias_for_namespace(
        &mut self,
        symbol: SymbolId,
        name: &JsString,
    ) -> Result<NodeId, Error> {
        let alias_type = self.checker.get_declared_type_of_type_alias(symbol)?;
        let local_parameters = self.checker.get_local_type_parameters(symbol)?.to_vec();
        let mut type_parameters = Vec::new();
        for parameter in local_parameters {
            type_parameters.push(self.type_parameter_node(parameter)?);
        }
        let saved = self.flags;
        self.flags |= tsr_nodebuilder::flags::IN_TYPE_ALIAS;
        let type_node = self.type_node(alias_type);
        self.flags = saved;
        let type_node = type_node?;
        self.approximate_length += 8 + name.len();
        let name = self.ast.new_identifier(name.clone());
        let type_parameters = self.node_list(type_parameters)?;
        Ok(self.ast.new_type_alias_declaration(
            None,
            Some(name),
            Some(type_parameters),
            Some(type_node),
        ))
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.filterInheritedProperties
    fn filter_inherited_properties(
        &mut self,
        ty: TypeId,
        base_types: &[TypeId],
        properties: Vec<SymbolId>,
    ) -> Result<Vec<SymbolId>, Error> {
        if base_types.is_empty() {
            return Ok(properties);
        }
        // Build a lookup from property name to symbol for parent-identity comparison.
        let mut by_name = crate::types::Map::default();
        for &property in &properties {
            by_name.insert(self.checker.symbol(property)?.name_to_owned(), property);
        }
        // Collect names of properties inherited unchanged from base types.
        let mut inherited = crate::types::Set::default();
        let target = target_type(self.checker, ty)?;
        let this_type = self.checker.types.interface(target)?.this_type;
        for &base in base_types {
            let with_this = self
                .checker
                .get_type_with_optional_this_argument(base, this_type, false)?;
            for property in self.checker.get_properties_of_type(with_this)? {
                let name = self.checker.symbol(property)?.name_to_owned();
                if let Some(&existing) = by_name.get(&name) {
                    if self.checker.symbol(property)?.parent()
                        == self.checker.symbol(existing)?.parent()
                    {
                        inherited.insert(name);
                    }
                }
            }
        }
        if inherited.is_empty() {
            return Ok(properties);
        }
        let mut result = Vec::with_capacity(properties.len());
        for property in properties {
            if !inherited.contains(&self.checker.symbol(property)?.name_to_owned()) {
                result.push(property);
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/nodebuilder_hover.go:NodeBuilderImpl.isNamespaceMember
    fn is_namespace_member(&self, symbol: SymbolId) -> Result<bool, Error> {
        let read = self.checker.symbol(symbol)?;
        let flags = read.flags();
        if flags & (sf::TYPE | sf::NAMESPACE | sf::ALIAS) != 0 {
            return Ok(true);
        }
        if flags & sf::PROTOTYPE != 0 || read.name_bytes() == b"prototype" {
            return Ok(false);
        }
        let Some(value) = read.value_declaration() else {
            return Ok(true);
        };
        let view = self.checker.ast(value)?;
        let is_static = tsr_ast::utilities::has_static_modifier(view, value)?;
        let parent_class = match view.node(value)?.parent() {
            Some(parent) => tsr_ast::utilities::is_class_like(&view.node(parent)?),
            None => false,
        };
        Ok(!(is_static && parent_class))
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.ExpandSymbolForHover
    /// The hover declarations of `symbol`, with their modifiers taken from the
    /// symbol's own declarations.
    pub(crate) fn expand_symbol_for_hover_request(
        &mut self,
        symbol: SymbolId,
        meaning: tsr_ast::SymbolFlags,
    ) -> Result<Option<Vec<NodeId>>, Error> {
        self.prepare_context(
            None,
            tsr_nodebuilder::flags::IGNORE_ERRORS
                | tsr_nodebuilder::flags::MULTILINE_OBJECT_LITERALS
                | tsr_nodebuilder::flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
            tsr_nodebuilder::internal_flags::NONE,
        )?;
        // Push the declared type onto the type stack to prevent re-expansion.
        // We push a nil sentinel after the real type so that isTypeOnStack
        // (which skips the last element) still checks declaredType.
        let declared = self.checker.get_declared_type_of_symbol(symbol)?;
        self.type_stack.push(Some(declared));
        self.type_stack.push(None);
        let nodes = self.expand_symbol_for_hover(symbol);
        self.type_stack.truncate(self.type_stack.len() - 2);
        let nodes = nodes?;
        self.propagate_verbosity_out();
        // Simplify declarations by applying original modifiers
        let mut result = Vec::with_capacity(nodes.len());
        for node in nodes {
            match self.ast.view().node(node)?.kind().known() {
                Some(K::ClassDeclaration) => {
                    result.push(self.simplify_class_declaration(node, symbol)?);
                }
                Some(K::EnumDeclaration) => {
                    result.push(self.simplify_modifiers(node, K::EnumDeclaration, symbol)?);
                }
                Some(K::InterfaceDeclaration) => {
                    if meaning & sf::INTERFACE != 0 {
                        result.push(self.simplify_modifiers(
                            node,
                            K::InterfaceDeclaration,
                            symbol,
                        )?);
                    }
                }
                Some(K::ModuleDeclaration) => {
                    result.push(self.simplify_modifiers(node, K::ModuleDeclaration, symbol)?);
                }
                _ => {}
            }
        }
        Ok(self.exit_context_slice(result))
    }

    // port: tsc/internal/checker/nodebuilder.go:simplifyClassDeclaration
    fn simplify_class_declaration(
        &mut self,
        class: NodeId,
        symbol: SymbolId,
    ) -> Result<NodeId, Error> {
        let mut original = None;
        for declaration in self.checker.symbol_declarations(symbol)?.iter().flatten() {
            if is_class_like_node(self.checker, declaration)? {
                original = Some(declaration);
                break;
            }
        }
        let (flags, is_anonymous) = match original {
            Some(original) => {
                let view = self.checker.ast(original)?;
                let read = view.node(original)?;
                (
                    read.modifier_flags(view)?,
                    read.kind() == K::ClassExpression,
                )
            }
            None => (self.builder_modifier_flags(class)?, false),
        };
        let modifiers = flags & !(mf::EXPORT | mf::AMBIENT);
        let mut class = class;
        if is_anonymous {
            let read = self.ast.view().node(class)?;
            let heritage = read
                .data_source()
                .as_class_declaration()
                .and_then(|data| data.heritage_clauses());
            let (existing, type_parameters, members) = (
                read.modifiers(),
                read.type_parameter_list(),
                read.member_list(),
            );
            class = self.ast.update_class_declaration(
                class,
                existing,
                None,
                type_parameters,
                heritage,
                members,
            );
        }
        self.replace_builder_modifiers(class, modifiers)
    }

    // port: tsc/internal/checker/nodebuilder.go:simplifyModifiers
    fn simplify_modifiers(
        &mut self,
        declaration: NodeId,
        kind: K,
        symbol: SymbolId,
    ) -> Result<NodeId, Error> {
        let mut original = None;
        for candidate in self.checker.symbol_declarations(symbol)?.iter().flatten() {
            if self.checker.node(candidate)?.kind() == kind {
                original = Some(candidate);
                break;
            }
        }
        let flags = match original {
            Some(original) => {
                let view = self.checker.ast(original)?;
                view.node(original)?.modifier_flags(view)?
            }
            None => self.builder_modifier_flags(declaration)?,
        };
        self.replace_builder_modifiers(declaration, flags & !(mf::EXPORT | mf::AMBIENT))
    }
}

/// Construct signatures, call signatures and properties.
type ResolvedParts = (Vec<SignatureId>, Vec<SignatureId>, Vec<SymbolId>);

impl CheckerState {
    /// The construct signatures, call signatures and properties of resolved
    /// structured members (`resolveStructuredTypeMembers`).
    pub(crate) fn resolved_signatures_and_properties(
        &mut self,
        ty: TypeId,
    ) -> Result<ResolvedParts, Error> {
        let calls = self.signatures_of_type(ty, false)?;
        let constructs = self.signatures_of_type(ty, true)?;
        let properties = self.get_properties_of_type(ty)?;
        Ok((constructs, calls, properties))
    }

    // port: tsc/internal/checker/printer.go:Checker.ExpandSymbolForHover
    /// The printed hover declarations of `symbol`, one per line; the
    /// verbosity signals are written back.
    pub(crate) fn expand_symbol_for_hover_text(
        &mut self,
        symbol: SymbolId,
        meaning: tsr_ast::SymbolFlags,
        verbosity: Option<&mut crate::VerbosityContext>,
    ) -> Result<JsString, Error> {
        let source = match self.symbol(symbol)?.value_declaration() {
            Some(declaration) => tsr_ast::utilities::get_source_file_of_node(
                self.ast(declaration)?,
                Some(declaration),
            )?,
            None => None,
        };
        let request = verbosity.as_deref().copied();
        let mut signals = None;
        let text = NodeBuilder::with_cached(self, 0, |builder| {
            builder.verbosity = request;
            let nodes = builder
                .expand_symbol_for_hover_request(symbol, meaning)?
                .unwrap_or_default();
            signals = builder.verbosity.take();
            // The printer reads the source file through the builder's view.
            if let Some(source) = source {
                builder.retain_source_node(source)?;
            }
            let printer = tsr_printer::Printer::new(
                tsr_printer::PrinterOptions {
                    remove_comments: true,
                    ..Default::default()
                },
                &builder.emit,
            );
            let mut text = Vec::new();
            for (index, &node) in nodes.iter().enumerate() {
                if index > 0 {
                    text.push(b'\n');
                }
                let mut writer = tsr_printer::TextWriter::new(b"\n", 0);
                printer.write(builder.ast.view(), node, source, &mut writer, None)?;
                text.extend_from_slice(writer.text());
            }
            Ok(JsString::from_bytes(text))
        })?;
        if let (Some(verbosity), Some(signals)) = (verbosity, signals) {
            verbosity.can_increase_verbosity = signals.can_increase_verbosity;
            verbosity.truncated = signals.truncated;
        }
        Ok(text)
    }
}

impl crate::Operation<'_> {
    /// `Checker.ExpandSymbolForHover`: the expanded hover declarations of a
    /// class, interface, enum or namespace, printed.
    pub fn expand_symbol_for_hover(
        &mut self,
        symbol: crate::handles::SymbolRef,
        meaning: tsr_ast::SymbolFlags,
        verbosity: Option<&mut crate::VerbosityContext>,
    ) -> Result<JsString, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state_mut()
            .expand_symbol_for_hover_text(symbol, meaning, verbosity)
    }
}
