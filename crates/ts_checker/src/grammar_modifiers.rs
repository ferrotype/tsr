//! Modifier grammar in source order. The first native grammar error wins; parse
//! diagnostics suppress this grammar layer without suppressing semantic checks.
use crate::{CheckerState, Error};
use ts_arena::NodeId;
use ts_ast::{modifier_flags as mf, node_flags as nf, SyntaxKind as K};
use ts_diagnostics as d;
use ts_jsstring::JsString;

fn class_like(kind: ts_ast::NodeKind) -> bool {
    matches!(kind.known(), Some(K::ClassDeclaration | K::ClassExpression))
}

impl CheckerState {
    // port: tsc/internal/checker/grammarchecks.go:Checker.grammarErrorOnNode
    pub(crate) fn grammar_error_node(
        &mut self,
        node: NodeId,
        diagnostic: &'static d::Message,
        args: Vec<JsString>,
    ) -> Result<bool, Error> {
        let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
            .ok_or(Error::MissingLink("grammar source"))?;
        if !self
            .ast(source)?
            .source_file(source)?
            .diagnostics()
            .is_empty()
        {
            return Ok(false);
        }
        self.error_at(Some(node), diagnostic, args)?;
        Ok(true)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.grammarErrorOnFirstToken
    pub(crate) fn grammar_error_first_token(
        &mut self,
        node: NodeId,
        diagnostic: &'static d::Message,
        args: Vec<JsString>,
    ) -> Result<bool, Error> {
        let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
            .ok_or(Error::MissingLink("grammar source"))?;
        let view = self.ast(source)?;
        if !view.source_file(source)?.diagnostics().is_empty() {
            return Ok(false);
        }
        let range = ts_scanner::get_range_of_token_at_position(
            view,
            source,
            i64::from(view.node(node)?.pos()),
        )?;
        self.add_diagnostic(ts_ast::Diagnostic::new(
            Some(source),
            range,
            diagnostic,
            args,
        ))?;
        Ok(true)
    }

    fn grammar_is_this_parameter(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() != K::Parameter {
            return Ok(false);
        }
        let Some(name) = read.name() else {
            return Ok(false);
        };
        Ok(self.ast(name)?.node(name)?.kind() == K::Identifier
            && self.ast(name)?.node_text(name)?.as_bytes() == b"this")
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.findFirstIllegalModifier
    fn first_illegal_modifier(
        &self,
        node: NodeId,
        modifiers: &[NodeId],
    ) -> Result<Option<NodeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        let first = modifiers
            .iter()
            .copied()
            .find_map(|id| {
                match self
                    .ast(id)
                    .and_then(|view| view.node(id).map_err(Error::from))
                {
                    Ok(read) if read.kind() == K::Decorator => None,
                    other => Some(other.map(|_| id)),
                }
            })
            .transpose()?;
        let Some(first) = first else {
            return Ok(None);
        };
        match read.kind().known() {
            Some(
                K::GetAccessor
                | K::SetAccessor
                | K::Constructor
                | K::PropertyDeclaration
                | K::PropertySignature
                | K::MethodDeclaration
                | K::MethodSignature
                | K::IndexSignature
                | K::ModuleDeclaration
                | K::ImportDeclaration
                | K::JSImportDeclaration
                | K::ImportEqualsDeclaration
                | K::ExportDeclaration
                | K::ExportAssignment
                | K::FunctionExpression
                | K::ArrowFunction
                | K::Parameter
                | K::TypeParameter
                | K::JSTypeAliasDeclaration,
            ) => Ok(None),
            Some(
                K::ClassStaticBlockDeclaration
                | K::PropertyAssignment
                | K::ShorthandPropertyAssignment
                | K::NamespaceExportDeclaration
                | K::MissingDeclaration,
            ) => Ok(Some(first)),
            _ => {
                let parent = read.parent().ok_or(Error::MissingLink("modifier parent"))?;
                if matches!(
                    self.ast(parent)?.node(parent)?.kind().known(),
                    Some(K::ModuleBlock | K::SourceFile)
                ) {
                    return Ok(None);
                }
                let allowed = match read.kind().known() {
                    Some(K::FunctionDeclaration) => Some(K::AsyncKeyword),
                    Some(K::ClassDeclaration | K::ConstructorType) => Some(K::AbstractKeyword),
                    Some(
                        K::ClassExpression | K::InterfaceDeclaration | K::TypeAliasDeclaration,
                    ) => None,
                    Some(K::VariableStatement) => {
                        let list = read
                            .data_source()
                            .as_variable_statement()
                            .ok_or(Error::MissingLink("variable statement"))?
                            .declaration_list()
                            .ok_or(Error::MissingLink("declaration list"))?;
                        (self.ast(list)?.node(list)?.flags() & nf::USING != 0)
                            .then_some(K::AwaitKeyword)
                    }
                    Some(K::EnumDeclaration) => Some(K::ConstKeyword),
                    _ => {
                        return Err(Error::Unsupported(
                            "findFirstIllegalModifier: source family",
                        ))
                    }
                };
                let first_kind = self.ast(first)?.node(first)?.kind();
                Ok(if allowed.is_some_and(|allowed| first_kind == allowed) {
                    None
                } else {
                    Some(first)
                })
            }
        }
    }

    // port: tsc/internal/ast/utilities.go:NodeCanBeDecorated
    fn grammar_can_decorate(&self, node: NodeId, legacy: bool) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        if legacy
            && read
                .name()
                .map(|name| {
                    self.ast(name)?
                        .node(name)
                        .map(|read| read.kind() == K::PrivateIdentifier)
                        .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or(false)
        {
            return Ok(false);
        }
        let parent = read.parent();
        let Some(parent) = parent else {
            return Ok(read.kind() == K::ClassDeclaration);
        };
        let parent_read = self.ast(parent)?.node(parent)?;
        let class_parent = if legacy {
            parent_read.kind() == K::ClassDeclaration
        } else {
            class_like(parent_read.kind())
        };
        match read.kind().known() {
            Some(K::ClassDeclaration) => Ok(true),
            Some(K::ClassExpression) => Ok(!legacy),
            Some(K::PropertyDeclaration) => Ok(class_parent
                && (legacy
                    || read.modifier_flags(self.ast(node)?)? & (mf::ABSTRACT | mf::AMBIENT) == 0)),
            Some(K::GetAccessor | K::SetAccessor | K::MethodDeclaration) => {
                Ok(read.body().is_some() && class_parent)
            }
            Some(K::Parameter) => {
                if !legacy
                    || parent_read.body().is_none()
                    || !matches!(
                        parent_read.kind().known(),
                        Some(K::Constructor | K::MethodDeclaration | K::SetAccessor)
                    )
                    || self.grammar_is_this_parameter(node)?
                {
                    return Ok(false);
                }
                Ok(parent_read
                    .parent()
                    .map(|grand| {
                        self.ast(grand)?
                            .node(grand)
                            .map(|read| read.kind() == K::ClassDeclaration)
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false))
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarModifiers
    pub(crate) fn check_grammar_modifiers(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let modifiers = self.source_list(node, read.modifiers())?;
        if modifiers.is_empty() {
            return Ok(false);
        }
        let kind = read.kind();
        // port: tsc/internal/ast/utilities.go:CanHaveIllegalDecorators
        if matches!(
            kind.known(),
            Some(
                K::PropertyAssignment
                    | K::ShorthandPropertyAssignment
                    | K::FunctionDeclaration
                    | K::Constructor
                    | K::IndexSignature
                    | K::ClassStaticBlockDeclaration
                    | K::MissingDeclaration
                    | K::VariableStatement
                    | K::InterfaceDeclaration
                    | K::TypeAliasDeclaration
                    | K::EnumDeclaration
                    | K::ModuleDeclaration
                    | K::ImportEqualsDeclaration
                    | K::ImportDeclaration
                    | K::JSImportDeclaration
                    | K::NamespaceExportDeclaration
                    | K::ExportDeclaration
                    | K::ExportAssignment
            )
        ) {
            for &modifier in &modifiers {
                if self.ast(modifier)?.node(modifier)?.kind() == K::Decorator {
                    return self.grammar_error_first_token(
                        modifier,
                        d::Decorators_are_not_valid_here,
                        vec![],
                    );
                }
            }
        }
        if let Some(modifier) = self.first_illegal_modifier(node, &modifiers)? {
            return self.grammar_error_first_token(
                modifier,
                d::Modifiers_cannot_appear_here,
                vec![],
            );
        }
        if self.grammar_is_this_parameter(node)? {
            return self.grammar_error_first_token(
                node,
                d::Neither_decorators_nor_modifiers_may_be_applied_to_this_parameters,
                vec![],
            );
        }
        let parent = read.parent().ok_or(Error::MissingLink("modifier parent"))?;
        let parent_kind = self.ast(parent)?.node(parent)?.kind();
        let parent_flags = self.ast(parent)?.node(parent)?.flags();
        let private_name = ts_ast::utilities::is_private_identifier_class_element_declaration(
            self.ast(node)?,
            node,
        )?;
        let block_scope = if kind == K::VariableStatement {
            let list = read
                .data_source()
                .as_variable_statement()
                .ok_or(Error::MissingLink("variable statement"))?
                .declaration_list()
                .ok_or(Error::MissingLink("declaration list"))?;
            self.ast(list)?.node(list)?.flags() & nf::BLOCK_SCOPED
        } else {
            0
        };
        let legacy = self
            .program()?
            .host
            .options()
            .experimental_decorators
            .is_true();
        let mut flags = 0;
        let (
            mut last_static,
            mut last_declare,
            mut last_async,
            mut last_override,
            mut first_decorator,
        ) = (None, None, None, None, None);
        let mut saw_export_before_decorators = false;
        let mut leading_decorators = false;
        macro_rules! fail { ($at:expr, $message:ident $(, $arg:expr)* $(,)?) => { return self.grammar_error_node($at, d::$message, vec![$(JsString::from_bytes($arg.as_bytes())),*]) }; }
        for modifier in modifiers {
            let modifier_read = self.ast(modifier)?.node(modifier)?;
            let mk = modifier_read.kind();
            let reparsed = modifier_read.flags() & nf::REPARSED != 0;
            let text = mk
                .known()
                .map(ts_scanner::token_to_string)
                .ok_or(Error::MissingLink("modifier kind"))?;
            macro_rules! precede {
                ($bit:expr, $previous:expr) => {
                    if flags & $bit != 0 && !reparsed {
                        fail!(
                            modifier,
                            X_0_modifier_must_precede_1_modifier,
                            text,
                            $previous
                        );
                    }
                };
            }
            if mk == K::Decorator {
                if !self.grammar_can_decorate(node, legacy)? {
                    if kind == K::MethodDeclaration
                        && self
                            .ast(node)?
                            .node(node)?
                            .body()
                            .map(|body| {
                                self.ast(body)?
                                    .node(body)
                                    .map(|read| !ts_ast::node_is_present(Some(&read)))
                                    .map_err(Error::from)
                            })
                            .transpose()?
                            .unwrap_or(true)
                    {
                        return self.grammar_error_first_token(node, d::A_decorator_can_only_decorate_a_method_implementation_not_an_overload, vec![]);
                    }
                    return self.grammar_error_first_token(
                        node,
                        d::Decorators_are_not_valid_here,
                        vec![],
                    );
                }
                if legacy && matches!(kind.known(), Some(K::GetAccessor | K::SetAccessor)) {
                    let symbol = self
                        .get_symbol_of_declaration(node)?
                        .ok_or(Error::MissingLink("decorated accessor symbol"))?;
                    // port: tsc/internal/ast/utilities.go:GetAllAccessorDeclarationsForDeclaration
                    let other_kind = if kind == K::GetAccessor {
                        K::SetAccessor
                    } else {
                        K::GetAccessor
                    };
                    if let Some(other) = self.declaration_of_kind(symbol, other_kind)? {
                        if self.ast(other)?.node(other)?.pos() < self.ast(node)?.node(node)?.pos()
                            && self.has_grammar_decorator(other)?
                        {
                            return self.grammar_error_first_token(node, d::Decorators_cannot_be_applied_to_multiple_get_Slashset_accessors_of_the_same_name, vec![]);
                        }
                    }
                }
                if flags & !(mf::EXPORT_DEFAULT | mf::DECORATOR) != 0 {
                    fail!(modifier, Decorators_are_not_valid_here);
                }
                if leading_decorators && flags & mf::MODIFIER != 0 {
                    let source =
                        ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
                            .ok_or(Error::MissingLink("decorator source"))?;
                    if !self
                        .ast(source)?
                        .source_file(source)?
                        .diagnostics()
                        .is_empty()
                    {
                        return Ok(false);
                    }
                    let mut diagnostic = self.diagnostic_for_node(Some(modifier), d::Decorators_may_not_appear_after_export_or_export_default_if_they_also_appear_before_export, vec![])?;
                    diagnostic.related_information.push(std::sync::Arc::new(
                        self.diagnostic_for_node(
                            first_decorator,
                            d::Decorator_used_before_export_here,
                            vec![],
                        )?,
                    ));
                    self.add_diagnostic(diagnostic)?;
                    return Ok(true);
                }
                flags |= mf::DECORATOR;
                if flags & mf::MODIFIER == 0 {
                    leading_decorators = true;
                } else if flags & mf::EXPORT != 0 {
                    saw_export_before_decorators = true;
                }
                first_decorator.get_or_insert(modifier);
                continue;
            }
            if mk != K::ReadonlyKeyword {
                if matches!(
                    kind.known(),
                    Some(K::PropertySignature | K::MethodSignature)
                ) {
                    fail!(modifier, X_0_modifier_cannot_appear_on_a_type_member, text);
                }
                if kind == K::IndexSignature && (mk != K::StaticKeyword || !class_like(parent_kind))
                {
                    fail!(
                        modifier,
                        X_0_modifier_cannot_appear_on_an_index_signature,
                        text
                    );
                }
            }
            if !matches!(
                mk.known(),
                Some(K::InKeyword | K::OutKeyword | K::ConstKeyword)
            ) && kind == K::TypeParameter
            {
                fail!(
                    modifier,
                    X_0_modifier_cannot_appear_on_a_type_parameter,
                    text
                );
            }
            match mk.known() {
                Some(K::ConstKeyword) => {
                    if !matches!(kind.known(), Some(K::EnumDeclaration | K::TypeParameter)) {
                        fail!(node, A_class_member_cannot_have_the_0_keyword, "const");
                    }
                    if kind == K::TypeParameter
                        && !(ts_ast::utilities::is_function_like_declaration_kind(parent_kind)
                            || class_like(parent_kind)
                            || matches!(
                                parent_kind.known(),
                                Some(
                                    K::FunctionType
                                        | K::ConstructorType
                                        | K::CallSignature
                                        | K::ConstructSignature
                                        | K::MethodSignature
                                )
                            ))
                    {
                        fail!(modifier, X_0_modifier_can_only_appear_on_a_type_parameter_of_a_function_method_or_class, text);
                    }
                }
                Some(K::OverrideKeyword) => {
                    if flags & mf::OVERRIDE != 0 {
                        fail!(modifier, X_0_modifier_already_seen, text);
                    }
                    if flags & mf::AMBIENT != 0 {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_be_used_with_1_modifier,
                            text,
                            "declare"
                        );
                    }
                    precede!(mf::READONLY, "readonly");
                    precede!(mf::ACCESSOR, "accessor");
                    precede!(mf::ASYNC, "async");
                    flags |= mf::OVERRIDE;
                    last_override = Some(modifier);
                }
                Some(K::PublicKeyword | K::ProtectedKeyword | K::PrivateKeyword) => {
                    if flags & mf::ACCESSIBILITY_MODIFIER != 0 {
                        fail!(modifier, Accessibility_modifier_already_seen);
                    }
                    precede!(mf::OVERRIDE, "override");
                    precede!(mf::STATIC, "static");
                    precede!(mf::ACCESSOR, "accessor");
                    precede!(mf::READONLY, "readonly");
                    precede!(mf::ASYNC, "async");
                    if matches!(parent_kind.known(), Some(K::ModuleBlock | K::SourceFile)) {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_appear_on_a_module_or_namespace_element,
                            text
                        );
                    }
                    if flags & mf::ABSTRACT != 0 {
                        if mk == K::PrivateKeyword {
                            fail!(
                                modifier,
                                X_0_modifier_cannot_be_used_with_1_modifier,
                                text,
                                "abstract"
                            );
                        }
                        if !reparsed {
                            fail!(
                                modifier,
                                X_0_modifier_must_precede_1_modifier,
                                text,
                                "abstract"
                            );
                        }
                    } else if private_name {
                        fail!(
                            modifier,
                            An_accessibility_modifier_cannot_be_used_with_a_private_identifier
                        );
                    }
                    flags |= ts_ast::modifier_to_flag(mk);
                }
                Some(K::StaticKeyword) => {
                    if flags & mf::STATIC != 0 {
                        fail!(modifier, X_0_modifier_already_seen, text);
                    }
                    precede!(mf::READONLY, "readonly");
                    precede!(mf::ASYNC, "async");
                    precede!(mf::ACCESSOR, "accessor");
                    if matches!(parent_kind.known(), Some(K::ModuleBlock | K::SourceFile)) {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_appear_on_a_module_or_namespace_element,
                            text
                        );
                    }
                    if kind == K::Parameter {
                        fail!(modifier, X_0_modifier_cannot_appear_on_a_parameter, text);
                    }
                    if flags & mf::ABSTRACT != 0 {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_be_used_with_1_modifier,
                            text,
                            "abstract"
                        );
                    }
                    precede!(mf::OVERRIDE, "override");
                    flags |= mf::STATIC;
                    last_static = Some(modifier);
                }
                Some(K::AccessorKeyword) => {
                    if flags & mf::ACCESSOR != 0 {
                        fail!(modifier, X_0_modifier_already_seen, text);
                    }
                    if flags & mf::READONLY != 0 {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_be_used_with_1_modifier,
                            text,
                            "readonly"
                        );
                    }
                    if flags & mf::AMBIENT != 0 {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_be_used_with_1_modifier,
                            text,
                            "declare"
                        );
                    }
                    if kind != K::PropertyDeclaration {
                        fail!(
                            modifier,
                            X_accessor_modifier_can_only_appear_on_a_property_declaration
                        );
                    }
                    flags |= mf::ACCESSOR;
                }
                Some(K::ReadonlyKeyword) => {
                    if flags & mf::READONLY != 0 {
                        fail!(modifier, X_0_modifier_already_seen, text);
                    }
                    if !matches!(
                        kind.known(),
                        Some(
                            K::PropertyDeclaration
                                | K::PropertySignature
                                | K::IndexSignature
                                | K::Parameter
                        )
                    ) {
                        fail!(modifier, X_readonly_modifier_can_only_appear_on_a_property_declaration_or_index_signature);
                    }
                    if flags & mf::ACCESSOR != 0 {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_be_used_with_1_modifier,
                            text,
                            "accessor"
                        );
                    }
                    flags |= mf::READONLY;
                }
                Some(K::ExportKeyword) => {
                    if self
                        .program()?
                        .host
                        .options()
                        .verbatim_module_syntax
                        .is_true()
                        && self.ast(node)?.node(node)?.flags() & nf::AMBIENT == 0
                        && !matches!(
                            kind.known(),
                            Some(
                                K::TypeAliasDeclaration
                                    | K::InterfaceDeclaration
                                    | K::ModuleDeclaration
                            )
                        )
                        && parent_kind == K::SourceFile
                    {
                        let file = self.ast(parent)?.source_file(parent)?;
                        if self
                            .program()?
                            .host
                            .get_emit_module_format_of_file(file.file_name())?
                            == ts_core::ModuleKind::COMMON_JS
                        {
                            fail!(modifier, A_top_level_export_modifier_cannot_be_used_on_value_declarations_in_a_CommonJS_module_when_verbatimModuleSyntax_is_enabled);
                        }
                    }
                    if flags & mf::EXPORT != 0 {
                        fail!(modifier, X_0_modifier_already_seen, text);
                    }
                    precede!(mf::AMBIENT, "declare");
                    precede!(mf::ABSTRACT, "abstract");
                    precede!(mf::ASYNC, "async");
                    if class_like(parent_kind) && kind != K::JSTypeAliasDeclaration {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_appear_on_class_elements_of_this_kind,
                            text
                        );
                    }
                    if kind == K::Parameter {
                        fail!(modifier, X_0_modifier_cannot_appear_on_a_parameter, text);
                    }
                    if block_scope == nf::USING {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_appear_on_a_using_declaration,
                            text
                        );
                    }
                    if block_scope == nf::AWAIT_USING {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_appear_on_an_await_using_declaration,
                            text
                        );
                    }
                    flags |= mf::EXPORT;
                }
                Some(K::DefaultKeyword) => {
                    let container = if parent_kind == K::SourceFile {
                        parent
                    } else {
                        self.ast(parent)?
                            .node(parent)?
                            .parent()
                            .ok_or(Error::MissingLink("default export container"))?
                    };
                    if self.ast(container)?.node(container)?.kind() == K::ModuleDeclaration
                        && !ts_ast::is_ambient_module(self.ast(container)?, container)?
                    {
                        fail!(
                            modifier,
                            A_default_export_can_only_be_used_in_an_ECMAScript_style_module
                        );
                    }
                    if block_scope == nf::USING {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_appear_on_a_using_declaration,
                            text
                        );
                    }
                    if block_scope == nf::AWAIT_USING {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_appear_on_an_await_using_declaration,
                            text
                        );
                    }
                    if flags & mf::EXPORT == 0 && !reparsed {
                        fail!(
                            modifier,
                            X_0_modifier_must_precede_1_modifier,
                            "export",
                            "default"
                        );
                    }
                    if saw_export_before_decorators {
                        fail!(
                            first_decorator.ok_or(Error::MissingLink("first decorator"))?,
                            Decorators_are_not_valid_here
                        );
                    }
                    flags |= mf::DEFAULT;
                }
                Some(K::DeclareKeyword) => {
                    if flags & mf::AMBIENT != 0 {
                        fail!(modifier, X_0_modifier_already_seen, text);
                    }
                    if flags & mf::ASYNC != 0 {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_be_used_in_an_ambient_context,
                            "async"
                        );
                    }
                    if flags & mf::OVERRIDE != 0 {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_be_used_in_an_ambient_context,
                            "override"
                        );
                    }
                    if class_like(parent_kind) && kind != K::PropertyDeclaration {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_appear_on_class_elements_of_this_kind,
                            text
                        );
                    }
                    if kind == K::Parameter {
                        fail!(modifier, X_0_modifier_cannot_appear_on_a_parameter, text);
                    }
                    if block_scope == nf::USING {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_appear_on_a_using_declaration,
                            text
                        );
                    }
                    if block_scope == nf::AWAIT_USING {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_appear_on_an_await_using_declaration,
                            text
                        );
                    }
                    if parent_flags & nf::AMBIENT != 0 && parent_kind == K::ModuleBlock {
                        fail!(
                            modifier,
                            A_declare_modifier_cannot_be_used_in_an_already_ambient_context
                        );
                    }
                    if private_name {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_be_used_with_a_private_identifier,
                            text
                        );
                    }
                    if flags & mf::ACCESSOR != 0 {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_be_used_with_1_modifier,
                            text,
                            "accessor"
                        );
                    }
                    flags |= mf::AMBIENT;
                    last_declare = Some(modifier);
                }
                Some(K::AbstractKeyword) => {
                    if flags & mf::ABSTRACT != 0 {
                        fail!(modifier, X_0_modifier_already_seen, text);
                    }
                    if !matches!(kind.known(), Some(K::ClassDeclaration | K::ConstructorType)) {
                        if !matches!(
                            kind.known(),
                            Some(
                                K::MethodDeclaration
                                    | K::PropertyDeclaration
                                    | K::GetAccessor
                                    | K::SetAccessor
                            )
                        ) {
                            fail!(modifier, X_abstract_modifier_can_only_appear_on_a_class_method_or_property_declaration);
                        }
                        if parent_kind != K::ClassDeclaration
                            || self
                                .ast(parent)?
                                .node(parent)?
                                .modifier_flags(self.ast(parent)?)?
                                & mf::ABSTRACT
                                == 0
                        {
                            if kind == K::PropertyDeclaration {
                                fail!(
                                    modifier,
                                    Abstract_properties_can_only_appear_within_an_abstract_class
                                );
                            }
                            fail!(
                                modifier,
                                Abstract_methods_can_only_appear_within_an_abstract_class
                            );
                        }
                        if flags & mf::STATIC != 0 {
                            fail!(
                                modifier,
                                X_0_modifier_cannot_be_used_with_1_modifier,
                                "static",
                                "abstract"
                            );
                        }
                        if flags & mf::PRIVATE != 0 {
                            fail!(
                                modifier,
                                X_0_modifier_cannot_be_used_with_1_modifier,
                                "private",
                                "abstract"
                            );
                        }
                        if flags & mf::ASYNC != 0 {
                            if let Some(last) = last_async {
                                fail!(
                                    last,
                                    X_0_modifier_cannot_be_used_with_1_modifier,
                                    "async",
                                    "abstract"
                                );
                            }
                        }
                        precede!(mf::OVERRIDE, "override");
                        precede!(mf::ACCESSOR, "accessor");
                    }
                    if self
                        .ast(node)?
                        .node(node)?
                        .name()
                        .map(|name| {
                            self.ast(name)?
                                .node(name)
                                .map(|read| read.kind() == K::PrivateIdentifier)
                                .map_err(Error::from)
                        })
                        .transpose()?
                        .unwrap_or(false)
                    {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_be_used_with_a_private_identifier,
                            text
                        );
                    }
                    flags |= mf::ABSTRACT;
                }
                Some(K::AsyncKeyword) => {
                    if flags & mf::ASYNC != 0 {
                        fail!(modifier, X_0_modifier_already_seen, text);
                    }
                    if flags & mf::AMBIENT != 0 || parent_flags & nf::AMBIENT != 0 {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_be_used_in_an_ambient_context,
                            text
                        );
                    }
                    if kind == K::Parameter {
                        fail!(modifier, X_0_modifier_cannot_appear_on_a_parameter, text);
                    }
                    if flags & mf::ABSTRACT != 0 {
                        fail!(
                            modifier,
                            X_0_modifier_cannot_be_used_with_1_modifier,
                            text,
                            "abstract"
                        );
                    }
                    flags |= mf::ASYNC;
                    last_async = Some(modifier);
                }
                Some(K::InKeyword | K::OutKeyword) => {
                    let bit = if mk == K::InKeyword { mf::IN } else { mf::OUT };
                    if kind != K::TypeParameter
                        || !(class_like(parent_kind)
                            || matches!(
                                parent_kind.known(),
                                Some(
                                    K::InterfaceDeclaration
                                        | K::TypeAliasDeclaration
                                        | K::JSTypeAliasDeclaration
                                )
                            ))
                    {
                        fail!(modifier, X_0_modifier_can_only_appear_on_a_type_parameter_of_a_class_interface_or_type_alias, text);
                    }
                    if flags & bit != 0 {
                        fail!(modifier, X_0_modifier_already_seen, text);
                    }
                    if bit == mf::IN && flags & mf::OUT != 0 {
                        fail!(modifier, X_0_modifier_must_precede_1_modifier, "in", "out");
                    }
                    flags |= bit;
                }
                _ => {}
            }
        }
        if kind == K::Constructor {
            for (flag, modifier, name) in [
                (mf::STATIC, last_static, "static"),
                (mf::OVERRIDE, last_override, "override"),
                (mf::ASYNC, last_async, "async"),
            ] {
                if flags & flag != 0 {
                    fail!(
                        modifier.ok_or(Error::MissingLink("constructor modifier"))?,
                        X_0_modifier_cannot_appear_on_a_constructor_declaration,
                        name
                    );
                }
            }
            return Ok(false);
        }
        if matches!(
            kind.known(),
            Some(K::ImportDeclaration | K::JSImportDeclaration | K::ImportEqualsDeclaration)
        ) && flags & mf::AMBIENT != 0
        {
            fail!(
                last_declare.ok_or(Error::MissingLink("import declare"))?,
                A_0_modifier_cannot_be_used_with_an_import_declaration,
                "declare"
            );
        }
        if kind == K::Parameter && flags & mf::PARAMETER_PROPERTY_MODIFIER != 0 {
            let read = self.ast(node)?.node(node)?;
            if let Some(name) = read.name() {
                if matches!(
                    self.ast(name)?.node(name)?.kind().known(),
                    Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
                ) {
                    fail!(
                        node,
                        A_parameter_property_may_not_be_declared_using_a_binding_pattern
                    );
                }
            }
            if read
                .data_source()
                .as_parameter_declaration()
                .ok_or(Error::MissingLink("parameter modifiers"))?
                .dot_dot_dot_token()
                .is_some()
            {
                fail!(
                    node,
                    A_parameter_property_cannot_be_declared_using_a_rest_parameter
                );
            }
        }
        // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarAsyncModifier
        if flags & mf::ASYNC != 0
            && !matches!(
                kind.known(),
                Some(
                    K::MethodDeclaration
                        | K::FunctionDeclaration
                        | K::FunctionExpression
                        | K::ArrowFunction
                )
            )
        {
            fail!(
                last_async.ok_or(Error::MissingLink("async modifier"))?,
                X_0_modifier_cannot_be_used_here,
                "async"
            );
        }
        Ok(false)
    }

    fn has_grammar_decorator(&self, node: NodeId) -> Result<bool, Error> {
        for modifier in self.source_list(node, self.ast(node)?.node(node)?.modifiers())? {
            if self.ast(modifier)?.node(modifier)?.kind() == K::Decorator {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
