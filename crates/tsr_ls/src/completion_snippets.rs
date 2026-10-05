use crate::{
    completion_context::{Container, Context},
    syntax::Syntax,
    CompletionOptions, LanguageService, Result,
};
use tsr_ast::{AstBuilder, FactoryMethods, NodeId, NodeListId, NodeSlice, SyntaxKind as K};
use tsr_checker::{Operation, SymbolRef};
use tsr_lsproto as lsp;
use tsr_printer::{EmitContext, Printer, PrinterOptions};

pub(super) fn list(ast: &mut AstBuilder, nodes: &[Option<NodeId>]) -> Result<NodeListId> {
    let nodes = ast.node_slice_from_slice(nodes)?;
    Ok(ast.new_list(tsr_core::TextRange::new(-1, -1), nodes)?)
}
pub(super) fn body(ast: &mut AstBuilder, emit: &mut EmitContext, snippet: bool) -> Result<NodeId> {
    let statements = if snippet {
        let stop = ast.new_empty_statement();
        emit.set_snippet_element(
            stop,
            tsr_printer::SnippetElement {
                kind: tsr_printer::SnippetKind::TabStop,
                order: 0,
            },
        );
        list(ast, &[Some(stop)])?
    } else {
        ast.new_list(tsr_core::TextRange::new(-1, -1), NodeSlice::empty())?
    };
    Ok(ast.new_block(Some(statements), true))
}
pub(super) fn settings(
    options: &CompletionOptions,
    syntax: &Syntax<'_>,
) -> Result<tsr_format::FormatCodeSettings> {
    let mut settings = options.format.clone();
    if let Some(newline) = &options.newline {
        settings.editor.new_line_character = newline.as_bytes().to_vec();
    }
    let mut jsdoc = tsr_ast::EagerJsDocProvider::default();
    let mut file = tsr_format::FormatFile {
        view: syntax.view,
        source: syntax.source,
        jsdoc: &mut jsdoc,
    };
    if settings.semicolons == tsr_format::SemicolonPreference::Ignore
        && !file.probably_uses_semicolons()?
    {
        settings.semicolons = tsr_format::SemicolonPreference::Remove;
    }
    Ok(settings)
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/completions.go:LanguageService.createObjectLiteralMethod
    // port: tsc/internal/ls/completions.go:LanguageService.getEntryForObjectLiteralMethodCompletion
    pub(crate) fn object_method_snippet(
        &self,
        checker: &mut Operation<'_>,
        syntax: &Syntax<'_>,
        context: &Context,
        symbol: SymbolRef,
        mut item: lsp::CompletionItem,
        options: &CompletionOptions,
    ) -> Result<Option<lsp::CompletionItem>> {
        let Some((Container::Object, enclosing)) = context.container else {
            return Ok(None);
        };
        if !options.object_method_snippets || syntax.file.is_js() {
            return Ok(None);
        }
        let Some(declaration) = checker.symbol_declarations(symbol)?.iter().flatten().next() else {
            return Ok(None);
        };
        let view = self.view(declaration)?;
        let read = view.node(declaration)?;
        if !matches!(
            read.kind().known(),
            Some(
                K::PropertySignature
                    | K::PropertyDeclaration
                    | K::MethodSignature
                    | K::MethodDeclaration
            )
        ) {
            return Ok(None);
        }
        let name = read.name();
        let ty = checker.get_type_of_symbol_at_location(symbol, Some(enclosing))?;
        let mut ty = checker.get_widened_type(ty)?;
        if checker.type_flags(ty)? & tsr_checker::type_flags::UNION != 0 {
            let types = checker.constituents(ty)?;
            if types.len() < 10 {
                ty = checker.union_type_with(&types, tsr_checker::UnionReduction::Subtype)?;
            }
        }
        if checker.type_flags(ty)? & tsr_checker::type_flags::UNION != 0 {
            let mut function = None;
            for part in checker.constituents(ty)? {
                if !checker
                    .get_signatures_of_type(part, tsr_checker::SignatureKind::Call)?
                    .is_empty()
                    && function.replace(part).is_some()
                {
                    return Ok(None);
                }
            }
            let Some(function) = function else {
                return Ok(None);
            };
            ty = function;
        }
        if checker
            .get_signatures_of_type(ty, tsr_checker::SignatureKind::Call)?
            .len()
            != 1
        {
            return Ok(None);
        }
        let mut builder = checker.node_builder();
        let flags = tsr_nodebuilder::flags::OMIT_THIS_PARAMETER
            | if options.quote == crate::QuotePreference::Single {
                tsr_nodebuilder::flags::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE
            } else {
                0
            };
        let Some(function) = builder.type_to_type_node(ty, Some(enclosing), flags, 0)? else {
            return Ok(None);
        };
        if builder.view().node(function)?.kind() != K::FunctionType {
            return Ok(None);
        }
        let mut generated = builder.into_syntax();
        // The declaration can belong to a different program file from the
        // enclosing expression. Retain its syntax before cloning its name.
        if let Some(file) = self.program.file_of_node(declaration) {
            generated.ast.retain_completed(file.bound());
        }
        let ast = &mut generated.ast;
        let parameters: Vec<_> = ast
            .view()
            .node_slice(ast.view().node(function)?.parameters(ast.view())?)?
            .iter()
            .flatten()
            .collect();
        let mut untyped = Vec::new();
        for parameter in parameters {
            let (rest, name, initializer) = {
                let read = ast.view().node(parameter)?;
                let data = read.data_source();
                let data = data.as_parameter_declaration().unwrap();
                (data.dot_dot_dot_token(), data.name(), data.initializer())
            };
            let rest = tsr_ast::deep_clone_node(ast, rest);
            let name = tsr_ast::deep_clone_node(ast, name);
            let initializer = tsr_ast::deep_clone_node(ast, initializer);
            untyped.push(Some(ast.new_parameter_declaration(
                None,
                rest,
                name,
                None,
                None,
                initializer,
            )));
        }
        let parameters = list(ast, &untyped)?;
        let name = tsr_ast::deep_clone_node(ast, name);
        let empty_name = ast.new_identifier(tsr_ast::JsString::default());
        let signature = ast.new_method_signature_declaration(
            None,
            Some(empty_name),
            None,
            None,
            Some(parameters),
            None,
        );
        let detail = Printer::new(
            PrinterOptions {
                remove_comments: true,
                omit_trailing_semicolon: true,
                ..Default::default()
            },
            &generated.emit,
        )
        .emit(ast.view(), signature, Some(syntax.source))?;
        let body = body(ast, &mut generated.emit, options.snippets)?;
        let method = ast.new_method_declaration(
            None,
            None,
            name,
            None,
            None,
            Some(parameters),
            None,
            None,
            Some(body),
        );
        let text = crate::snippet_printer::print(
            ast,
            method,
            &generated.emit,
            &syntax.file,
            &settings(options, syntax)?,
        )? + ",";
        let optional = item.label.ends_with('?');
        if optional {
            item.label.pop();
        }
        if options.label_details {
            item.label_details = Some(Box::new(lsp::CompletionItemLabelDetails {
                detail: Some(Box::new(String::from_utf8_lossy(&detail).into_owned())),
                ..Default::default()
            }));
        } else {
            item.label.push_str(&String::from_utf8_lossy(&detail));
        }
        item.data.as_mut().unwrap().name.clone_from(&item.label);
        if optional {
            item.label.push('?');
        }
        item.filter_text = Some(Box::new(if optional && options.snippets {
            item.data.as_ref().unwrap().name.clone()
        } else {
            text.clone()
        }));
        item.insert_text = Some(Box::new(text));
        item.insert_text_format = options
            .snippets
            .then(|| Box::new(lsp::InsertTextFormat::SNIPPET));
        item.sort_text = Some(Box::new(format!(
            "{}1",
            item.sort_text.as_deref().map_or("11", String::as_str)
        )));
        item.data.as_mut().unwrap().source = "ObjectLiteralMethodSnippet/".into();
        Ok(Some(item))
    }
}
