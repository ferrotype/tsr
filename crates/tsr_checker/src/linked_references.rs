//! Alias marking by reference hint (`markLinkedReferences` in
//! `tsc/internal/checker/checker.go`). Checking marks aliases at its own call
//! sites; the unspecified hint is the emit resolver's walk over a whole file
//! (`MarkLinkedReferencesRecursively`), which decides each node's hint itself.

use crate::{type_flags as tf, CheckerState, Error, TypeId};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{modifier_flags as mf, node_flags as nf, symbol_flags as sf, SyntaxKind as K};

fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}

/// `ReferenceHint`. Checking marks identifiers, properties and exports at its
/// own call sites, so only the decorator and unspecified hints reach the
/// dispatcher today; the others keep the pin's full dispatch.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "The pin's hints are kept; checking marks most of them at its own call sites"
)]
pub(crate) enum ReferenceHint {
    Unspecified,
    Identifier,
    Property,
    ExportAssignment,
    Jsx,
    ExportImportEquals,
    ExportSpecifier,
    Decorator,
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.markLinkedReferences
    pub(crate) fn mark_linked_references(
        &mut self,
        location: NodeId,
        hint: ReferenceHint,
        property: Option<SymbolId>,
        parent_type: Option<TypeId>,
    ) -> Result<(), Error> {
        // canCollectSymbolAliasAccessibilityData
        if self
            .program()?
            .host
            .options()
            .verbatim_module_syntax
            .is_true()
        {
            return Ok(());
        }
        let read = self.node(location)?;
        if read.flags() & nf::AMBIENT != 0
            && !matches!(
                read.kind().known(),
                Some(K::PropertySignature | K::PropertyDeclaration)
            )
        {
            // References within types and declaration files are never going to contribute to retaining a JS import,
            // except for properties (which can be decorated).
            return Ok(());
        }
        match hint {
            ReferenceHint::Identifier => self.mark_identifier_alias_referenced(location),
            ReferenceHint::Property => {
                self.mark_property_alias_referenced(location, property, parent_type)
            }
            ReferenceHint::ExportAssignment => {
                self.mark_export_assignment_alias_referenced(location)
            }
            ReferenceHint::Jsx => self.mark_jsx_alias_referenced(location),
            ReferenceHint::ExportImportEquals => self.mark_import_equals_alias_referenced(location),
            ReferenceHint::ExportSpecifier => self.mark_export_specifier_alias_referenced(location),
            ReferenceHint::Decorator => self.mark_decorator_alias_referenced(location),
            ReferenceHint::Unspecified => self.mark_unspecified_references(location),
        }
    }

    /// The `ReferenceHintUnspecified` case of `markLinkedReferences`.
    fn mark_unspecified_references(&mut self, location: NodeId) -> Result<(), Error> {
        let read = self.node(location)?;
        if read.flags() & nf::IN_WITH_STATEMENT != 0 {
            // We cannot answer semantic questions within a with block, do not proceed any further
            return Ok(());
        }
        let kind = read.kind();
        let parent = read.parent();
        let view = self.ast(location)?;
        if parent.is_some()
            && tsr_ast::utilities_targets::is_jsx_tag_name(view, location)?
            && self.is_jsx_intrinsic_tag_name(location)?
        {
            return Ok(()); // builtin JSX tag names aren't real type refs by most metrics, but are expressions, so must be filtered
        }
        if kind == K::Identifier {
            let parent = required(parent, "linked reference parent")?;
            let parent_read = self.node(parent)?;
            // A shorthand property with an object-assignment-initializer (e.g. `{ s = 5 }`) is only valid inside a
            // destructuring assignment target. When it appears in an ordinary object literal expression, the checker
            // checks the initializer and never resolves the property name, so resolving it here would report a spurious
            // "No value exists in scope for the shorthand property" diagnostic. Skip such names to match checking.
            if parent_read.kind() == K::ShorthandPropertyAssignment
                && parent_read.name() == Some(location)
                && parent_read
                    .data_source()
                    .as_shorthand_property_assignment()
                    .is_some_and(|data| data.object_assignment_initializer().is_some())
            {
                let object = required(parent_read.parent(), "shorthand property parent")?;
                if !tsr_ast::is_assignment_target(self.ast(object)?, object)? {
                    return Ok(());
                }
            }
            let [meta_property, decorator, for_node, computed_name, heritage_clause] =
                self.many_reference_ancestors(location)?;
            if meta_property.is_some() {
                return Ok(()); // identifiers in meta properties shouldn't be resolved, but are expressions, so must be filtered
            }
            if let Some(decorator) = decorator {
                // Decorators on nodes that cannot be decorated (e.g. class expressions, static blocks,
                // `this` parameters) are never resolved during normal checking, so resolving them here would
                // report spurious diagnostics. Only bail out for such invalid-position decorators; valid
                // decorator expressions must still be resolved and marked for emit.
                if let Some(decorated) = self.node(decorator)?.parent() {
                    let decorated_parent = self.node(decorated)?.parent();
                    let grandparent = match decorated_parent {
                        Some(node) => self.node(node)?.parent(),
                        None => None,
                    };
                    let legacy = self.legacy_decorators()?;
                    if !tsr_ast::utilities_class::node_can_be_decorated(
                        self.ast(decorated)?,
                        legacy,
                        decorated,
                        decorated_parent,
                        grandparent,
                    )? {
                        return Ok(());
                    }
                }
            }
            // The right-hand side of a 'for-in'/'for-of' statement whose initializer is an empty variable
            // declaration list (a grammar error, e.g. `for (var of X)`) is never checked, because the RHS
            // is only checked while inferring the type of a variable declaration and there is none here.
            // Resolving identifiers in the RHS here would report spurious diagnostics.
            if let Some(for_node) = for_node {
                let data = self.node(for_node)?;
                let statement = data.data_source();
                let statement = statement
                    .as_for_in_or_of_statement()
                    .ok_or(Error::MissingLink("for statement payload"))?;
                let initializer = statement.initializer();
                let expression = statement.expression();
                if let (Some(initializer), Some(expression)) = (initializer, expression) {
                    let list = self.node(initializer)?;
                    if list.kind() == K::VariableDeclarationList
                        && self
                            .source_list(
                                initializer,
                                list.as_variable_declaration_list()
                                    .and_then(|d| d.declarations()),
                            )?
                            .is_empty()
                        && (location == expression
                            || tsr_ast::utilities::is_node_descendant_of(
                                self.ast(location)?,
                                Some(location),
                                Some(expression),
                            )?)
                    {
                        return Ok(());
                    }
                }
            }
            // Computed property names on enum members are a grammar error and are never checked
            // (checkEnumMember only checks the member initializer, not the name), so resolving
            // identifiers in them here would report a spurious "Cannot find name" diagnostic.
            if let Some(computed_name) = computed_name {
                let owner = self.node(computed_name)?.parent();
                if let Some(owner) = owner {
                    if self.node(owner)?.kind() == K::EnumMember {
                        return Ok(());
                    }
                }
                if self.is_invalid_computed_property_name(computed_name)? {
                    return Ok(());
                }
            }
            if let Some(heritage_clause) = heritage_clause {
                let owner = required(
                    self.node(heritage_clause)?.parent(),
                    "heritage clause owner",
                )?;
                // extends heritage clauses on interfaces are not expressions and are unchecked if they are
                if self.node(owner)?.kind() == K::InterfaceDeclaration {
                    return Ok(());
                }
                // On a class, only the first `extends` type is resolved as a value (the base class); any
                // additional `extends` types are grammar errors (e.g. `class C extends A extends B` or
                // `class C extends A, B`) and are never resolved during checking.
                let extends = self
                    .node(heritage_clause)?
                    .data_source()
                    .as_heritage_clause()
                    .is_some_and(|data| data.token() == K::ExtendsKeyword);
                let class_like = matches!(
                    self.node(owner)?.kind().known(),
                    Some(K::ClassDeclaration | K::ClassExpression)
                );
                if class_like && extends {
                    if let Some(first) =
                        tsr_ast::utilities_class::get_class_extends_heritage_element(
                            self.ast(owner)?,
                            owner,
                        )?
                    {
                        if location != first
                            && !tsr_ast::utilities::is_node_descendant_of(
                                self.ast(location)?,
                                Some(location),
                                Some(first),
                            )?
                        {
                            return Ok(());
                        }
                    }
                }
            }
            // Identifiers in expression contexts are emitted, so we need to follow their referenced aliases and mark them as used
            // Some non-expression identifiers are also treated as expression identifiers for this purpose, eg, `a` in `b = {a}` or `q` in `import r = q`
            // This is the exception, rather than the rule - most non-expression identifiers are declaration names.
            let parent_kind = self.node(parent)?.kind();
            if (self.expression_node(location)? || parent_kind == K::ShorthandPropertyAssignment)
                && self.should_mark_identifier_alias_referenced(location)?
            {
                if matches!(
                    parent_kind.known(),
                    Some(K::PropertyAccessExpression | K::QualifiedName)
                ) {
                    let parent_read = self.node(parent)?;
                    let left = if parent_kind == K::PropertyAccessExpression {
                        parent_read.expression()
                    } else {
                        parent_read
                            .data_source()
                            .as_qualified_name()
                            .and_then(|data| data.left())
                    };
                    if left != Some(location) {
                        return Ok(()); // Only mark the LHS (the RHS is a property lookup)
                    }
                }
                return self.mark_identifier_alias_referenced(location);
            }
        }
        if matches!(
            kind.known(),
            Some(K::PropertyAccessExpression | K::QualifiedName)
        ) {
            let mut top = location;
            while matches!(
                self.node(top)?.kind().known(),
                Some(K::PropertyAccessExpression | K::QualifiedName)
            ) {
                if self.is_part_of_type_node(top)? {
                    return Ok(());
                }
                top = required(self.node(top)?.parent(), "property access parent")?;
            }
            return self.mark_property_alias_referenced(location, None, None);
        }
        match kind.known() {
            Some(K::ExportAssignment) => {
                return self.mark_export_assignment_alias_referenced(location)
            }
            Some(K::JsxOpeningElement | K::JsxSelfClosingElement | K::JsxOpeningFragment) => {
                return self.mark_jsx_alias_referenced(location)
            }
            Some(K::ImportEqualsDeclaration) => {
                if is_internal_module_import_equals_declaration(self, location)?
                    || self.check_external_import_or_export(location)?
                {
                    return self.mark_import_equals_alias_referenced(location);
                }
                return Ok(());
            }
            Some(K::ExportSpecifier) => {
                return self.mark_export_specifier_alias_referenced(location)
            }
            _ => {}
        }
        if !self
            .program()?
            .host
            .options()
            .emit_decorator_metadata
            .is_true()
        {
            return Ok(());
        }
        let read = self.node(location)?;
        let view = self.ast(location)?;
        if !crate::decorators::can_have_decorators(read.kind())
            || !tsr_ast::utilities_middle::has_decorators(view, &read)?
            || read.modifiers().is_none()
        {
            return Ok(());
        }
        let parent = read.parent();
        let grandparent = match parent {
            Some(parent) => self.node(parent)?.parent(),
            None => None,
        };
        if !tsr_ast::utilities_class::node_can_be_decorated(
            view,
            self.legacy_decorators()?,
            location,
            parent,
            grandparent,
        )? {
            return Ok(());
        }
        self.mark_decorator_alias_referenced(location)
    }

    /// `ast.FindManyAncestors(location, IsMetaProperty, IsDecorator,
    /// IsForInOrOfStatement, IsComputedPropertyName, IsHeritageClause)`: the
    /// first ancestor, the location included, that each predicate matches; a
    /// node answers at most one predicate, the first that is still open.
    fn many_reference_ancestors(&self, location: NodeId) -> Result<[Option<NodeId>; 5], Error> {
        let mut found: [Option<NodeId>; 5] = [None; 5];
        let mut current = Some(location);
        while let Some(node) = current {
            let read = self.node(node)?;
            let kind = read.kind();
            let matches = [
                kind == K::MetaProperty,
                kind == K::Decorator,
                matches!(kind.known(), Some(K::ForInStatement | K::ForOfStatement)),
                kind == K::ComputedPropertyName,
                kind == K::HeritageClause,
            ];
            if let Some(index) = (0..5).find(|&index| found[index].is_none() && matches[index]) {
                found[index] = Some(node);
                if found.iter().all(Option::is_some) {
                    break;
                }
            }
            current = read.parent();
        }
        Ok(found)
    }

    // port: tsc/internal/checker/checker.go:shouldMarkIdentifierAliasReferenced
    fn should_mark_identifier_alias_referenced(&self, node: NodeId) -> Result<bool, Error> {
        let Some(parent) = self.node(node)?.parent() else {
            return Ok(true);
        };
        let read = self.node(parent)?;
        // A property access expression LHS? checkPropertyAccessExpression will handle that.
        if read.kind() == K::PropertyAccessExpression && read.expression() == Some(node) {
            return Ok(false);
        }
        // Next two check for an identifier inside a type only export.
        if read.kind() == K::ExportSpecifier && read.is_type_only() {
            return Ok(false);
        }
        if let Some(grandparent) = read.parent() {
            if let Some(great) = self.node(grandparent)?.parent() {
                let great = self.node(great)?;
                if great.kind() == K::ExportDeclaration && great.is_type_only() {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.markIdentifierAliasReferenced
    fn mark_identifier_alias_referenced(&mut self, location: NodeId) -> Result<(), Error> {
        if tsr_ast::utilities_positions::is_this_in_type_query(self.ast(location)?, location)? {
            return Ok(());
        }
        let symbol = self.resolved_value_symbol(location)?;
        if symbol != self.builtins.arguments_symbol && symbol != self.builtins.unknown_symbol {
            self.mark_alias_referenced_at(location, symbol)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.markPropertyAliasReferenced
    fn mark_property_alias_referenced(
        &mut self,
        location: NodeId,
        property: Option<SymbolId>,
        parent_type: Option<TypeId>,
    ) -> Result<(), Error> {
        if self.is_part_of_import_equals_module_reference(location)? {
            return Ok(());
        }
        let read = self.node(location)?;
        let (left, right) = if read.kind() == K::PropertyAccessExpression {
            (read.expression(), read.name())
        } else {
            let data = read.data_source();
            let data = data
                .as_qualified_name()
                .ok_or(Error::MissingLink("qualified name payload"))?;
            (data.left(), data.right())
        };
        let left = required(left, "property alias receiver")?;
        let right = required(right, "property alias name")?;
        let left_read = self.node(left)?;
        if left_read.kind() != K::Identifier || self.node_text(left)?.as_bytes() == b"this" {
            return Ok(());
        }
        let parent_symbol = self.resolved_value_symbol(left)?;
        if parent_symbol == self.builtins.unknown_symbol {
            return Ok(());
        }
        // In `Foo.Bar.Baz`, 'Foo' is not referenced if 'Bar' is a const enum or a module containing only const enums.
        // `Foo` is also not referenced in `enum FooCopy { Bar = Foo.Bar }`, because the enum member value gets inlined
        // here even if `Foo` is not a const enum.
        //
        // The exceptions are:
        //   1. if 'isolatedModules' is enabled, because the const enum value will not be inlined, and
        //   2. if 'preserveConstEnums' is enabled and the expression is itself an export, e.g. `export = Foo.Bar.Baz`.
        //
        // The property lookup is deferred as much as possible, in as many situations as possible, to avoid alias marking
        // pulling on types/symbols it doesn't strictly need to.
        let options = self.program()?.host.options();
        let isolated = options.isolated_modules();
        let preserve = options.should_preserve_const_enums();
        if isolated || preserve && self.is_export_or_export_expression(location)? {
            return self.mark_alias_referenced_at(location, parent_symbol);
        }
        // Hereafter, this relies on type checking - but every check prior to this only used symbol information
        let left_type = match parent_type {
            Some(ty) => ty,
            None => self.check_expression_cached(left)?,
        };
        if self.types.flags(left_type)? & tf::ANY != 0
            || left_type == self.builtins.silent_never_type
        {
            return self.mark_alias_referenced_at(location, parent_symbol);
        }
        let mut property = property;
        if property.is_none() && parent_type.is_none() {
            // A private name's property is never a const enum, so its lookup cannot change the decision.
            if self.node(right)?.kind() != K::PrivateIdentifier {
                let assignment = self.assignment_target_kind(location)?;
                let apparent =
                    if !matches!(assignment, crate::flow_assignments::AssignmentKind::None)
                        || self.access_is_call_target(location)?
                    {
                        let widened = self.widened_type(left_type)?;
                        self.apparent_type(widened)?
                    } else {
                        self.apparent_type(left_type)?
                    };
                let name = self.node_text(right)?.into_js_string();
                property = self.constituent_property(apparent, name.as_bytes(), false)?;
            }
        }
        let skip = match property {
            Some(property) => {
                let flags = self.symbol(property)?.flags();
                let enum_member_initializer = flags & sf::ENUM_MEMBER != 0
                    && match self.node(location)?.parent() {
                        Some(parent) => self.node(parent)?.kind() == K::EnumMember,
                        None => false,
                    };
                is_const_enum_or_const_enum_only_module(flags) || enum_member_initializer
            }
            None => false,
        };
        if !skip {
            self.mark_alias_referenced_at(location, parent_symbol)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:isPartOfImportEqualsModuleReference
    fn is_part_of_import_equals_module_reference(&self, location: NodeId) -> Result<bool, Error> {
        let mut import_equals = None;
        let mut current = Some(location);
        while let Some(node) = current {
            let read = self.node(node)?;
            if read.kind() == K::ImportEqualsDeclaration {
                import_equals = Some(node);
                break;
            }
            current = read.parent();
        }
        let Some(import_equals) = import_equals else {
            return Ok(false);
        };
        let reference = self
            .node(import_equals)?
            .data_source()
            .as_import_equals_declaration()
            .and_then(|data| data.module_reference());
        let mut current = Some(location);
        while let Some(node) = current {
            if node == import_equals {
                break;
            }
            if Some(node) == reference {
                return Ok(true);
            }
            current = self.node(node)?.parent();
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:isExportOrExportExpression
    pub(crate) fn is_export_or_export_expression(&self, location: NodeId) -> Result<bool, Error> {
        let mut current = Some(location);
        while let Some(node) = current {
            let read = self.node(node)?;
            if let Some(parent) = read.parent() {
                let parent_read = self.node(parent)?;
                if tsr_ast::utilities_modules::is_any_export_assignment(&parent_read) {
                    if parent_read.expression() == Some(node)
                        && tsr_ast::is_entity_name_expression(self.ast(node)?, node)?
                    {
                        return Ok(true);
                    }
                } else if parent_read.kind() == K::ExportSpecifier {
                    let data = parent_read.data_source();
                    let data = data.as_export_specifier();
                    if data.is_some_and(|data| {
                        data.name() == Some(node) || data.property_name() == Some(node)
                    }) {
                        return Ok(true);
                    }
                }
            }
            current = read.parent();
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.markExportAssignmentAliasReferenced
    fn mark_export_assignment_alias_referenced(&mut self, location: NodeId) -> Result<(), Error> {
        let Some(id) = self.node(location)?.expression() else {
            return Ok(());
        };
        if self.node(id)?.kind() == K::Identifier {
            let symbol = self.resolve_entity_name_at(id, sf::ALL, true, true, Some(location))?;
            if let Some(symbol) = symbol {
                let symbol = self.get_export_symbol_of_value_symbol_if_exported(symbol)?;
                self.mark_alias_referenced_at(id, symbol)?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.markImportEqualsAliasReferenced
    fn mark_import_equals_alias_referenced(&mut self, location: NodeId) -> Result<(), Error> {
        let read = self.node(location)?;
        if read.modifier_flags(self.ast(location)?)? & mf::EXPORT != 0 {
            self.mark_module_export_referenced(location)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.markExportSpecifierAliasReferenced
    fn mark_export_specifier_alias_referenced(&mut self, location: NodeId) -> Result<(), Error> {
        let read = self.node(location)?;
        let named = required(read.parent(), "export specifier list")?;
        let declaration = required(self.node(named)?.parent(), "export specifier declaration")?;
        let declaration_read = self.node(declaration)?;
        if declaration_read.module_specifier().is_some()
            || read.is_type_only()
            || declaration_read.is_type_only()
        {
            return Ok(());
        }
        let exported = required(read.property_name_or_name(), "exported name")?;
        if self.node(exported)?.kind() == K::StringLiteral {
            return Ok(()); // Skip for invalid syntax like this: export { "x" }
        }
        let text = self.node_text(exported)?.into_js_string();
        let symbol = self.resolve_name_ex(
            Some(exported),
            text.as_bytes(),
            sf::VALUE | sf::TYPE | sf::NAMESPACE | sf::ALIAS,
            None,
            true,
            false,
        )?;
        if let Some(symbol) = symbol {
            let global = symbol == self.builtins.undefined_symbol
                || symbol == self.builtins.global_this_symbol
                || match self.symbol_declarations(symbol)?.first().flatten() {
                    Some(declaration) => {
                        let view = self.ast(declaration)?;
                        match tsr_ast::get_declaration_container(view, declaration)? {
                            Some(container) => {
                                tsr_ast::utilities_middle::is_global_source_file(view, container)?
                            }
                            None => false,
                        }
                    }
                    None => false,
                };
            if global {
                // Do nothing, non-local symbol
                return Ok(());
            }
        }
        let target = match symbol {
            Some(symbol) if self.symbol(symbol)?.flags() & sf::ALIAS != 0 => {
                Some(self.resolve_alias(symbol)?)
            }
            other => other,
        };
        let value = match target {
            Some(target) => self.module_symbol_flags(target, false, false)? & sf::VALUE != 0,
            None => true,
        };
        if value {
            self.mark_module_export_referenced(location)?; // marks export as used
            self.mark_identifier_alias_referenced(exported)?; // marks target of export as used
        }
        Ok(())
    }
}

// port: tsc/internal/checker/checker.go:isInternalModuleImportEqualsDeclaration
fn is_internal_module_import_equals_declaration(
    state: &CheckerState,
    node: NodeId,
) -> Result<bool, Error> {
    let read = state.node(node)?;
    if read.kind() != K::ImportEqualsDeclaration {
        return Ok(false);
    }
    let reference = read
        .data_source()
        .as_import_equals_declaration()
        .and_then(|data| data.module_reference());
    Ok(match reference {
        Some(reference) => state.node(reference)?.kind() != K::ExternalModuleReference,
        None => true,
    })
}

// port: tsc/internal/checker/emitresolver.go:isConstEnumOrConstEnumOnlyModule
// port: tsc/internal/checker/utilities.go:isConstEnumSymbol
pub(crate) fn is_const_enum_or_const_enum_only_module(flags: tsr_ast::SymbolFlags) -> bool {
    flags & sf::CONST_ENUM != 0 || flags & sf::CONST_ENUM_ONLY_MODULE != 0
}
