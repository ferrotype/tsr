//! `transformers/estransforms/taggedtemplate.go`: a tagged template with an
//! invalid escape (allowed in tagged templates since ES2018) becomes a call
//! with a `__makeTemplateObject` template object.
use super::utilities::{subtree_facts, view};
use crate::transformer::{Error, Failure, TransformOptions, Transformer};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tsr_ast::{
    node_flags, subtree_flags, token_flags, Factory, FactoryMethods, JsString, NodeId, NodeVisitor,
    RuntimeFactory, SyntaxKind as K,
};
use tsr_printer::EmitContext;

/// `newlineNormalizer`: `<CR><LF>` and `<CR>` to `<LF>`.
fn normalize_newlines(text: &[u8]) -> Vec<u8> {
    let mut normalized = Vec::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if text[index] == b'\r' {
            normalized.push(b'\n');
            index += if text.get(index + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
        } else {
            normalized.push(text[index]);
            index += 1;
        }
    }
    normalized
}

struct TaggedTemplateTransformer {
    emit_context: RefCell<EmitContext>,
    failure: Failure,
    current_source_file: Cell<Option<NodeId>>,

    tagged_template_string_declarations: RefCell<Vec<NodeId>>,
}

// port: tsc/internal/transformers/estransforms/taggedtemplate.go:newTaggedTemplateLiftRestrictionTransformer
pub fn new_tagged_template_lift_restriction_transformer<'a>(
    opts: &TransformOptions<'a>,
) -> Option<Transformer<'a>> {
    let tx = Rc::new(TaggedTemplateTransformer {
        emit_context: RefCell::new(opts.context.clone()),
        failure: opts.failure.clone(),
        current_source_file: Cell::new(None),
        tagged_template_string_declarations: RefCell::new(Vec::new()),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}

impl TaggedTemplateTransformer {
    // port: tsc/internal/transformers/estransforms/taggedtemplate.go:taggedTemplateTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        let id = nil_checked(node);
        let Some(facts) = self.failure.ok(subtree_facts(visitor.factory(), id)) else {
            return node;
        };
        if facts & subtree_flags::INVALID_TEMPLATE_ESCAPE == 0 {
            return node;
        }
        match visitor.factory().node(id).kind().known() {
            Some(K::SourceFile) => Some(self.visit_source_file(visitor, id)),
            Some(K::TaggedTemplateExpression) => {
                Some(self.visit_tagged_template_expression(visitor, id))
            }
            _ => visitor.visit_each_child(node),
        }
    }

    // port: tsc/internal/transformers/estransforms/taggedtemplate.go:taggedTemplateTransformer.visitSourceFile
    fn visit_source_file(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        self.current_source_file.set(Some(node));
        self.tagged_template_string_declarations
            .borrow_mut()
            .clear();
        let mut visited = nil_checked(visitor.visit_each_child(Some(node)));
        if self.failure.is_set() {
            return node;
        }

        let declarations = self.tagged_template_string_declarations.take();
        if !declarations.is_empty() {
            let factory = visitor.factory_mut();
            let (statements, end_of_file_token) = source_file_parts(factory, visited);
            let mut statements = list_nodes(factory, nil_checked_list(statements));
            let declaration_list = new_node_list(factory, declarations);
            let declaration_list =
                factory.new_variable_declaration_list(Some(declaration_list), node_flags::NONE);
            statements.push(factory.new_variable_statement(None, Some(declaration_list)));
            let stmt_list = new_node_list(factory, statements);
            let original_statements = nil_checked_list(source_file_parts(factory, node).0);
            let loc = factory.read_list(original_statements).loc();
            factory.set_list_location(stmt_list, loc);
            visited = factory.update_source(visited, Some(stmt_list), end_of_file_token);
        }

        let mut emit_context = self.emit_context.borrow_mut();
        let helpers = emit_context.read_emit_helpers();
        emit_context.add_emit_helper(visited, &helpers);
        visited
    }

    // port: tsc/internal/transformers/estransforms/taggedtemplate.go:taggedTemplateTransformer.visitTaggedTemplateExpression
    fn visit_tagged_template_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        self.process_tagged_template_expression(visitor, node)
    }

    // port: tsc/internal/transformers/estransforms/taggedtemplate.go:taggedTemplateTransformer.processTaggedTemplateExpression
    fn process_tagged_template_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (node_tag, template) = {
            let read = visitor.factory().node(node);
            let data = read
                .as_tagged_template_expression()
                .expect("TaggedTemplateExpression payload");
            (data.tag(), nil_checked(data.template()))
        };
        let tag = visitor.visit_node(node_tag);

        if !has_invalid_escape(visitor.factory(), template) {
            return nil_checked(visitor.visit_each_child(Some(node)));
        }

        // Build up the template arguments and the raw and cooked strings for the template.
        let mut template_arguments: Vec<Option<NodeId>> = vec![None]; // placeholder for the template object
        let mut cooked_strings = Vec::new();
        let mut raw_strings = Vec::new();

        if tsr_ast::is_no_substitution_template_literal(&visitor.factory().node(template)) {
            let emit_context = self.emit_context.borrow();
            cooked_strings.push(create_template_cooked(
                &emit_context,
                visitor.factory_mut(),
                template,
            ));
            let Some(raw) = self
                .failure
                .ok(get_raw_literal(visitor.factory_mut(), template))
            else {
                return node;
            };
            raw_strings.push(raw);
        } else {
            let (head, spans) = {
                let read = visitor.factory().node(template);
                let data = read
                    .as_template_expression()
                    .expect("TemplateExpression payload");
                (
                    nil_checked(data.head()),
                    nil_checked_list(data.template_spans()),
                )
            };
            {
                let emit_context = self.emit_context.borrow();
                cooked_strings.push(create_template_cooked(
                    &emit_context,
                    visitor.factory_mut(),
                    head,
                ));
            }
            let Some(raw) = self
                .failure
                .ok(get_raw_literal(visitor.factory_mut(), head))
            else {
                return node;
            };
            raw_strings.push(raw);
            for span in list_nodes(visitor.factory(), spans) {
                let (expression, literal) = {
                    let read = visitor.factory().node(span);
                    let data = read.as_template_span().expect("TemplateSpan payload");
                    (data.expression(), nil_checked(data.literal()))
                };
                {
                    let emit_context = self.emit_context.borrow();
                    cooked_strings.push(create_template_cooked(
                        &emit_context,
                        visitor.factory_mut(),
                        literal,
                    ));
                }
                let Some(raw) = self
                    .failure
                    .ok(get_raw_literal(visitor.factory_mut(), literal))
                else {
                    return node;
                };
                raw_strings.push(raw);
                template_arguments.push(visitor.visit_node(expression));
            }
        }

        let factory = visitor.factory_mut();
        let mut emit_context = self.emit_context.borrow_mut();
        let cooked = new_node_list(factory, cooked_strings);
        let cooked = factory.new_array_literal_expression(Some(cooked), false);
        let raw = new_node_list(factory, raw_strings);
        let raw = factory.new_array_literal_expression(Some(raw), false);
        let helper_call = emit_context.new_template_object_helper(factory, cooked, raw);

        // Create a variable to cache the template object if we're in a module.
        // Do not do this in the global scope, as any variable we currently generate could conflict with
        // variables from outside of the current compilation. In the future, we can revisit this behavior.
        let current_source_file = nil_checked(self.current_source_file.get());
        let is_external_module = match factory.read_source_file(current_source_file) {
            Ok(file) => tsr_ast::utilities::is_external_module(&file),
            Err(error) => {
                self.failure.record(error);
                return node;
            }
        };
        if is_external_module {
            let temp_var =
                emit_context.new_unique_name(factory, JsString::from_bytes(&b"templateObject"[..]));
            let declaration = factory.new_variable_declaration(Some(temp_var), None, None, None);
            self.tagged_template_string_declarations
                .borrow_mut()
                .push(declaration);
            let assignment = emit_context.new_assignment_expression(factory, temp_var, helper_call);
            template_arguments[0] =
                Some(emit_context.new_logical_or_expression(factory, temp_var, assignment));
        } else {
            template_arguments[0] = Some(helper_call);
        }

        let arguments = factory.alloc_nodes(template_arguments);
        let arguments = factory.alloc_list(tsr_core::TextRange::new(-1, -1), arguments);
        let call = factory.new_call_expression(tag, None, None, Some(arguments), node_flags::NONE);
        let loc = factory.node(node).range();
        factory.set_node_range(call, loc);
        call
    }
}

/// The fields of `TemplateLiteralLikeNodeBase`.
struct TemplateLiteralLike {
    text: JsString,
    raw_text: Vec<u8>,
    template_flags: i32,
}

/// `node.TemplateLiteralLikeData()`.
fn template_literal_like_data(factory: &dyn Factory, node: NodeId) -> TemplateLiteralLike {
    let read = factory.node(node);
    macro_rules! data {
        ($as:ident) => {{
            let data = read.$as().expect("template literal payload");
            TemplateLiteralLike {
                text: data.text_owned(),
                raw_text: data.raw_text().to_vec(),
                template_flags: data.template_flags(),
            }
        }};
    }
    match read.kind().known() {
        Some(K::NoSubstitutionTemplateLiteral) => data!(as_no_substitution_template_literal),
        Some(K::TemplateHead) => data!(as_template_head),
        Some(K::TemplateMiddle) => data!(as_template_middle),
        Some(K::TemplateTail) => data!(as_template_tail),
        _ => panic!("runtime error: invalid memory address or nil pointer dereference"),
    }
}

// port: tsc/internal/transformers/estransforms/taggedtemplate.go:createTemplateCooked
fn create_template_cooked(
    emit_context: &EmitContext,
    factory: &mut dyn Factory,
    template: NodeId,
) -> NodeId {
    let template = template_literal_like_data(factory, template);
    if template.template_flags & token_flags::IS_INVALID != 0 {
        return emit_context.new_void_zero_expression(factory);
    }
    factory.new_string_literal(template.text, token_flags::NONE)
}

// port: tsc/internal/transformers/estransforms/taggedtemplate.go:getRawLiteral
fn get_raw_literal(factory: &mut dyn RuntimeFactory, node: NodeId) -> Result<NodeId, Error> {
    let mut text = template_literal_like_data(factory, node).raw_text;
    if text.is_empty() {
        let view = view(factory)?;
        let source_file = tsr_ast::utilities::get_source_file_of_node(view, Some(node))?
            .expect("runtime error: invalid memory address or nil pointer dereference");
        text = tsr_scanner::get_source_text_of_node_from_source_file(
            view,
            source_file,
            Some(node),
            false, /*includeTrivia*/
        )?
        .as_bytes()
        .to_vec();
        // text contains the original source, it will also contain quotes ("`"), dollar signs and braces ("${" and "}"),
        // thus we need to remove those characters.
        // First template piece starts with "`", others with "}"
        // Last template piece ends with "`", others with "${"
        let kind = factory.node(node).kind();
        let is_last = kind == K::NoSubstitutionTemplateLiteral || kind == K::TemplateTail;
        let end_len = if is_last { 1 } else { 2 };
        let end = text
            .len()
            .checked_sub(end_len)
            .filter(|end| *end >= 1)
            .expect("runtime error: slice bounds out of range");
        text = text[1..end].to_vec();
    }

    // Newline normalization:
    // ES6 Spec 11.8.6.1 - Static Semantics of TV's and TRV's
    // <CR><LF> and <CR> LineTerminatorSequences are normalized to <LF> for both TV and TRV.
    let text = normalize_newlines(&text);

    let result = factory.new_string_literal(JsString::from_bytes(text), token_flags::NONE);
    let loc = factory.node(node).range();
    factory.set_node_range(result, loc);
    Ok(result)
}

// port: tsc/internal/transformers/estransforms/taggedtemplate.go:hasInvalidEscape
fn has_invalid_escape(factory: &dyn RuntimeFactory, template: NodeId) -> bool {
    if tsr_ast::is_no_substitution_template_literal(&factory.node(template)) {
        return template_literal_like_data(factory, template).template_flags
            & token_flags::CONTAINS_INVALID_ESCAPE
            != 0;
    }
    let (head, spans) = {
        let read = factory.node(template);
        let data = read
            .as_template_expression()
            .expect("TemplateExpression payload");
        (
            nil_checked(data.head()),
            nil_checked_list(data.template_spans()),
        )
    };
    if template_literal_like_data(factory, head).template_flags
        & token_flags::CONTAINS_INVALID_ESCAPE
        != 0
    {
        return true;
    }
    for span in list_nodes(factory, spans) {
        let literal = nil_checked(
            factory
                .node(span)
                .as_template_span()
                .expect("TemplateSpan payload")
                .literal(),
        );
        if template_literal_like_data(factory, literal).template_flags
            & token_flags::CONTAINS_INVALID_ESCAPE
            != 0
        {
            return true;
        }
    }
    false
}

fn nil_checked(node: Option<NodeId>) -> NodeId {
    node.expect("runtime error: invalid memory address or nil pointer dereference")
}

fn nil_checked_list(list: Option<tsr_ast::NodeListId>) -> tsr_ast::NodeListId {
    list.expect("runtime error: invalid memory address or nil pointer dereference")
}

/// A file's `Statements` and `EndOfFileToken`.
fn source_file_parts(
    factory: &dyn RuntimeFactory,
    file: NodeId,
) -> (Option<tsr_ast::NodeListId>, Option<NodeId>) {
    let read = factory.node(file);
    let data = read.as_source_file().expect("SourceFile payload");
    (data.statements(), data.end_of_file_token())
}

/// A list's nodes.
fn list_nodes(factory: &dyn RuntimeFactory, list: tsr_ast::NodeListId) -> Vec<NodeId> {
    let nodes = factory.read_list(list).nodes();
    factory.read_nodes(nodes).iter().map(nil_checked).collect()
}

/// `NodeFactory.NewNodeList`: an undefined location.
fn new_node_list(factory: &mut dyn RuntimeFactory, nodes: Vec<NodeId>) -> tsr_ast::NodeListId {
    let nodes = factory.alloc_nodes(nodes.into_iter().map(Some).collect());
    factory.alloc_list(tsr_core::TextRange::new(-1, -1), nodes)
}
