//! Function/constructor declaration groups, overload agreement and implementation
//! compatibility. Signature comparisons use the production relater.
use crate::{CheckerState, Error, RelationKind, SignatureId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, node_flags as nf, symbol_flags as sf, SyntaxKind as K};
use ts_diagnostics as d;

const DECLARATION_FLAGS: u32 =
    mf::EXPORT | mf::AMBIENT | mf::PRIVATE | mf::PROTECTED | mf::ABSTRACT;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkFunctionOrConstructorSymbol
    pub(crate) fn check_function_or_constructor_symbol(
        &mut self,
        symbol: SymbolId,
    ) -> Result<(), Error> {
        if !self.query.function_symbols_checked.insert(symbol) {
            return Ok(());
        }
        let result = self.check_function_or_constructor_symbol_worker(symbol);
        if result.is_err() {
            self.query.function_symbols_checked.remove(&symbol);
        }
        result
    }

    fn declaration_body_present(&self, node: NodeId) -> Result<bool, Error> {
        self.node(node)?
            .body()
            .map(|body| {
                self.ast(body)?
                    .node(body)
                    .map(|read| ts_ast::node_is_present(Some(&read)))
                    .map_err(Error::from)
            })
            .transpose()
            .map(|value| value.unwrap_or(false))
    }

    // port: tsc/internal/checker/checker.go:Checker.getEffectiveDeclarationFlags
    pub(crate) fn effective_declaration_flags(
        &self,
        node: NodeId,
        mask: u32,
    ) -> Result<u32, Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let mut flags = ts_ast::utilities::get_combined_modifier_flags(view, node)?;
        let parent = read
            .parent()
            .ok_or(Error::MissingLink("overload declaration parent"))?;
        let parent_kind = self.node(parent)?.kind();
        if !matches!(
            parent_kind.known(),
            Some(K::InterfaceDeclaration | K::ClassDeclaration | K::ClassExpression)
        ) && read.flags() & nf::AMBIENT != 0
        {
            let mut container = Some(parent);
            while let Some(current) = container {
                if ts_binder::get_container_flags(self.ast(current)?, current)?.0
                    & ts_binder::ContainerFlags::IS_CONTAINER
                    != 0
                {
                    break;
                }
                container = self.node(current)?.parent();
            }
            if let Some(container) = container {
                let global_augmentation = if parent_kind == K::ModuleBlock {
                    self.node(parent)?
                        .parent()
                        .map(|parent| {
                            self.ast(parent)?
                                .node(parent)
                                .map(|read| ts_ast::utilities::is_global_scope_augmentation(&read))
                                .map_err(Error::from)
                        })
                        .transpose()?
                        .unwrap_or(false)
                } else {
                    false
                };
                if self.node(container)?.flags() & nf::EXPORT_CONTEXT != 0
                    && flags & mf::AMBIENT == 0
                    && !global_augmentation
                {
                    flags |= mf::EXPORT;
                }
            }
            flags |= mf::AMBIENT;
        }
        Ok(flags & mask)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkFunctionOrConstructorSymbolWorker
    fn check_function_or_constructor_symbol_worker(
        &mut self,
        symbol: SymbolId,
    ) -> Result<(), Error> {
        let declarations: Vec<_> = self.symbol_declarations(symbol)?.iter().flatten().collect();
        let constructor = self.symbol(symbol)?.flags() & sf::CONSTRUCTOR != 0;
        let mut some_flags = 0;
        let mut all_flags = DECLARATION_FLAGS;
        let mut some_question = false;
        let mut all_question = true;
        let mut has_overloads = false;
        let (mut body_declaration, mut last_nonambient, mut previous) = (None, None, None);
        let mut duplicate_function = false;
        let mut duplicate_constructor = false;
        let mut nonambient_class = false;
        let mut functions = Vec::new();
        for &node in &declarations {
            let read = self.node(node)?;
            let ambient = read.flags() & nf::AMBIENT != 0;
            let parent = read.parent();
            let ambient_or_interface = ambient
                || parent
                    .map(|parent| {
                        self.ast(parent)?
                            .node(parent)
                            .map(|read| {
                                matches!(
                                    read.kind().known(),
                                    Some(K::InterfaceDeclaration | K::TypeLiteral)
                                )
                            })
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false);
            if ambient_or_interface {
                previous = None;
            }
            if matches!(
                read.kind().known(),
                Some(K::ClassDeclaration | K::ClassExpression)
            ) && !ambient
            {
                nonambient_class = true;
            }
            if matches!(
                read.kind().known(),
                Some(
                    K::FunctionDeclaration
                        | K::MethodDeclaration
                        | K::MethodSignature
                        | K::Constructor
                )
            ) {
                functions.push(node);
                let flags = self.effective_declaration_flags(node, DECLARATION_FLAGS)?;
                some_flags |= flags;
                all_flags &= flags;
                let question = read.question_token(self.ast(node)?)?.is_some();
                some_question |= question;
                all_question &= question;
                let present = self.declaration_body_present(node)?;
                if present && body_declaration.is_some() {
                    if constructor {
                        duplicate_constructor = true;
                    } else {
                        duplicate_function = true;
                    }
                } else if let Some(previous) = previous {
                    let previous_read = self.node(previous)?;
                    if previous_read.parent() == parent
                        && previous_read.end() != read.pos()
                        && previous_read.flags() & nf::REPARSED == 0
                    {
                        self.report_implementation_expected(previous, constructor)?;
                    }
                }
                if present {
                    body_declaration.get_or_insert(node);
                } else {
                    has_overloads = true;
                }
                previous = Some(node);
                if !ambient_or_interface {
                    last_nonambient = Some(node);
                }
            }
        }
        if duplicate_constructor {
            for &declaration in &functions {
                self.error_at(
                    Some(declaration),
                    d::Multiple_constructor_implementations_are_not_allowed,
                    vec![],
                )?;
            }
        }
        if duplicate_function {
            for &declaration in &functions {
                self.error_at(
                    Some(self.node(declaration)?.name().unwrap_or(declaration)),
                    d::Duplicate_function_implementation,
                    vec![],
                )?;
            }
        }
        if nonambient_class
            && !constructor
            && self.symbol(symbol)?.flags() & sf::FUNCTION != 0
            && !declarations.is_empty()
        {
            let mut related = Vec::new();
            for &declaration in &declarations {
                if self.node(declaration)?.kind() == K::ClassDeclaration {
                    related.push(std::sync::Arc::new(self.diagnostic_for_node(
                        Some(declaration),
                        d::Consider_adding_a_declare_modifier_to_this_class,
                        vec![],
                    )?));
                }
            }
            for &declaration in &declarations {
                let read = self.node(declaration)?;
                let diagnostic = match read.kind().known() {
                    Some(K::ClassDeclaration) => {
                        d::Class_declaration_cannot_implement_overload_list_for_0
                    }
                    Some(K::FunctionDeclaration) => {
                        d::Function_with_bodies_can_only_merge_with_classes_that_are_ambient
                    }
                    _ => continue,
                };
                let mut diagnostic = self.diagnostic_for_node(
                    Some(read.name().unwrap_or(declaration)),
                    diagnostic,
                    vec![self.symbol(symbol)?.name_to_owned()],
                )?;
                diagnostic.related_information.clone_from(&related);
                self.add_diagnostic(diagnostic)?;
            }
        }
        if let Some(last) = last_nonambient {
            let read = self.node(last)?;
            if read.body().is_none()
                && read.modifier_flags(self.ast(last)?)? & mf::ABSTRACT == 0
                && read.question_token(self.ast(last)?)?.is_none()
            {
                self.report_implementation_expected(last, constructor)?;
            }
        }
        if has_overloads {
            self.check_overload_flags(&declarations, body_declaration, some_flags, all_flags)?;
            if some_question != all_question {
                let canonical = self.canonical_overload(&declarations, body_declaration)?;
                let optional = self
                    .ast(canonical)?
                    .node(canonical)?
                    .question_token(self.ast(canonical)?)?
                    .is_some();
                for &node in &declarations {
                    let read = self.node(node)?;
                    if read.question_token(self.ast(node)?)?.is_some() != optional {
                        self.error_at(
                            read.name(),
                            d::Overload_signatures_must_all_be_optional_or_required,
                            vec![],
                        )?;
                    }
                }
            }
            if let Some(body) = body_declaration {
                let signatures = self.signatures_of_symbol(Some(symbol))?;
                let implementation = self.signature_from_declaration(body)?;
                for signature in signatures {
                    if !self.implementation_compatible_with_overload(implementation, signature)? {
                        let node = self.signatures.get(signature)?.declaration;
                        let mut diagnostic = self.diagnostic_for_node(node, d::This_overload_signature_is_not_compatible_with_its_implementation_signature, vec![])?;
                        diagnostic.related_information.push(std::sync::Arc::new(
                            self.diagnostic_for_node(
                                Some(body),
                                d::The_implementation_signature_is_declared_here,
                                vec![],
                            )?,
                        ));
                        self.add_diagnostic(diagnostic)?;
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    fn canonical_overload(
        &self,
        overloads: &[NodeId],
        implementation: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        let first = *overloads
            .first()
            .ok_or(Error::MissingLink("canonical overload"))?;
        if let Some(implementation) = implementation {
            if self.node(implementation)?.parent() == self.node(first)?.parent() {
                return Ok(implementation);
            }
        }
        Ok(first)
    }

    fn check_overload_flags(
        &mut self,
        overloads: &[NodeId],
        implementation: Option<NodeId>,
        some: u32,
        all: u32,
    ) -> Result<(), Error> {
        if some == all {
            return Ok(());
        }
        let canonical = self.canonical_overload(overloads, implementation)?;
        let canonical_flags = self.effective_declaration_flags(canonical, DECLARATION_FLAGS)?;
        let mut groups: Vec<(NodeId, Vec<NodeId>)> = Vec::new();
        for &overload in overloads {
            let source =
                ts_ast::utilities::get_source_file_of_node(self.ast(overload)?, Some(overload))?
                    .ok_or(Error::MissingLink("overload source"))?;
            if let Some((_, group)) = groups.iter_mut().find(|(file, _)| *file == source) {
                group.push(overload);
            } else {
                groups.push((source, vec![overload]));
            }
        }
        for (_, group) in groups {
            let canonical = self.canonical_overload(&group, implementation)?;
            let file_flags = self.effective_declaration_flags(canonical, DECLARATION_FLAGS)?;
            for overload in group {
                let flags = self.effective_declaration_flags(overload, DECLARATION_FLAGS)?;
                let deviation = flags ^ canonical_flags;
                let file_deviation = flags ^ file_flags;
                let name = self.node(overload)?.name();
                let (location, diagnostic) = if file_deviation & mf::EXPORT != 0 {
                    (
                        name,
                        d::Overload_signatures_must_all_be_exported_or_non_exported,
                    )
                } else if file_deviation & mf::AMBIENT != 0 {
                    (
                        name,
                        d::Overload_signatures_must_all_be_ambient_or_non_ambient,
                    )
                } else if deviation & (mf::PRIVATE | mf::PROTECTED) != 0 {
                    (
                        Some(name.unwrap_or(overload)),
                        d::Overload_signatures_must_all_be_public_private_or_protected,
                    )
                } else if deviation & mf::ABSTRACT != 0 {
                    (
                        name,
                        d::Overload_signatures_must_all_be_abstract_or_non_abstract,
                    )
                } else {
                    continue;
                };
                self.error_at(location, diagnostic, vec![])?;
            }
        }
        Ok(())
    }

    fn report_implementation_expected(
        &mut self,
        node: NodeId,
        constructor: bool,
    ) -> Result<(), Error> {
        let read = self.node(node)?;
        let name = read.name();
        if let Some(name) = name {
            if !ts_ast::node_is_present(Some(&self.node(name)?)) {
                return Ok(());
            }
        }
        let parent = read
            .parent()
            .ok_or(Error::MissingLink("implementation parent"))?;
        let end = read.end();
        let kind = read.kind();
        let children = self.source_children(parent)?;
        let next = children
            .iter()
            .position(|&child| child == node)
            .and_then(|index| children.get(index + 1))
            .copied();
        if let Some(next) = next {
            let read = self.node(next)?;
            if read.pos() == end && read.kind() == kind {
                let next_name = read.name();
                let location = Some(next_name.unwrap_or(next));
                let same_name = if let (Some(name), Some(next_name)) = (name, next_name) {
                    let a = self.node(name)?;
                    let b = self.node(next_name)?;
                    if a.kind() == K::ComputedPropertyName && b.kind() == K::ComputedPropertyName {
                        let a = self.check_computed_property_name(name)?;
                        let b = self.check_computed_property_name(next_name)?;
                        self.is_type_related_to(a, b, RelationKind::Identity)?
                    } else if a.kind() == K::PrivateIdentifier && b.kind() == K::PrivateIdentifier
                        || ts_ast::utilities::is_property_name_literal(&a)
                            && ts_ast::utilities::is_property_name_literal(&b)
                    {
                        self.node_text(name)?.as_bytes() == self.node_text(next_name)?.as_bytes()
                    } else {
                        false
                    }
                } else {
                    false
                };
                if same_name {
                    if matches!(
                        kind.known(),
                        Some(K::MethodDeclaration | K::MethodSignature)
                    ) {
                        let first_static = self
                            .ast(node)?
                            .node(node)?
                            .modifier_flags(self.ast(node)?)?
                            & mf::STATIC
                            != 0;
                        let next_static = self
                            .ast(next)?
                            .node(next)?
                            .modifier_flags(self.ast(next)?)?
                            & mf::STATIC
                            != 0;
                        if first_static != next_static {
                            self.error_at(
                                location,
                                if first_static {
                                    d::Function_overload_must_be_static
                                } else {
                                    d::Function_overload_must_not_be_static
                                },
                                vec![],
                            )?;
                        }
                    }
                    return Ok(());
                }
                if self.declaration_body_present(next)? {
                    let text = ts_scanner::declaration_name_to_string(self.ast(node)?, name)?;
                    self.error_at(
                        location,
                        d::Function_implementation_name_must_be_0,
                        vec![text],
                    )?;
                    return Ok(());
                }
            }
        }
        let location = Some(name.unwrap_or(node));
        let diagnostic = if constructor {
            d::Constructor_implementation_is_missing
        } else if self
            .ast(node)?
            .node(node)?
            .modifier_flags(self.ast(node)?)?
            & mf::ABSTRACT
            != 0
        {
            d::All_declarations_of_an_abstract_method_must_be_consecutive
        } else {
            d::Function_implementation_is_missing_or_not_immediately_following_the_declaration
        };
        self.error_at(location, diagnostic, vec![])?;
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.isImplementationCompatibleWithOverload
    fn implementation_compatible_with_overload(
        &mut self,
        implementation: SignatureId,
        overload: SignatureId,
    ) -> Result<bool, Error> {
        let source = self.erased_signature(implementation)?;
        let target = self.erased_signature(overload)?;
        let source_return = self.return_type_of_signature(source)?;
        let target_return = self.return_type_of_signature(target)?;
        if target_return == self.builtins.void_type
            || self.is_type_related_to(target_return, source_return, RelationKind::Assignable)?
            || self.is_type_related_to(source_return, target_return, RelationKind::Assignable)?
        {
            return self.signature_is_assignable(source, target, true);
        }
        Ok(false)
    }
}
