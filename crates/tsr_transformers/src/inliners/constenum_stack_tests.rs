//! Cold-owner recursive/continuation equivalence, including real replacements.
use super::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use tsr_arena::Counters;
use tsr_ast::{AstBuilder, SourceFileParseOptions};
use tsr_core::{NewLineKind, ScriptKind};
use tsr_jsstring::SourceText;
use tsr_printer::emit_resolver::EnumMemberValue;
use tsr_printer::script_resolver::{
    EmitResolver, ReferenceResolver, ResolverResult, TypeReferenceSerializationKind,
};
use tsr_printer::{EmitContext, Printer, PrinterOptions};

struct Constants {
    values: VecDeque<ResolverResult<Option<ConstantValue>>>,
    queries: Vec<NodeId>,
}
impl ReferenceResolver for Constants {
    fn get_referenced_export_container(
        &mut self,
        _: NodeId,
        _: bool,
    ) -> ResolverResult<Option<NodeId>> {
        unreachable!()
    }
    fn get_referenced_import_declaration(&mut self, _: NodeId) -> ResolverResult<Option<NodeId>> {
        unreachable!()
    }
    fn get_referenced_value_declaration(&mut self, _: NodeId) -> ResolverResult<Option<NodeId>> {
        unreachable!()
    }
    fn get_referenced_value_declarations(
        &mut self,
        _: NodeId,
    ) -> ResolverResult<Option<Vec<NodeId>>> {
        unreachable!()
    }
    fn get_element_access_expression_name(&mut self, _: NodeId) -> ResolverResult<JsString> {
        unreachable!()
    }
    fn get_referenced_member_value_declaration(
        &mut self,
        _: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        unreachable!()
    }
}
impl EmitResolver for Constants {
    fn is_referenced_alias_declaration(&mut self, _: NodeId) -> ResolverResult<bool> {
        unreachable!()
    }
    fn is_value_alias_declaration(&mut self, _: NodeId) -> ResolverResult<bool> {
        unreachable!()
    }
    fn is_top_level_value_import_equals_with_entity_name(
        &mut self,
        _: NodeId,
    ) -> ResolverResult<bool> {
        unreachable!()
    }
    fn mark_linked_references_recursively(&mut self, _: NodeId) -> ResolverResult<()> {
        unreachable!()
    }
    fn get_external_module_file_from_declaration(
        &mut self,
        _: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        unreachable!()
    }
    fn get_effective_declaration_flags(&mut self, _: NodeId, _: u32) -> ResolverResult<u32> {
        unreachable!()
    }
    fn get_type_reference_serialization_kind(
        &mut self,
        _: Option<NodeId>,
        _: Option<NodeId>,
    ) -> ResolverResult<TypeReferenceSerializationKind> {
        unreachable!()
    }
    fn get_constant_value(&mut self, node: NodeId) -> ResolverResult<Option<ConstantValue>> {
        self.queries.push(node);
        self.values.pop_front().expect("declared resolver query")
    }
    fn get_enum_member_value(&mut self, _: NodeId) -> ResolverResult<EnumMemberValue> {
        unreachable!()
    }
    fn get_jsx_factory_entity(&mut self, _: NodeId) -> ResolverResult<Option<Vec<JsString>>> {
        unreachable!()
    }
    fn get_jsx_fragment_factory_entity(
        &mut self,
        _: NodeId,
    ) -> ResolverResult<Option<Vec<JsString>>> {
        unreachable!()
    }
    fn treat_as_parse_tree_identifier(
        &mut self,
        _: NodeId,
        _: &[u8],
        _: Option<NodeId>,
    ) -> ResolverResult<()> {
        unreachable!()
    }
    fn set_referenced_import_declaration(&mut self, _: NodeId, _: NodeId) -> ResolverResult<()> {
        unreachable!()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    output: Vec<u8>,
    queries: Vec<(i32, i32)>,
    failure: Option<String>,
}

fn observe(text: &str, iterative: bool, fail: bool, erase: bool) -> Observation {
    let parsed = tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.as_bytes()),
        ScriptKind::TS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/main.ts".as_slice()),
            path: JsString::from_bytes(b"/main.ts".as_slice()),
            ..Default::default()
        },
    );
    let context = EmitContext::new();
    let mut factory = AstBuilder::with_hooks(
        SourceText::default(),
        &Counters::new(),
        context.factory_hooks(),
    );
    let mut root = parsed.root();
    factory.retain_file(parsed.publish_unbound());
    let values = VecDeque::from([
        Ok(Some(ConstantValue::Number(tsr_jsnum::Number::new(1.0)))),
        if fail {
            Err(tsr_arena::Error::InvalidGraph.into())
        } else {
            Ok(Some(ConstantValue::String(JsString::from_bytes(
                b"value".as_slice(),
            ))))
        },
        Ok(None),
    ]);
    let resolver = Rc::new(RefCell::new(Constants {
        values,
        queries: Vec::new(),
    }));
    let mut opts = crate::transformer_tests::options(&context);
    opts.emit_resolver = resolver.clone();
    if erase {
        root = crate::tstransforms::typeeraser::new_type_eraser_transformer(&opts)
            .unwrap()
            .transform_source_file(&mut factory, root)
            .unwrap();
    }
    let tx = ConstEnumInliningTransformer {
        opts: opts.clone(),
        iterative_spines: iterative,
    };
    let transformer = Transformer::new(
        |visitor: &mut NodeVisitor<'_>, node| tx.visit(visitor, node),
        Some(context.clone()),
        opts.failure.clone(),
    );
    let result = transformer.visitor(&mut factory).visit_source_file(root);
    let failure = opts.failure.take().map(|error| error.to_string());
    drop(transformer);
    let view = factory.view();
    let queries = resolver
        .borrow()
        .queries
        .iter()
        .map(|&node| {
            let read = view.node(node).unwrap();
            (read.pos(), read.end())
        })
        .collect();
    let output = Printer::new(
        PrinterOptions {
            new_line: NewLineKind::LF,
            ..Default::default()
        },
        &context,
    )
    .emit_source_file(view, result)
    .unwrap();
    Observation {
        output,
        queries,
        failure,
    }
}

#[test]
fn wasm_constenum_spines_preserve_rewrites_comments_and_query_order() {
    for text in [
        "const x = E.a + (E.b + E.c);",
        "const x = (E.a + E.b) * E.c;",
        "const x = E['*/'] + E.b - E.c;",
        "const x = (() => E.a + E.b) + (function(p = E.c) { return p; });",
    ] {
        let recursive = observe(text, false, false, false);
        let iterative = observe(text, true, false, false);
        assert_eq!(iterative, recursive, "{text}");
        assert_eq!(iterative.queries.len(), 3);
        assert!(iterative.failure.is_none());
        assert!(iterative
            .output
            .windows(b"value".len())
            .any(|part| part == b"value"));
    }
}

#[test]
fn wasm_constenum_spines_preserve_first_failure_and_partial_updates() {
    let text = "const x = (E.a + E.b) + E.c;";
    let recursive = observe(text, false, true, false);
    let iterative = observe(text, true, true, false);
    assert_eq!(iterative, recursive);
    assert_eq!(iterative.queries.len(), 2);
    assert!(iterative.failure.is_some());
}

#[test]
fn wasm_constenum_spines_preserve_partially_emitted_nodes() {
    let text = "const x = ((E.a as number) + (E.b as string)) + E.c;";
    assert_eq!(
        observe(text, true, false, true),
        observe(text, false, false, true)
    );
}

#[test]
fn wasm_constenum_spines_visit_five_thousand_binary_levels() {
    let text = format!(
        "const x = {};",
        std::iter::repeat_n("1", 5_001)
            .collect::<Vec<_>>()
            .join(" + ")
    );
    let parsed = tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.as_bytes()),
        ScriptKind::TS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/main.ts".as_slice()),
            path: JsString::from_bytes(b"/main.ts".as_slice()),
            ..Default::default()
        },
    );
    assert!(parsed
        .view()
        .source_file(parsed.root())
        .unwrap()
        .diagnostics()
        .is_empty());
    let context = EmitContext::new();
    let mut factory = AstBuilder::with_hooks(
        SourceText::default(),
        &Counters::new(),
        context.factory_hooks(),
    );
    let root = parsed.root();
    factory.retain_file(parsed.publish_unbound());
    let opts = crate::transformer_tests::options(&context);
    let tx = ConstEnumInliningTransformer {
        opts: opts.clone(),
        iterative_spines: true,
    };
    let leaves = Cell::new(0);
    let transformer = Transformer::new(
        |visitor: &mut NodeVisitor<'_>, node| {
            if node.is_some_and(|id| visitor.factory().node(id).kind() == K::NumericLiteral) {
                leaves.set(leaves.get() + 1);
            }
            tx.visit(visitor, node)
        },
        Some(context),
        opts.failure.clone(),
    );
    assert_eq!(
        transformer
            .transform_source_file(&mut factory, root)
            .unwrap(),
        root
    );
    assert_eq!(leaves.get(), 5_001);
    assert!(!opts.failure.is_set());
}
