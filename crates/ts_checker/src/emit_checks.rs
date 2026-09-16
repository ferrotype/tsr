//! Checking-time emit dependencies: requested helpers and deferred generated-name
//! collisions. Flags belong to the checker and never mutate published syntax.

use crate::{external_emit_helpers as eh, node_check_flags as nc, CheckerState, Error, LinkStore};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, symbol_flags as sf, JsString, SyntaxKind as K};
use ts_core::{ModuleKind, ScriptTarget};
use ts_diagnostics as d;

#[derive(Default)]
pub(crate) struct EmitCheckState {
    pub(crate) node_flags: LinkStore<NodeId, u32>,
    pub(crate) requested_helpers: LinkStore<NodeId, u32>,
    pub(crate) helpers_module: LinkStore<NodeId, Option<SymbolId>>,
    pub(crate) computed_names: LinkStore<NodeId, Option<JsString>>,
}

impl CheckerState {
    pub(crate) fn private_elements_need_transform(&self) -> Result<bool, Error> {
        let options = self.program()?.host.options();
        // The pin has ES2022 for private names and ESNext for decorators.
        Ok(options.emit_script_target() < ScriptTarget::ESNEXT
            || !options.use_define_for_class_fields())
    }

    // port: tsc/internal/ast/utilities.go:GetEnclosingBlockScopeContainer
    // port: tsc/internal/ast/utilities.go:IsBlockScope
    pub(crate) fn enclosing_emit_block_scope(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let mut current = self.ast(node)?.node(node)?.parent();
        while let Some(node) = current {
            let read = self.ast(node)?.node(node)?;
            let parent = read.parent();
            let scoped = match read.kind().known() {
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
                ) => true,
                Some(K::Block) => match parent {
                    Some(parent) => {
                        let read = self.ast(parent)?.node(parent)?;
                        !ts_ast::utilities::is_function_like(Some(&read))
                            && read.kind() != K::ClassStaticBlockDeclaration
                    }
                    None => true,
                },
                _ => false,
            };
            if scoped {
                return Ok(Some(node));
            }
            current = parent;
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.setNodeLinksForPrivateIdentifierScope
    pub(crate) fn set_node_links_for_private_identifier_scope(
        &mut self,
        node: NodeId,
    ) -> Result<(), Error> {
        let Some(name) = self.ast(node)?.node(node)?.name() else {
            return Ok(());
        };
        if self.ast(name)?.node(name)?.kind() != K::PrivateIdentifier
            || !self.private_elements_need_transform()?
        {
            return Ok(());
        }
        let mut scope = self.enclosing_emit_block_scope(node)?;
        while let Some(node) = scope {
            *self.emit_checks.node_flags.get_or_default(node) |=
                nc::CONTAINS_CLASS_WITH_PRIVATE_IDENTIFIERS;
            scope = self.enclosing_emit_block_scope(node)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkSuperExpression
    pub(crate) fn mark_super_static_initializer_scopes(
        &mut self,
        node: NodeId,
    ) -> Result<(), Error> {
        let parent = self
            .ast(node)?
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("super expression parent"))?;
        let mut scope = self.enclosing_emit_block_scope(parent)?;
        while let Some(node) = scope {
            if self.ast(node)?.node(node)?.kind() != K::SourceFile
                || ts_ast::utilities::is_external_or_common_js_module(
                    &self.ast(node)?.source_file(node)?,
                )
            {
                *self.emit_checks.node_flags.get_or_default(node) |=
                    nc::CONTAINS_SUPER_PROPERTY_IN_STATIC_INITIALIZER;
            }
            scope = self.enclosing_emit_block_scope(node)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkExternalEmitHelpers
    pub(crate) fn check_external_emit_helpers(
        &mut self,
        location: NodeId,
        helpers: u32,
    ) -> Result<(), Error> {
        let options = self.program()?.host.options();
        if !options.import_helpers.is_true()
            || self.ast(location)?.node(location)?.flags() & nf::AMBIENT != 0
        {
            return Ok(());
        }
        let (source, _) = self.module_source(location)?;
        let read = self.ast(source)?.source_file(source)?;
        let module_kind = options.emit_module_kind();
        let effective = read.external_module_indicator.is_some()
            || (module_kind == ModuleKind::COMMON_JS
                || (ModuleKind::NODE16..=ModuleKind::NODE_NEXT).contains(&module_kind))
                && read.common_js_module_indicator().is_some();
        if !effective {
            return Ok(());
        }
        let module = if let Some(&module) = self.emit_checks.helpers_module.try_get(source) {
            module
        } else {
            let module = self.resolve_external_helpers_module(source, location)?;
            *self.emit_checks.helpers_module.get_or_default(source) = module;
            module
        };
        let Some(module) = module else {
            return Ok(());
        };
        let requested = *self.emit_checks.requested_helpers.get_or_default(source);
        let unchecked = helpers & !requested;
        let mut helper = eh::FIRST_EMIT_HELPER;
        while helper <= eh::LAST_EMIT_HELPER {
            if unchecked & helper != 0 {
                for &name in helper_names(
                    helper,
                    self.program()?
                        .host
                        .options()
                        .experimental_decorators
                        .is_true(),
                )? {
                    let exports = self.module_exports(module)?;
                    let symbol = self.table(exports)?.get(name).flatten();
                    let symbol = if let Some(symbol) = symbol {
                        let symbol = self.get_merged_symbol(symbol);
                        if self.module_symbol_flags(symbol, false, false)? & sf::VALUE != 0 {
                            Some(if self.symbol(symbol)?.flags() & sf::ALIAS != 0 {
                                self.resolve_alias(symbol)?
                            } else {
                                symbol
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let Some(symbol) = symbol {
                        let arity = if helper == eh::CLASS_PRIVATE_FIELD_GET {
                            Some(3)
                        } else if helper == eh::CLASS_PRIVATE_FIELD_SET {
                            Some(4)
                        } else {
                            None
                        };
                        if let Some(arity) = arity {
                            let mut compatible = false;
                            for signature in self.signatures_of_symbol(Some(symbol))? {
                                if self.parameter_count(signature)? > arity {
                                    compatible = true;
                                    break;
                                }
                            }
                            if !compatible {
                                self.error_at(Some(location), d::This_syntax_requires_an_imported_helper_named_1_with_2_parameters_which_is_not_compatible_with_the_one_in_0_Consider_upgrading_your_version_of_0,
                                    vec![JsString::from_bytes(b"tslib".as_slice()), JsString::from_bytes(name), JsString::from_bytes((arity + 1).to_string().into_bytes())])?;
                            }
                        }
                    } else {
                        self.error_at(Some(location), d::This_syntax_requires_an_imported_helper_named_1_which_does_not_exist_in_0_Consider_upgrading_your_version_of_0,
                            vec![JsString::from_bytes(b"tslib".as_slice()), JsString::from_bytes(name)])?;
                    }
                }
            }
            helper <<= 1;
        }
        *self.emit_checks.requested_helpers.get_or_default(source) |= helpers;
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.needCollisionCheckForIdentifier
    fn need_emit_collision_check(
        &self,
        node: NodeId,
        identifier: NodeId,
        name: &[u8],
    ) -> Result<bool, Error> {
        if self.ast(identifier)?.node(identifier)?.kind() != K::Identifier
            || self.ast(identifier)?.node_text(identifier)?.as_bytes() != name
        {
            return Ok(false);
        }
        let read = self.ast(node)?.node(node)?;
        if matches!(
            read.kind().known(),
            Some(
                K::PropertyDeclaration
                    | K::PropertySignature
                    | K::MethodDeclaration
                    | K::MethodSignature
                    | K::GetAccessor
                    | K::SetAccessor
                    | K::PropertyAssignment
            )
        ) || read.flags() & nf::AMBIENT != 0
        {
            return Ok(false);
        }
        if matches!(
            read.kind().known(),
            Some(K::ImportClause | K::ImportEqualsDeclaration | K::ImportSpecifier)
        ) && self.local_type_only_alias_declaration(node)?.is_some()
        {
            return Ok(false);
        }
        let root = ts_ast::utilities::get_root_declaration(self.ast(node)?, node)?;
        if self.ast(root)?.node(root)?.kind() == K::Parameter {
            let parent = self
                .ast(root)?
                .node(root)?
                .parent()
                .ok_or(Error::MissingLink("parameter parent"))?;
            let body = self.ast(parent)?.node(parent)?.body();
            let read = body
                .map(|node| self.ast(node)?.node(node).map_err(Error::from))
                .transpose()?;
            if ts_ast::node_is_missing(read.as_ref()) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkCollisionsForDeclarationName
    pub(crate) fn check_collisions_for_declaration_name(
        &mut self,
        node: NodeId,
    ) -> Result<(), Error> {
        let Some(name) = self.ast(node)?.node(node)?.name() else {
            return Ok(());
        };
        if self.ast(name)?.node(name)?.kind() != K::Identifier {
            return Ok(());
        }
        let kind = self.ast(node)?.node(node)?.kind();
        let class = matches!(kind.known(), Some(K::ClassDeclaration | K::ClassExpression));
        let module = kind == K::ModuleDeclaration;
        let instantiated = !module
            || ts_ast::get_module_instance_state(self.ast(node)?, node)?
                == ts_ast::ModuleInstanceState::Instantiated;
        let format = self.module_emit_format(node)?;
        let container = self.emit_declaration_container(node)?;
        let top_level_module = if let Some(container) = container {
            self.ast(container)?.node(container)?.kind() == K::SourceFile
                && ts_ast::utilities::is_external_or_common_js_module(
                    &self.ast(container)?.source_file(container)?,
                )
        } else {
            false
        };
        let require_exports = format < ModuleKind::ES2015
            && (self.need_emit_collision_check(node, name, b"require")?
                || self.need_emit_collision_check(node, name, b"exports")?);
        let object = !class
            && format == ModuleKind::COMMON_JS
            && self.need_emit_collision_check(node, name, b"Object")?;
        if instantiated && top_level_module && (require_exports || object) {
            let text = ts_scanner::declaration_name_to_string(self.ast(name)?, Some(name))?;
            self.emit_skipped_error(
                name,
                d::Duplicate_identifier_0_Compiler_reserves_name_1_in_top_level_scope_of_a_module,
                vec![text.clone(), text],
            )?;
        }
        if self.program()?.host.options().emit_script_target() < ScriptTarget::ES2017
            && instantiated
            && top_level_module
            && self.need_emit_collision_check(node, name, b"Promise")?
        {
            let container = container.ok_or(Error::MissingLink("Promise declaration container"))?;
            if self.ast(container)?.node(container)?.flags() & nf::HAS_ASYNC_FUNCTIONS != 0 {
                let text = ts_scanner::declaration_name_to_string(self.ast(name)?, Some(name))?;
                self.emit_skipped_error(name,d::Duplicate_identifier_0_Compiler_reserves_name_1_in_top_level_scope_of_a_module_containing_async_functions,vec![text.clone(),text])?;
            }
        }
        if self.program()?.host.options().emit_script_target() <= ScriptTarget::ES2021 {
            if self.need_emit_collision_check(node, name, b"WeakMap")?
                || self.need_emit_collision_check(node, name, b"WeakSet")?
            {
                self.deferred_checks
                    .pending
                    .push(crate::deferred_checks::DeferredCheck::WeakMapSetCollision { node });
            }
            if self.need_emit_collision_check(node, name, b"Reflect")? {
                self.deferred_checks
                    .pending
                    .push(crate::deferred_checks::DeferredCheck::ReflectCollision { node });
            }
        }
        if class {
            self.check_module_reserved_type_name(name, d::Class_name_cannot_be_0)?;
            if self.ast(node)?.node(node)?.flags() & nf::AMBIENT == 0
                && format < ModuleKind::ES2015
                && self.ast(name)?.node_text(name)?.as_bytes() == b"Object"
            {
                self.error_at(
                    Some(name),
                    d::Class_name_cannot_be_Object_when_targeting_ES5_and_above_with_module_0,
                    vec![JsString::from_bytes(module_kind_text(
                        self.program()?.host.options().emit_module_kind(),
                    ))],
                )?;
            }
        } else if kind == K::EnumDeclaration {
            self.check_module_reserved_type_name(name, d::Enum_name_cannot_be_0)?;
        }
        Ok(())
    }

    fn emit_declaration_container(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let root = ts_ast::utilities::get_root_declaration(self.ast(node)?, node)?;
        let mut current = root;
        loop {
            let read = self.ast(current)?.node(current)?;
            if !matches!(
                read.kind().known(),
                Some(
                    K::VariableDeclaration
                        | K::VariableDeclarationList
                        | K::ImportSpecifier
                        | K::NamedImports
                        | K::NamespaceImport
                        | K::ImportClause
                )
            ) {
                return Ok(read.parent());
            }
            current = read
                .parent()
                .ok_or(Error::MissingLink("declaration container"))?;
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkWeakMapSetCollision
    pub(crate) fn check_weak_map_set_collision(&mut self, node: NodeId) -> Result<(), Error> {
        if let Some(scope) = self.enclosing_emit_block_scope(node)? {
            if *self.emit_checks.node_flags.get_or_default(scope)
                & nc::CONTAINS_CLASS_WITH_PRIVATE_IDENTIFIERS
                != 0
            {
                if let Some(name) = self.ast(node)?.node(node)?.name() {
                    if self.ast(name)?.node(name)?.kind() == K::Identifier {
                        let text = self.ast(name)?.node_text(name)?.into_js_string();
                        self.emit_skipped_error(
                            node,
                            d::Compiler_reserves_name_0_when_emitting_private_identifier_downlevel,
                            vec![text],
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkReflectCollision
    pub(crate) fn check_reflect_collision(&mut self, node: NodeId) -> Result<(), Error> {
        let kind = self.ast(node)?.node(node)?.kind();
        let scopes = if kind == K::ClassExpression {
            self.source_list(node, self.ast(node)?.node(node)?.member_list())?
        } else if kind == K::FunctionExpression {
            vec![node]
        } else {
            self.enclosing_emit_block_scope(node)?.into_iter().collect()
        };
        let collision = scopes.into_iter().any(|scope| {
            *self.emit_checks.node_flags.get_or_default(scope)
                & nc::CONTAINS_SUPER_PROPERTY_IN_STATIC_INITIALIZER
                != 0
        });
        if collision {
            if let Some(name) = self.ast(node)?.node(node)?.name() {
                if self.ast(name)?.node(name)?.kind() == K::Identifier {
                    let text = ts_scanner::declaration_name_to_string(self.ast(name)?, Some(name))?;
                    self.emit_skipped_error(node,d::Duplicate_identifier_0_Compiler_reserves_name_1_when_emitting_super_references_in_static_initializers,vec![text,JsString::from_bytes(b"Reflect".as_slice())])?;
                }
            }
        }
        Ok(())
    }

    fn emit_skipped_error(
        &mut self,
        node: NodeId,
        message: &'static d::Message,
        args: Vec<JsString>,
    ) -> Result<(), Error> {
        let mut diagnostic = self.diagnostic_for_node(Some(node), message, args)?;
        diagnostic.skipped_on_no_emit = true;
        self.add_diagnostic(diagnostic).map(|_| ())
    }

    // port: tsc/internal/checker/checker.go:Checker.getEffectivePropertyNameForPropertyNameNode
    pub(crate) fn effective_property_name(
        &mut self,
        node: NodeId,
    ) -> Result<Option<JsString>, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() != K::ComputedPropertyName {
            return Ok(Some(self.ast(node)?.node_text(node)?.into_js_string()));
        }
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("computed name expression"))?;
        let read = self.ast(expression)?.node(expression)?;
        if matches!(
            read.kind().known(),
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral | K::NumericLiteral)
        ) {
            return Ok(Some(
                self.ast(expression)?
                    .node_text(expression)?
                    .into_js_string(),
            ));
        }
        if read.kind() == K::PrefixUnaryExpression {
            let data = read
                .data_source()
                .as_prefix_unary_expression()
                .ok_or(Error::MissingLink("computed unary name"))?;
            let operator = data.operator();
            if let Some(operand) = data.operand() {
                if matches!(operator.known(), Some(K::PlusToken | K::MinusToken))
                    && self.ast(operand)?.node(operand)?.kind() == K::NumericLiteral
                {
                    let mut text = Vec::new();
                    if operator == K::MinusToken {
                        text.push(b'-');
                    }
                    text.extend_from_slice(self.ast(operand)?.node_text(operand)?.as_bytes());
                    return Ok(Some(JsString::from_bytes(text)));
                }
            }
        }
        if let Some(name) = self.emit_checks.computed_names.try_get(node) {
            return Ok(name.clone());
        }
        let ty = self.get_type_of_expression(expression)?;
        let name = self.index_property_name(ty)?;
        self.emit_checks
            .computed_names
            .get_or_default(node)
            .clone_from(&name);
        Ok(name)
    }
}

// port: tsc/internal/checker/checker.go:Checker.getHelperNames
fn helper_names(helper: u32, legacy: bool) -> Result<&'static [&'static [u8]], Error> {
    Ok(match helper {
        eh::REST => &[b"__rest"],
        eh::DECORATE if legacy => &[b"__decorate"],
        eh::DECORATE => &[b"__esDecorate", b"__runInitializers"],
        eh::METADATA => &[b"__metadata"],
        eh::PARAM => &[b"__param"],
        eh::AWAITER => &[b"__awaiter"],
        eh::AWAIT => &[b"__await"],
        eh::ASYNC_GENERATOR => &[b"__asyncGenerator"],
        eh::ASYNC_DELEGATOR => &[b"__asyncDelegator"],
        eh::ASYNC_VALUES => &[b"__asyncValues"],
        eh::EXPORT_STAR => &[b"__exportStar"],
        eh::IMPORT_STAR => &[b"__importStar"],
        eh::IMPORT_DEFAULT => &[b"__importDefault"],
        eh::MAKE_TEMPLATE_OBJECT => &[b"__makeTemplateObject"],
        eh::CLASS_PRIVATE_FIELD_GET => &[b"__classPrivateFieldGet"],
        eh::CLASS_PRIVATE_FIELD_SET => &[b"__classPrivateFieldSet"],
        eh::CLASS_PRIVATE_FIELD_IN => &[b"__classPrivateFieldIn"],
        eh::SET_FUNCTION_NAME => &[b"__setFunctionName"],
        eh::PROP_KEY => &[b"__propKey"],
        eh::ADD_DISPOSABLE_RESOURCE_AND_DISPOSE_RESOURCES => {
            &[b"__addDisposableResource", b"__disposeResources"]
        }
        eh::REWRITE_RELATIVE_IMPORT_EXTENSION => &[b"__rewriteRelativeImportExtension"],
        _ => return Err(Error::MissingLink("recognized external emit helper")),
    })
}

// Source: tsc/internal/core/modulekind_stringer_generated.go:ModuleKind.String
pub(crate) fn module_kind_text(kind: ModuleKind) -> Vec<u8> {
    let text: &[u8] = match kind {
        ModuleKind::NONE => b"None",
        ModuleKind::COMMON_JS => b"CommonJS",
        ModuleKind::AMD => b"AMD",
        ModuleKind::UMD => b"UMD",
        ModuleKind::SYSTEM => b"System",
        ModuleKind::ES2015 => b"ES2015",
        ModuleKind::ES2020 => b"ES2020",
        ModuleKind::ES2022 => b"ES2022",
        ModuleKind::ESNEXT => b"ESNext",
        ModuleKind::NODE16 => b"Node16",
        ModuleKind::NODE18 => b"Node18",
        ModuleKind::NODE20 => b"Node20",
        ModuleKind::NODE_NEXT => b"NodeNext",
        ModuleKind::PRESERVE => b"Preserve",
        _ => return format!("ModuleKind({})", kind.0).into_bytes(),
    };
    text.to_vec()
}
