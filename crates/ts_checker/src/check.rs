//! Source checking over bound declarations. Each unsupported semantic branch
//! fails explicitly; a failed file check never becomes a successful cache hit.

use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, SyntaxKind as K};
use ts_core::Tristate;
use ts_diagnostics as messages;

#[derive(Clone, Copy)]
pub(crate) enum SourceCheckStatus {
    Checking,
    Complete,
    Failed(Error),
}

fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkSourceFile
    pub(crate) fn check_source_file(&mut self, source: NodeId) -> Result<(), Error> {
        let options = self.program()?.host.options();
        let check_unused = options.no_unused_locals == Tristate::TRUE
            || options.no_unused_parameters == Tristate::TRUE;
        self.check_source_file_ex(source, check_unused)
    }

    /// `checkUnused` is also requested by suggestion collection, which runs the
    /// unused-identifier pass regardless of the compiler options.
    pub(crate) fn check_source_file_ex(
        &mut self,
        source: NodeId,
        check_unused: bool,
    ) -> Result<(), Error> {
        match self.source_checks.get(&source).copied() {
            Some(SourceCheckStatus::Complete) => {}
            Some(SourceCheckStatus::Failed(error)) => return Err(error),
            Some(SourceCheckStatus::Checking) => {
                return Err(Error::Unsupported("recursive checkSourceFile"))
            }
            None => {
                self.source_checks
                    .insert(source, SourceCheckStatus::Checking);
                let result = self.check_source_file_worker(source);
                self.source_checks.insert(
                    source,
                    match result {
                        Ok(()) => SourceCheckStatus::Complete,
                        Err(error) => SourceCheckStatus::Failed(error),
                    },
                );
                result?;
            }
        }
        if check_unused {
            if let Some(&result) = self.query.unused_checks.get(&source) {
                return result;
            }
            // This pass depends on a completed type check, but its own failure
            // must not poison later requests that need only type checking.
            let result = (|| {
                if !self.ast(source)?.source_file(source)?.is_declaration_file {
                    let nodes = self
                        .query
                        .identifier_check_nodes
                        .remove(&source)
                        .unwrap_or_default();
                    self.check_unused_identifiers(nodes)?;
                }
                Ok(())
            })();
            // The queue has been consumed: retrying this phase must return its
            // recorded failure, not succeed on the now-empty queue.
            self.query.unused_checks.insert(source, result);
            result?;
        }
        Ok(())
    }

    fn check_source_file_worker(&mut self, source: NodeId) -> Result<(), Error> {
        let view = self.ast(source)?;
        let file = view.source_file(source)?;
        if !matches!(
            file.script_kind,
            ts_core::ScriptKind::TS
                | ts_core::ScriptKind::JS
                | ts_core::ScriptKind::TSX
                | ts_core::ScriptKind::JSX
        ) {
            return Err(Error::Unsupported("checkSourceFile: non-script input"));
        }
        // A TSX/JSX file can contain only ordinary declarations or imports.
        // Reject unported JSX expressions at their operation, not by file kind.
        self.check_grammar_source(source)?;
        self.query.renamed_binding_elements_in_types.clear();
        let view = self.ast(source)?;
        let statements: Vec<_> = view
            .node_slice(view.node(source)?.statements(view)?)?
            .iter()
            .collect();
        for statement in statements.into_iter().flatten() {
            self.check_source_element(statement)?;
        }
        self.finish_deferred_function_bodies(source)?;
        if ts_ast::utilities::is_external_or_common_js_module(
            &self.ast(source)?.source_file(source)?,
        ) {
            self.check_external_module_exports(source)?;
            self.register_for_unused_identifiers_check(source)?;
        }
        if !self.ast(source)?.source_file(source)?.is_declaration_file {
            self.check_unused_renamed_binding_elements()?;
        }
        self.check_deferred_diagnostics()?;
        self.query.reported_unreachable.clear();
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkSourceElementWorker
    pub(crate) fn check_source_element(&mut self, node: NodeId) -> Result<(), Error> {
        let previous = self.current_node.replace(node);
        let previous_unreachable = self.within_unreachable_code;
        self.instantiation.count = 0;
        let result = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.check_source_element_worker(node)
        });
        self.current_node = previous;
        self.within_unreachable_code = previous_unreachable;
        result
    }

    #[allow(
        clippy::match_same_arms,
        reason = "Keep the pinned upstream per-kind dispatch auditable when individual syntax cases change"
    )]
    fn check_source_element_worker(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_eager_jsdoc(node)?;
        if !self.within_unreachable_code
            && self.program()?.host.options().allow_unreachable_code != Tristate::TRUE
            && self.check_source_element_unreachable(node)?
        {
            self.within_unreachable_code = true;
        }
        let view = self.ast(node)?;
        let read = view.node(node)?;
        match read.kind().known() {
            Some(K::ModuleDeclaration) => self.check_module_declaration(node),
            Some(K::ModuleBlock) => self.check_module_block(node),
            Some(K::ExportAssignment) => self.check_export_assignment(node),
            Some(K::ExportDeclaration) => self.check_export_declaration(node),
            Some(K::ExportSpecifier) => self.check_export_specifier(node),
            Some(K::ImportDeclaration | K::JSImportDeclaration) => {
                self.check_import_declaration(node)
            }
            Some(K::ImportEqualsDeclaration) => self.check_import_equals_declaration(node),
            Some(K::ClassStaticBlockDeclaration) => {
                self.check_grammar_modifiers(node)?;
                for child in self.source_children(node)? {
                    self.check_source_element(child)?;
                }
                Ok(())
            }
            Some(K::ClassDeclaration) => self.check_class_declaration(node),
            Some(K::PropertyDeclaration) => self.check_class_property(node),
            Some(K::Parameter) => self.check_parameter(node),
            Some(K::BindingElement) => self.check_binding_element(node),
            Some(K::GetAccessor | K::SetAccessor) => self.check_class_accessor(node),
            Some(K::MethodDeclaration | K::MethodSignature | K::Constructor) => {
                self.check_function_declaration(node)
            }
            Some(K::FunctionDeclaration) => self.check_function_declaration(node),
            Some(K::Block) => self.check_block_statement(node),
            Some(K::IfStatement) => self.check_if_statement(node),
            Some(K::ReturnStatement) => self.check_return_statement(node),
            Some(K::ThrowStatement) => self.check_throw_statement(node),
            Some(K::DoStatement | K::WhileStatement) => self.check_loop_statement(node),
            Some(K::ForStatement) => self.check_for_statement(node),
            Some(K::ForInStatement) => self.check_for_in_statement(node),
            Some(K::ForOfStatement) => self.check_for_of_statement(node),
            Some(K::BreakStatement | K::ContinueStatement) => self.check_jump_statement(node),
            Some(K::SwitchStatement) => self.check_switch_statement(node),
            Some(K::LabeledStatement) => self.check_labeled_statement(node),
            Some(K::WithStatement) => self.check_with_statement(node),
            Some(K::TryStatement) => self.check_try_statement(node),
            Some(K::CatchClause) => self.check_catch_clause(node),
            Some(K::EnumDeclaration) => self.check_enum_declaration(node),
            Some(K::EnumMember) => self.check_enum_member(node),
            Some(K::TypeQuery) => {
                self.get_type_from_type_node(node)?;
                Ok(())
            }
            Some(K::VariableStatement) => {
                let grammar_failed = self.check_grammar_modifiers(node)?;
                let read = self.ast(node)?.node(node)?;
                let list = required(
                    read.data_source()
                        .as_variable_statement()
                        .ok_or(ts_arena::Error::InvalidGraph)?
                        .declaration_list(),
                    "variable declaration list",
                )?;
                if !grammar_failed && !self.check_grammar_variable_list(list)? {
                    self.check_grammar_block_variable(node, list)?;
                }
                self.check_source_element(list)
            }
            Some(K::VariableDeclarationList) => {
                // port: tsc/internal/checker/checker.go:Checker.checkVariableDeclarationList
                let block_scope =
                    ts_ast::utilities::get_combined_node_flags(view, node)? & nf::BLOCK_SCOPED;
                if (block_scope == nf::USING || block_scope == nf::AWAIT_USING)
                    && self.program()?.host.options().emit_script_target()
                        < ts_core::ScriptTarget::ESNEXT
                {
                    self.check_external_emit_helpers(
                        node,
                        crate::external_emit_helpers::ADD_DISPOSABLE_RESOURCE_AND_DISPOSE_RESOURCES,
                    )?;
                }
                let view = self.ast(node)?;
                let list = required(
                    view.node(node)?
                        .data_source()
                        .as_variable_declaration_list()
                        .ok_or(ts_arena::Error::InvalidGraph)?
                        .declarations(),
                    "variable declarations",
                )?;
                let declarations: Vec<_> =
                    view.node_slice(view.list(list)?.nodes())?.iter().collect();
                for declaration in declarations.into_iter().flatten() {
                    self.check_source_element(declaration)?;
                }
                Ok(())
            }
            Some(K::PropertySignature) => self.check_property_signature(node),
            Some(K::VariableDeclaration) => self.check_variable_declaration(node),
            Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration | K::InterfaceDeclaration) => {
                self.check_type_declaration(node)
            }
            Some(K::TypeLiteral) => {
                self.check_object_type_members(node)?;
                let ty = self.get_type_from_type_node(node)?;
                self.resolve_type_members(ty)?;
                self.check_source_index_constraints(ty, node, false)
            }
            Some(K::ParenthesizedType) => {
                self.check_source_element(required(read.type_node(), "parenthesized type")?)
            }
            Some(K::JSDocOptionalType | K::JSDocVariadicType) => Ok(()),
            Some(K::UnionType | K::IntersectionType) => self.check_union_or_intersection_type(node),
            Some(K::TypeReference | K::ExpressionWithTypeArguments) => {
                self.check_type_reference_node(node)
            }
            Some(
                K::JSDocNonNullableType
                | K::JSDocNullableType
                | K::JSDocAllType
                | K::JSDocTypeLiteral,
            ) => self.check_jsdoc_type(node),
            Some(K::TypeParameter) => self.check_type_parameter(node),
            Some(K::TypePredicate) => self.check_type_predicate(node),
            Some(K::ImportType) => self.check_import_type_node(node),
            Some(K::ConditionalType) => {
                for child in self.source_children(node)? {
                    self.check_source_element(child)?;
                }
                Ok(())
            }
            Some(K::InferType) => self.check_infer_type(node),
            Some(K::MappedType) => self.check_mapped_type(node),
            Some(K::TemplateLiteralType) => self.check_template_type(node),
            Some(K::IndexedAccessType) => self.check_indexed_access_type(node),
            Some(
                K::ArrayType
                | K::TupleType
                | K::OptionalType
                | K::RestType
                | K::NamedTupleMember
                | K::TypeOperator,
            ) => self.check_array_tuple_syntax(node),
            Some(
                K::FunctionType
                | K::ConstructorType
                | K::CallSignature
                | K::ConstructSignature
                | K::IndexSignature,
            ) => self.check_signature_syntax(node),
            Some(
                K::ThisType
                | K::LiteralType
                | K::AnyKeyword
                | K::UnknownKeyword
                | K::StringKeyword
                | K::NumberKeyword
                | K::BigIntKeyword
                | K::BooleanKeyword
                | K::SymbolKeyword
                | K::VoidKeyword
                | K::UndefinedKeyword
                | K::NullKeyword
                | K::NeverKeyword
                | K::ObjectKeyword
                | K::IntrinsicKeyword,
            ) => {
                self.get_type_from_type_node(node)?;
                Ok(())
            }
            Some(K::ExpressionStatement) => {
                let expression = required(read.expression(), "expression statement")?;
                self.check_statement_ambient_context(node)?;
                self.check_expression(expression).map(|_| ())
            }
            Some(K::EmptyStatement) => self.check_statement_ambient_context(node).map(|_| ()),
            Some(K::DebuggerStatement) => self.check_statement_ambient_context(node).map(|_| ()),
            Some(K::MissingDeclaration) => {
                Self::check_missing_declaration(node);
                Ok(())
            }
            // Upstream's switch has no case for these; `export as namespace` is
            // checked through its alias target and a stray `;` class member has
            // nothing to check.
            Some(K::NamespaceExportDeclaration | K::SemicolonClassElement) => Ok(()),
            // Upstream visits modifier keywords through forEachChild and its
            // switch has no case for them.
            _ if ts_ast::is_modifier_kind(read.kind()) && read.kind() != K::Decorator => Ok(()),
            _ => Err(Error::Unsupported(
                "checkSourceElementWorker: statement/type family",
            )),
        }
    }

    /// A missing declaration can carry modifiers but never decorators upstream
    /// recognizes (`CanHaveDecorators`), so `checkDecorators` returns at once.
    // port: tsc/internal/checker/checker.go:Checker.checkMissingDeclaration
    fn check_missing_declaration(_node: NodeId) {}

    // port: tsc/internal/checker/checker.go:Checker.checkUnionOrIntersectionType
    fn check_union_or_intersection_type(&mut self, node: NodeId) -> Result<(), Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let list = required(
            if read.kind() == K::UnionType {
                read.data_source()
                    .as_union_type_node()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .types()
            } else {
                read.data_source()
                    .as_intersection_type_node()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .types()
            },
            "compound types",
        )?;
        let constituents: Vec<_> = view.node_slice(view.list(list)?.nodes())?.iter().collect();
        // Check the source children before construction can reduce or reorder
        // the compound type. A cached type is not a completed declaration check.
        for constituent in constituents {
            self.check_source_element(required(constituent, "compound constituent")?)?;
        }
        self.get_type_from_type_node(node)?;
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypeAliasDeclaration
    // port: tsc/internal/checker/checker.go:Checker.checkInterfaceDeclaration
    fn check_type_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        if !self.check_grammar_modifiers(node)?
            && self.ast(node)?.node(node)?.kind() == K::InterfaceDeclaration
        {
            self.check_interface_heritage_grammar(node)?;
        }
        self.check_type_parameters(node)?;
        let read = self.ast(node)?.node(node)?;
        let interface = read.kind() == K::InterfaceDeclaration;
        let parent = required(read.parent(), "type declaration parent")?;
        let name = required(read.name(), "type declaration name")?;
        if !self.container_allows_block_scoped_variable(parent)? {
            self.grammar_error_node(
                node,
                messages::X_0_declarations_can_only_be_declared_inside_a_block,
                vec![ts_ast::JsString::from_bytes(if interface {
                    b"interface".as_slice()
                } else {
                    b"type".as_slice()
                })],
            )?;
        }
        let text = self.ast(name)?.node_text(name)?.into_js_string();
        if matches!(
            text.as_bytes(),
            b"any"
                | b"unknown"
                | b"never"
                | b"number"
                | b"bigint"
                | b"boolean"
                | b"string"
                | b"symbol"
                | b"void"
                | b"object"
                | b"undefined"
        ) {
            self.error_at(
                Some(name),
                if interface {
                    messages::Interface_name_cannot_be_0
                } else {
                    messages::Type_alias_name_cannot_be_0
                },
                vec![text],
            )?;
        }
        let symbol = required(
            self.get_symbol_of_declaration(node)?,
            "type declaration symbol",
        )?;
        self.check_exports_on_merged_declarations(node)?;
        if interface {
            self.check_interface_inheritance(name, symbol)?;
            self.check_object_duplicate_declarations(node, false)?;
            self.check_interface_heritage(node)?;
            for member in self.source_list(node, self.ast(node)?.node(node)?.member_list())? {
                self.check_source_element(member)?;
            }
            self.check_class_or_interface_duplicate_indexes(node)?;
        } else {
            let annotation = required(
                self.ast(node)?.node(node)?.type_node(),
                "type alias annotation",
            )?;
            if self.ast(annotation)?.node(annotation)?.kind() == K::IntrinsicKeyword {
                // The `intrinsic` keyword is a leaf type node with no child nodes to check.
                return Ok(());
            }
            self.check_source_element(annotation)?;
        }
        self.register_for_unused_identifiers_check(node)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypeLiteral
    fn check_object_type_members(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_object_duplicate_declarations(node, false)?;
        for member in self.source_list(node, self.ast(node)?.node(node)?.member_list())? {
            self.check_source_element(member)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkVariableDeclaration
    fn check_variable_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_grammar_variable(node)?;
        self.check_variable_like(node)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkVariableLikeDeclaration
    pub(crate) fn check_variable_like(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let name = required(read.name(), "variable/property name")?;
        let name_kind = self.ast(name)?.node(name)?.kind();
        let property = read.kind() == K::PropertySignature;
        let binding = matches!(
            name_kind.known(),
            Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
        );
        if !property && name_kind != K::Identifier && !binding
            || property
                && !ts_ast::utilities::is_property_name_literal(&self.ast(name)?.node(name)?)
        {
            return Err(Error::Unsupported(
                "checkVariableLikeDeclaration: binding/computed/private name",
            ));
        }
        if !property && read.question_token(self.ast(node)?)?.is_some() {
            return Err(Error::Unsupported(
                "checkVariableLikeDeclaration: optional declaration",
            ));
        }
        let initializer = read.initializer();
        if property && initializer.is_some() {
            return Err(Error::Unsupported(
                "checkGrammarProperty: signature initializer",
            ));
        }
        if binding {
            self.check_binding_variable(node)
        } else {
            self.check_variable_initializer(node)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkVariableLikeDeclaration
    // Shared semantic tail after declaration-specific grammar and name checks.
    pub(crate) fn check_variable_initializer(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let annotation = read.type_node();
        let initializer = read.initializer();
        if let Some(annotation) = annotation {
            self.check_source_element(annotation)?;
        }
        if let Some(name) = self.ast(node)?.node(node)?.name() {
            if self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName {
                self.check_computed_property_name(name)?;
                if let Some(initializer) = initializer {
                    self.check_expression_cached(initializer)?;
                }
            }
        }
        let symbol = required(
            self.get_symbol_of_declaration(node)?,
            "variable/property symbol",
        )?;
        if self.check_require_alias_declaration(node, symbol)? {
            return Ok(());
        }
        let target = self.get_type_of_symbol(symbol)?;
        let target = self.auto_to_any(target)?;
        if self.symbol(symbol)?.value_declaration() != Some(node) {
            self.check_secondary_variable(node, symbol, target)?;
            if !matches!(
                self.ast(node)?.node(node)?.kind().known(),
                Some(K::PropertyDeclaration | K::PropertySignature)
            ) {
                self.check_exports_on_merged_declarations(node)?;
                if matches!(
                    self.ast(node)?.node(node)?.kind().known(),
                    Some(K::VariableDeclaration | K::BindingElement)
                ) {
                    self.check_var_names_not_shadowed(node)?;
                }
                self.check_collisions_for_declaration_name(node)?;
            }
            return Ok(());
        }
        if let Some(initializer) = initializer {
            let read = self.ast(node)?.node(node)?;
            let for_in = if read.kind() == K::VariableDeclaration {
                let list = required(read.parent(), "variable list")?;
                match self.ast(list)?.node(list)?.parent() {
                    Some(parent) => self.ast(parent)?.node(parent)?.kind() == K::ForInStatement,
                    None => false,
                }
            } else {
                false
            };
            if !for_in {
                let target = if target == self.builtins.auto_type {
                    self.builtins.any_type
                } else if self.query.global_types.get("autoArrayType") == Some(&target) {
                    *self
                        .query
                        .global_types
                        .get("anyArrayType")
                        .ok_or(Error::MissingLink("any array global"))?
                } else {
                    target
                };
                let source = self.check_expression_cached(initializer)?;
                self.check_expression_related_with_elaboration(
                    source,
                    target,
                    crate::RelationKind::Assignable,
                    Some(node),
                    Some(initializer),
                    None,
                )?;
                self.check_using_initializer(node, initializer, source)?;
            }
        }
        self.check_variable_declaration_flags(node, symbol, true)?;
        if !matches!(
            self.ast(node)?.node(node)?.kind().known(),
            Some(K::PropertyDeclaration | K::PropertySignature)
        ) {
            self.check_exports_on_merged_declarations(node)?;
            if matches!(
                self.ast(node)?.node(node)?.kind().known(),
                Some(K::VariableDeclaration | K::BindingElement)
            ) {
                self.check_var_names_not_shadowed(node)?;
            }
            self.check_collisions_for_declaration_name(node)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkAssignmentOperator
    // The previous slice used a separate limited relation. Every caller now
    // enters the production assignability cache; recursive work stays in the
    // relater that owns its assumption stack.
    pub(crate) fn source_type_assignable(
        &mut self,
        source: TypeId,
        target: TypeId,
        _active: &mut Vec<(TypeId, TypeId)>,
    ) -> Result<bool, Error> {
        self.is_type_related_to(source, target, crate::RelationKind::Assignable)
    }

    // port: tsc/internal/checker/relater.go:Checker.typeCouldHaveTopLevelSingletonTypes
    pub(crate) fn type_could_have_top_level_singletons(
        &mut self,
        ty: TypeId,
    ) -> Result<bool, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::BOOLEAN != 0 {
            return Ok(false);
        }
        if flags & tf::UNION_OR_INTERSECTION != 0 {
            for &ty in self.types.compound_types(ty)?.clone().iter() {
                if self.type_could_have_top_level_singletons(ty)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if flags & tf::INSTANTIABLE != 0 {
            if let Some(constraint) = self.constraint_of_type(ty)? {
                if constraint != ty {
                    return self.type_could_have_top_level_singletons(constraint);
                }
            }
        }
        Ok(flags & (tf::UNIT | tf::TEMPLATE_LITERAL | tf::STRING_MAPPING) != 0)
    }

    // port: tsc/internal/checker/relater.go:Checker.checkTypeAssignableTo
    // port: tsc/internal/checker/relater.go:Relater.reportRelationError
    pub(crate) fn check_assignable_at(
        &mut self,
        source: TypeId,
        target: TypeId,
        node: NodeId,
    ) -> Result<(), Error> {
        let (_, diagnostic) = self.check_type_related_ex(
            source,
            target,
            crate::RelationKind::Assignable,
            Some(node),
            None,
        )?;
        if let Some(diagnostic) = diagnostic {
            self.add_diagnostic(diagnostic)?;
        }
        Ok(())
    }
}

impl CheckerState {
    /// Resolved once and cached like upstream's memoized global type resolvers.
    // port: tsc/internal/checker/checker.go:Checker.getGlobalTypeResolver
    fn cached_global_type(&mut self, name: &'static str) -> Result<TypeId, Error> {
        if let Some(ty) = self.query.global_types.get(name) {
            return Ok(*ty);
        }
        let ty = self.get_global_type(name, 0, true)?;
        self.query.global_types.insert(name, ty);
        Ok(ty)
    }

    /// The `using`/`await using` branch of `checkVariableLikeDeclaration`: the
    /// initializer must be disposable, or null or undefined.
    // port: tsc/internal/checker/checker.go:Checker.checkVariableLikeDeclaration
    fn check_using_initializer(
        &mut self,
        node: NodeId,
        initializer: NodeId,
        initializer_type: TypeId,
    ) -> Result<(), Error> {
        let block_scope =
            ts_ast::utilities::get_combined_node_flags(self.ast(node)?, node)? & nf::BLOCK_SCOPED;
        let empty = self.builtins.empty_object_type;
        let (mut parts, message) = if block_scope == nf::AWAIT_USING {
            let async_disposable = self.cached_global_type("AsyncDisposable")?;
            let disposable = self.cached_global_type("Disposable")?;
            if async_disposable == empty || disposable == empty {
                return Ok(());
            }
            (
                vec![async_disposable, disposable],
                messages::The_initializer_of_an_await_using_declaration_must_be_either_an_object_with_a_Symbol_asyncDispose_or_Symbol_dispose_method_or_be_null_or_undefined,
            )
        } else if block_scope == nf::USING {
            let disposable = self.cached_global_type("Disposable")?;
            if disposable == empty {
                return Ok(());
            }
            (
                vec![disposable],
                messages::The_initializer_of_a_using_declaration_must_be_either_an_object_with_a_Symbol_dispose_method_or_be_null_or_undefined,
            )
        } else {
            return Ok(());
        };
        parts.push(self.builtins.null_type);
        parts.push(self.builtins.undefined_type);
        let optional_disposable = self.get_union_type(&parts)?;
        let widened = self.widen_type_for_variable_like(node, Some(initializer_type), false)?;
        let (_, diagnostic) = self.check_type_related_ex(
            widened,
            optional_disposable,
            crate::RelationKind::Assignable,
            Some(initializer),
            Some(message),
        )?;
        if let Some(diagnostic) = diagnostic {
            self.add_diagnostic(diagnostic)?;
        }
        Ok(())
    }
}
