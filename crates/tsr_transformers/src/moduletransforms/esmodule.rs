//! `transformers/moduletransforms/esmodule.go`: an ECMAScript module file's
//! module syntax as the emit module kind needs it: `import x = require()` as
//! a `require` call (a `createRequire` one for the node module kinds),
//! `export =` as `module.exports =` under `--module preserve`, `export * as ns`
//! split for ES2015, rewritten relative extensions, the `tslib` import of
//! `--importHelpers` and the `export {}` of a module without module syntax.
use crate::estransforms::utilities::{list_nodes, new_node_list, view};
use crate::moduletransforms::externalmoduleinfo::create_external_helpers_import_declaration_if_needed;
use crate::moduletransforms::utilities::{
    create_empty_imports, get_external_module_name_literal, rewrite_module_specifier,
};
use crate::transformer::{EmitModuleFormatOfFile, Error, Failure, TransformOptions, Transformer};
use crate::utilities::single_or_many;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use tsr_ast::{
    modifier_flags, node_flags, token_flags, FactoryMethods, JsString, NodeId, NodeListId,
    NodeVisitor, SyntaxKind as K,
};
use tsr_core::{CompilerOptions, JsxEmit, ModuleKind};
use tsr_printer::{emit_flags, generated_identifier_flags as g, AutoGenerateOptions, EmitContext};

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// `ESModuleTransformer`. Upstream's `resolver` and `helperNameSubstitutions`
/// fields are never read.
struct ESModuleTransformer<'a> {
    context: EmitContext,
    compiler_options: Arc<CompilerOptions>,
    get_emit_module_format_of_file: EmitModuleFormatOfFile<'a>,
    failure: Failure,
    current_source_file: Cell<Option<NodeId>>,
    import_require_statements: RefCell<Option<ImportRequireStatements>>,
}

/// `importRequireStatements`.
#[derive(Clone)]
struct ImportRequireStatements {
    statements: Vec<NodeId>,
    require_helper_name: NodeId,
}

// port: tsc/internal/transformers/moduletransforms/esmodule.go:NewESModuleTransformer
pub fn new_es_module_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let tx = Rc::new(ESModuleTransformer {
        context: opts.context.clone(),
        compiler_options: Arc::clone(&opts.compiler_options),
        get_emit_module_format_of_file: Rc::clone(&opts.get_emit_module_format_of_file),
        failure: opts.failure.clone(),
        current_source_file: Cell::new(None),
        import_require_statements: RefCell::new(None),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}

impl ESModuleTransformer<'_> {
    fn context(&self) -> EmitContext {
        self.context.clone()
    }

    /// Visits source elements that are not top-level or top-level nested statements.
    // port: tsc/internal/transformers/moduletransforms/esmodule.go:ESModuleTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        let id = node.expect(NIL);
        let kind = visitor.factory().node(id).kind();
        match kind.known() {
            Some(K::SourceFile) => Some(self.visit_source_file(visitor, id)),
            Some(K::ImportDeclaration) => Some(self.visit_import_declaration(visitor, id)),
            Some(K::ImportEqualsDeclaration) => self.visit_import_equals_declaration(visitor, id),
            Some(K::ExportAssignment) => self.visit_export_assignment(visitor, id),
            Some(K::ExportDeclaration) => self.visit_export_declaration(visitor, id),
            Some(K::CallExpression) => Some(self.visit_call_expression(visitor, id)),
            _ => visitor.visit_each_child(node),
        }
    }

    // port: tsc/internal/transformers/moduletransforms/esmodule.go:ESModuleTransformer.visitSourceFile
    fn visit_source_file(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let facts = visitor.factory().read_source_file(node).map(|file| {
            (
                file.is_declaration_file,
                tsr_ast::utilities::is_external_module(&file),
                file.file_name().to_vec(),
            )
        });
        let Some((is_declaration_file, is_external_module, file_name)) = self.failure.ok(facts)
        else {
            return node;
        };
        if is_declaration_file || !(is_external_module || self.compiler_options.isolated_modules())
        {
            return node;
        }

        self.current_source_file.set(Some(node));
        *self.import_require_statements.borrow_mut() = None;

        let mut result = visitor.visit_each_child(Some(node)).expect(NIL);
        let mut context = self.context();
        let helpers = context.read_emit_helpers();
        context.add_emit_helper(result, &helpers);

        let Some(format) = self
            .failure
            .ok((self.get_emit_module_format_of_file)(&file_name))
        else {
            return node;
        };
        let external_helpers_import_declaration =
            create_external_helpers_import_declaration_if_needed(
                &mut context,
                visitor.factory_mut(),
                result,
                &self.compiler_options,
                format,
                false, /*hasExportStarsToExportValues*/
                false, /*hasImportStar*/
                false, /*hasImportDefault*/
            );
        let Some(external_helpers_import_declaration) =
            self.failure.ok(external_helpers_import_declaration)
        else {
            return node;
        };
        let end_of_file_token = visitor
            .factory()
            .node(node)
            .as_source_file()
            .expect("SourceFile payload")
            .end_of_file_token();
        if external_helpers_import_declaration.is_some()
            || self.import_require_statements.borrow().is_some()
        {
            let nodes = list_nodes(
                visitor.factory(),
                Some(source_file_statements(visitor, result)),
            );
            let (prologue, rest) = context.split_standard_prologue(visitor.factory(), &nodes);
            let (custom, rest) = context.split_custom_prologue(visitor.factory(), rest);
            let mut statements: Vec<Option<NodeId>> = prologue.iter().copied().map(Some).collect();
            statements.extend(custom.iter().copied().map(Some));
            if external_helpers_import_declaration.is_some() {
                // The helpers import must be visited so that `import x = require("tslib")`
                // (TypeScript-only syntax) is transformed to `const x = require("tslib")`
                // for CJS output files via visitImportEqualsDeclaration.
                statements.push(visitor.visit_node(external_helpers_import_declaration));
            }
            let import_require_statements = self.import_require_statements.borrow().clone();
            if let Some(import_require_statements) = import_require_statements {
                statements.extend(import_require_statements.statements.into_iter().map(Some));
            }
            statements.extend(rest.iter().copied().map(Some));
            result = update_statements(visitor, result, statements, end_of_file_token);
        }

        let is_external_module = visitor
            .factory()
            .read_source_file(result)
            .map(|file| tsr_ast::utilities::is_external_module(&file));
        let Some(is_external_module) = self.failure.ok(is_external_module) else {
            return node;
        };
        if is_external_module && self.compiler_options.emit_module_kind() != ModuleKind::PRESERVE {
            let statements = crate::utilities::list_nodes(
                visitor.factory(),
                source_file_statements(visitor, result),
            );
            let mut has_indicator = false;
            for statement in &statements {
                let indicator = view(visitor.factory()).and_then(|view| {
                    Ok(tsr_ast::utilities_modules::is_external_module_indicator(
                        view,
                        statement.expect(NIL),
                    )?)
                });
                let Some(indicator) = self.failure.ok(indicator) else {
                    return node;
                };
                if indicator {
                    has_indicator = true;
                    break;
                }
            }
            if !has_indicator {
                let mut statements = statements;
                statements.push(Some(create_empty_imports(visitor.factory_mut())));
                result = update_statements(visitor, result, statements, end_of_file_token);
            }
        }

        *self.import_require_statements.borrow_mut() = None;
        self.current_source_file.set(None);
        result
    }

    // port: tsc/internal/transformers/moduletransforms/esmodule.go:ESModuleTransformer.visitImportDeclaration
    fn visit_import_declaration(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        if !self
            .compiler_options
            .rewrite_relative_import_extensions
            .is_true()
        {
            return node;
        }
        let (import_clause, module_specifier, attributes) = {
            let read = visitor.factory().node(node);
            let data = read
                .as_import_declaration()
                .expect("ImportDeclaration payload");
            (
                data.import_clause(),
                data.module_specifier(),
                data.attributes(),
            )
        };
        let updated_module_specifier = rewrite_module_specifier(
            &mut self.context(),
            visitor.factory_mut(),
            module_specifier,
            &self.compiler_options,
        );
        let import_clause = visitor.visit_node(import_clause);
        let attributes = visitor.visit_node(attributes);
        visitor.factory_mut().update_import_declaration(
            node,
            None, /*modifiers*/
            import_clause,
            updated_module_specifier,
            attributes,
        )
    }

    // port: tsc/internal/transformers/moduletransforms/esmodule.go:ESModuleTransformer.visitImportEqualsDeclaration
    fn visit_import_equals_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        // Though an error in es2020 modules, in node-flavor es2020 modules, we can helpfully transform this to a synthetic `require` call
        // To give easy access to a synchronous `require` in node-flavor esm. We do the transform even in scenarios where we error, but `import.meta.url`
        // is available, just because the output is reasonable for a node-like runtime.
        if self.compiler_options.emit_module_kind() < ModuleKind::NODE16 {
            return None;
        }

        let is_external = view(visitor.factory()).and_then(|view| {
            Ok(
                tsr_ast::utilities_modules::is_external_module_import_equals_declaration(
                    view, node,
                )?,
            )
        });
        let Some(is_external) = self.failure.ok(is_external) else {
            return Some(node);
        };
        assert!(
            is_external,
            "import= for internal module references should be handled in an earlier transformer."
        );

        let name = visitor.factory().node(node).name().expect(NIL);
        let name = tsr_ast::clone_node(visitor.factory_mut(), name);
        let Some(require_call) = self.failure.ok(self.create_require_call(visitor, node)) else {
            return Some(node);
        };
        let factory = visitor.factory_mut();
        let declaration = factory.new_variable_declaration(
            Some(name),
            None, /*exclamationToken*/
            None, /*type*/
            Some(require_call),
        );
        let declarations = new_node_list(factory, vec![declaration]);
        let declaration_list =
            factory.new_variable_declaration_list(Some(declarations), node_flags::CONST);
        let var_statement =
            factory.new_variable_statement(None /*modifiers*/, Some(declaration_list));
        let mut context = self.context();
        context.set_original(var_statement, node);
        context.assign_comment_and_source_map_ranges(visitor.factory(), var_statement, node);

        let mut statements = vec![var_statement];
        let appended =
            Self::append_exports_of_import_equals_declaration(visitor, &mut statements, node);
        if self.failure.ok(appended).is_none() {
            return Some(node);
        }
        single_or_many(visitor.factory_mut(), Some(&statements))
    }

    // port: tsc/internal/transformers/moduletransforms/esmodule.go:ESModuleTransformer.appendExportsOfImportEqualsDeclaration
    fn append_exports_of_import_equals_declaration(
        visitor: &mut NodeVisitor<'_>,
        statements: &mut Vec<NodeId>,
        node: NodeId,
    ) -> Result<(), Error> {
        if tsr_ast::utilities::has_syntactic_modifier(
            view(visitor.factory())?,
            node,
            modifier_flags::EXPORT,
        )? {
            let name = visitor.factory().node(node).name().expect(NIL);
            let factory = visitor.factory_mut();
            let name = tsr_ast::clone_node(factory, name);
            let specifier = factory.new_export_specifier(
                false, /*isTypeOnly*/
                None,  /*propertyName*/
                Some(name),
            );
            let elements = new_node_list(factory, vec![specifier]);
            let named_exports = factory.new_named_exports(Some(elements));
            statements.push(factory.new_export_declaration(
                None,  /*modifiers*/
                false, /*isTypeOnly*/
                Some(named_exports),
                None, /*moduleSpecifier*/
                None, /*attributes*/
            ));
        }
        Ok(())
    }

    // port: tsc/internal/transformers/moduletransforms/esmodule.go:ESModuleTransformer.visitExportAssignment
    fn visit_export_assignment(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let (is_export_equals, expression) = {
            let read = visitor.factory().node(node);
            let data = read
                .as_export_assignment()
                .expect("ExportAssignment payload");
            (data.is_export_equals(), data.expression())
        };
        if !is_export_equals {
            return visitor.visit_each_child(Some(node));
        }
        if self.compiler_options.emit_module_kind() != ModuleKind::PRESERVE {
            // Elide `export=` as it is not legal with --module ES6
            return None;
        }
        let factory = visitor.factory_mut();
        let module = factory.new_identifier(JsString::from_bytes(&b"module"[..]));
        let exports = factory.new_identifier(JsString::from_bytes(&b"exports"[..]));
        let target = factory.new_property_access_expression(
            Some(module),
            None, /*questionDotToken*/
            Some(exports),
            node_flags::NONE,
        );
        let expression = visitor.visit_node(expression);
        let mut context = self.context();
        let assignment = context.new_assignment_expression(
            visitor.factory_mut(),
            target,
            expression.expect(NIL),
        );
        let statement = visitor
            .factory_mut()
            .new_expression_statement(Some(assignment));
        context.set_original(statement, node);
        Some(statement)
    }

    // port: tsc/internal/transformers/moduletransforms/esmodule.go:ESModuleTransformer.visitExportDeclaration
    fn visit_export_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let (module_specifier, export_clause, attributes) = {
            let read = visitor.factory().node(node);
            let data = read
                .as_export_declaration()
                .expect("ExportDeclaration payload");
            (
                data.module_specifier(),
                data.export_clause(),
                data.attributes(),
            )
        };
        if module_specifier.is_none() {
            return Some(node);
        }

        let mut context = self.context();
        let updated_module_specifier = rewrite_module_specifier(
            &mut context,
            visitor.factory_mut(),
            module_specifier,
            &self.compiler_options,
        );
        if self.compiler_options.module > ModuleKind::ES2015
            || export_clause.is_none()
            || !tsr_ast::is_namespace_export(&visitor.factory().node(export_clause.expect(NIL)))
        {
            // Either ill-formed or don't need to be transformed.
            let attributes = visitor.visit_node(attributes);
            return Some(visitor.factory_mut().update_export_declaration(
                node,
                None,  /*modifiers*/
                false, /*isTypeOnly*/
                export_clause,
                updated_module_specifier,
                attributes,
            ));
        }

        let export_clause = export_clause.expect(NIL);
        let old_identifier = visitor.factory().node(export_clause).name().expect(NIL);
        let synth_name = context.new_generated_name_for_node(visitor.factory_mut(), old_identifier);
        let factory = visitor.factory_mut();
        let namespace_import = factory.new_namespace_import(Some(synth_name));
        let import_clause = factory.new_import_clause(
            K::Unknown.into(), /*phaseModifier*/
            None,              /*name*/
            Some(namespace_import),
        );
        let attributes = visitor.visit_node(attributes);
        let import_decl = visitor.factory_mut().new_import_declaration(
            None, /*modifiers*/
            Some(import_clause),
            updated_module_specifier,
            attributes,
        );
        context.set_original(import_decl, export_clause);

        let is_default = view(visitor.factory()).and_then(|view| {
            Ok(tsr_ast::utilities_modules::is_export_namespace_as_default_declaration(view, node)?)
        });
        let Some(is_default) = self.failure.ok(is_default) else {
            return Some(node);
        };
        let factory = visitor.factory_mut();
        let export_decl = if is_default {
            factory.new_export_assignment(
                None,  /*modifiers*/
                false, /*isExportEquals*/
                None,  /*typeNode*/
                Some(synth_name),
            )
        } else {
            let specifier = factory.new_export_specifier(
                false, /*isTypeOnly*/
                Some(synth_name),
                Some(old_identifier),
            );
            let elements = new_node_list(factory, vec![specifier]);
            let named_exports = factory.new_named_exports(Some(elements));
            factory.new_export_declaration(
                None,  /*modifiers*/
                false, /*isTypeOnly*/
                Some(named_exports),
                None, /*moduleSpecifier*/
                None, /*attributes*/
            )
        };
        context.set_original(export_decl, node);
        single_or_many(visitor.factory_mut(), Some(&[import_decl, export_decl]))
    }

    // port: tsc/internal/transformers/moduletransforms/esmodule.go:ESModuleTransformer.visitCallExpression
    fn visit_call_expression(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        if self
            .compiler_options
            .rewrite_relative_import_extensions
            .is_true()
        {
            let arguments = visitor
                .factory()
                .node(node)
                .as_call_expression()
                .expect("CallExpression payload")
                .arguments();
            let has_arguments = !list_nodes(visitor.factory(), arguments).is_empty();
            let matches = view(visitor.factory()).and_then(|view| {
                if tsr_ast::utilities_positions::is_import_call(view, node)? && has_arguments {
                    return Ok(true);
                }
                let read = view.node(node)?;
                Ok(tsr_ast::utilities::is_in_js_file(Some(&read))
                    && tsr_ast::utilities_middle::is_require_call(
                        view, &read, false, /*requireStringLiteralLikeArgument*/
                    )?)
            });
            let Some(matches) = self.failure.ok(matches) else {
                return node;
            };
            if matches {
                return self.visit_import_or_require_call(visitor, node);
            }
        }
        visitor.visit_each_child(Some(node)).expect(NIL)
    }

    // port: tsc/internal/transformers/moduletransforms/esmodule.go:ESModuleTransformer.visitImportOrRequireCall
    fn visit_import_or_require_call(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (expression, question_dot_token, arguments_list, flags) = {
            let read = visitor.factory().node(node);
            let data = read.as_call_expression().expect("CallExpression payload");
            (
                data.expression(),
                data.question_dot_token(),
                data.arguments().expect(NIL),
                read.flags(),
            )
        };
        let (arguments_loc, argument_nodes) = {
            let list = visitor.factory().read_list(arguments_list);
            (list.loc(), list.nodes())
        };
        if argument_nodes.is_empty() {
            return visitor.visit_each_child(Some(node)).expect(NIL);
        }

        let expression = visitor.visit_node(expression);

        let first = visitor
            .factory()
            .read_nodes(argument_nodes)
            .at(0)
            .expect(NIL);
        let mut context = self.context();
        let argument = if tsr_ast::utilities::is_string_literal_like(&visitor.factory().node(first))
        {
            rewrite_module_specifier(
                &mut context,
                visitor.factory_mut(),
                Some(first),
                &self.compiler_options,
            )
            .expect(NIL)
        } else {
            context.new_rewrite_relative_import_extensions_helper(
                visitor.factory_mut(),
                first,
                self.compiler_options.jsx == JsxEmit::PRESERVE,
            )
        };

        let mut arguments = vec![Some(argument)];

        let rest = argument_nodes
            .slice(1..argument_nodes.len())
            .expect("the arguments after the first");
        let (rest, _) = visitor.visit_slice(rest);
        arguments.extend(visitor.factory().read_nodes(rest).iter());

        let factory = visitor.factory_mut();
        let arguments = factory.alloc_nodes(arguments);
        let argument_list = factory.alloc_list(arguments_loc, arguments);
        factory.update_call_expression(
            node,
            expression,
            question_dot_token,
            None, /*typeArguments*/
            Some(argument_list),
            flags,
        )
    }

    // port: tsc/internal/transformers/moduletransforms/esmodule.go:ESModuleTransformer.createRequireCall
    fn create_require_call(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId, /*ImportDeclaration | ImportEqualsDeclaration | ExportDeclaration*/
    ) -> Result<NodeId, Error> {
        let module_name = get_external_module_name_literal(
            visitor.factory_mut(),
            node,
            self.current_source_file.get().expect(NIL),
            None, /*emitResolver*/
            &self.compiler_options,
        )?;

        let mut context = self.context();
        let mut args = Vec::new();
        if let Some(module_name) = module_name {
            args.push(
                rewrite_module_specifier(
                    &mut context,
                    visitor.factory_mut(),
                    Some(module_name),
                    &self.compiler_options,
                )
                .expect(NIL),
            );
        }

        if self.compiler_options.emit_module_kind() == ModuleKind::PRESERVE {
            let factory = visitor.factory_mut();
            let require = factory.new_identifier(JsString::from_bytes(&b"require"[..]));
            let args = new_node_list(factory, args);
            return Ok(factory.new_call_expression(
                Some(require),
                None, /*questionDotToken*/
                None, /*typeArguments*/
                Some(args),
                node_flags::NONE,
            ));
        }

        if self.import_require_statements.borrow().is_none() {
            let options = AutoGenerateOptions {
                flags: g::OPTIMISTIC | g::FILE_LEVEL,
                ..AutoGenerateOptions::default()
            };
            let create_require_name = context.new_unique_name_ex(
                visitor.factory_mut(),
                JsString::from_bytes(&b"_createRequire"[..]),
                options.clone(),
            );
            let factory = visitor.factory_mut();
            let create_require =
                factory.new_identifier(JsString::from_bytes(&b"createRequire"[..]));
            let specifier = factory.new_import_specifier(
                false, /*isTypeOnly*/
                Some(create_require),
                Some(create_require_name),
            );
            let elements = new_node_list(factory, vec![specifier]);
            let named_imports = factory.new_named_imports(Some(elements));
            let import_clause = factory.new_import_clause(
                K::Unknown.into(), /*phaseModifier*/
                None,              /*name*/
                Some(named_imports),
            );
            let module =
                factory.new_string_literal(JsString::from_bytes(&b"module"[..]), token_flags::NONE);
            let import_statement = factory.new_import_declaration(
                None, /*modifiers*/
                Some(import_clause),
                Some(module),
                None, /*attributes*/
            );
            context.add_emit_flags(import_statement, emit_flags::CUSTOM_PROLOGUE);

            let require_helper_name = context.new_unique_name_ex(
                visitor.factory_mut(),
                JsString::from_bytes(&b"__require"[..]),
                options,
            );
            let factory = visitor.factory_mut();
            let callee = tsr_ast::clone_node(factory, create_require_name);
            let meta = factory.new_identifier(JsString::from_bytes(&b"meta"[..]));
            let meta_property = factory.new_meta_property(K::ImportKeyword.into(), Some(meta));
            let url = factory.new_identifier(JsString::from_bytes(&b"url"[..]));
            let import_meta_url = factory.new_property_access_expression(
                Some(meta_property),
                None, /*questionDotToken*/
                Some(url),
                node_flags::NONE,
            );
            let call_arguments = new_node_list(factory, vec![import_meta_url]);
            let call = factory.new_call_expression(
                Some(callee),
                None, /*questionDotToken*/
                None, /*typeArguments*/
                Some(call_arguments),
                node_flags::NONE,
            );
            let declaration = factory.new_variable_declaration(
                Some(require_helper_name),
                None, /*exclamationToken*/
                None, /*type*/
                Some(call),
            );
            let declarations = new_node_list(factory, vec![declaration]);
            let declaration_list =
                factory.new_variable_declaration_list(Some(declarations), node_flags::CONST);
            let require_statement =
                factory.new_variable_statement(None /*modifiers*/, Some(declaration_list));
            context.add_emit_flags(require_statement, emit_flags::CUSTOM_PROLOGUE);
            *self.import_require_statements.borrow_mut() = Some(ImportRequireStatements {
                statements: vec![import_statement, require_statement],
                require_helper_name,
            });
        }

        let require_helper_name = self
            .import_require_statements
            .borrow()
            .as_ref()
            .expect(NIL)
            .require_helper_name;
        let factory = visitor.factory_mut();
        let callee = tsr_ast::clone_node(factory, require_helper_name);
        let args = new_node_list(factory, args);
        Ok(factory.new_call_expression(
            Some(callee),
            None, /*questionDotToken*/
            None, /*typeArguments*/
            Some(args),
            node_flags::NONE,
        ))
    }
}

/// `file.Statements` of a source file.
fn source_file_statements(visitor: &NodeVisitor<'_>, file: NodeId) -> NodeListId {
    visitor
        .factory()
        .node(file)
        .as_source_file()
        .expect("SourceFile payload")
        .statements()
        .expect(NIL)
}

/// `NewNodeList(statements)` at the location of `file.Statements`, then
/// `UpdateSourceFile(file, list, endOfFileToken)`.
fn update_statements(
    visitor: &mut NodeVisitor<'_>,
    file: NodeId,
    statements: Vec<Option<NodeId>>,
    end_of_file_token: Option<NodeId>,
) -> NodeId {
    let loc = visitor
        .factory()
        .read_list(source_file_statements(visitor, file))
        .loc();
    let factory = visitor.factory_mut();
    let nodes = factory.alloc_nodes(statements);
    let statement_list = factory.alloc_list(loc, nodes);
    factory.update_source(file, Some(statement_list), end_of_file_token)
}
