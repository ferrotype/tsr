//! The framework over a parsed file. Upstream has no tests for these files;
//! the expectations follow `transformer.go`, `chain.go` and
//! `modifiervisitor.go`.
use crate::{chain, extract_modifiers, Failure, TransformOptions, Transformer, TransformerFactory};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_ast::{
    modifier_flags, AstBuilder, Factory, JsString, NodeId, NodeVisitor, RuntimeFactory,
    SourceFileParseOptions, SyntaxKind as K,
};
use tsr_core::{CompilerOptions, ModuleKind, ScriptKind};
use tsr_jsstring::SourceText;
use tsr_printer::script_resolver::{
    EmitResolver, ReferenceResolver, ResolverResult, TypeReferenceSerializationKind,
};
use tsr_printer::{
    emit_resolver::{ConstantValue, EnumMemberValue},
    EmitContext,
};

/// A resolver no test here asks anything.
struct Unasked;
impl ReferenceResolver for Unasked {
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
impl EmitResolver for Unasked {
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
    fn get_constant_value(&mut self, _: NodeId) -> ResolverResult<Option<ConstantValue>> {
        unreachable!()
    }
    fn get_enum_member_value(&mut self, _: NodeId) -> ResolverResult<EnumMemberValue> {
        unreachable!()
    }
    fn get_jsx_factory_entity(&mut self, _: NodeId) -> ResolverResult<Option<NodeId>> {
        unreachable!()
    }
    fn get_jsx_fragment_factory_entity(&mut self, _: NodeId) -> ResolverResult<Option<NodeId>> {
        unreachable!()
    }
    fn set_referenced_import_declaration(&mut self, _: NodeId, _: NodeId) -> ResolverResult<()> {
        unreachable!()
    }
}

struct Fixture {
    context: EmitContext,
    factory: AstBuilder,
    root: NodeId,
}

fn parse(text: &str) -> Fixture {
    let context = EmitContext::new();
    let mut factory = AstBuilder::with_hooks(
        SourceText::default(),
        &Counters::new(),
        context.factory_hooks(),
    );
    let parsed = tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.as_bytes()),
        ScriptKind::TS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(&b"/main.ts"[..]),
            path: JsString::from_bytes(&b"/main.ts"[..]),
            ..Default::default()
        },
    );
    let root = parsed.root();
    factory.retain_file(parsed.publish_unbound());
    Fixture {
        context,
        factory,
        root,
    }
}

fn options<'a>(context: &EmitContext) -> TransformOptions<'a> {
    TransformOptions {
        context: context.clone(),
        compiler_options: Arc::new(CompilerOptions::default()),
        resolver: Rc::new(RefCell::new(Unasked)),
        emit_resolver: Rc::new(RefCell::new(Unasked)),
        get_emit_module_format_of_file: Rc::new(|_| Ok(ModuleKind::NONE)),
        failure: Failure::default(),
    }
}

fn statement_names(factory: &AstBuilder, file: NodeId) -> Vec<Vec<u8>> {
    let statements = Factory::node(factory, file)
        .as_source_file()
        .expect("source file")
        .statements()
        .expect("statements");
    let nodes = factory.read_list(statements).nodes();
    factory
        .read_nodes(nodes)
        .iter()
        .map(|statement| {
            let expression = Factory::node(factory, statement.unwrap())
                .expression()
                .unwrap();
            Factory::node(factory, expression)
                .as_identifier()
                .unwrap()
                .text()
                .to_vec()
        })
        .collect()
}

fn identifier_text(visitor: &NodeVisitor<'_>, statement: NodeId) -> Option<Vec<u8>> {
    let factory = visitor.factory();
    // The file's end-of-file token is visited too.
    if factory.node(statement).kind() != K::ExpressionStatement {
        return None;
    }
    let expression = factory.node(statement).expression()?;
    Some(factory.node(expression).as_identifier()?.text().to_vec())
}

/// Removes the expression statements that name `name`; records the names the
/// other statements have when it sees them.
fn remover<'a>(name: &'static [u8], seen: Rc<RefCell<Vec<Vec<u8>>>>) -> TransformerFactory<'a> {
    Box::new(move |opt| {
        let seen = seen.clone();
        Some(Transformer::new(
            move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
                let id = node?;
                if visitor.factory().node(id).kind() == K::SourceFile {
                    return visitor.visit_each_child(node);
                }
                match identifier_text(visitor, id) {
                    Some(text) if text == name => None,
                    Some(text) => {
                        seen.borrow_mut().push(text);
                        node
                    }
                    None => node,
                }
            },
            Some(opt.context.clone()),
            opt.failure.clone(),
        ))
    })
}

#[test]
fn a_chain_runs_its_transformers_one_file_pass_at_a_time_in_order() {
    let mut fixture = parse("a; b; c;");
    let (first, second) = (Rc::default(), Rc::default());
    let opt = options(&fixture.context);
    let tx = chain(vec![
        remover(b"b", Rc::clone(&first)),
        remover(b"c", Rc::clone(&second)),
    ])(&opt)
    .expect("a chain of two");
    let file = tx
        .transform_source_file(&mut fixture.factory, fixture.root)
        .expect("no failure");
    assert_eq!(statement_names(&fixture.factory, file), [b"a".to_vec()]);
    // The second transformer saw the first one's whole result, not the source.
    assert_eq!(*first.borrow(), [b"a".to_vec(), b"c".to_vec()]);
    assert_eq!(*second.borrow(), [b"a".to_vec()]);
    // An updated file keeps its original.
    assert_ne!(file, fixture.root);
    assert_eq!(fixture.context.most_original(file), fixture.root);
}

#[test]
fn a_chain_of_one_is_that_transformer_and_nil_transformers_are_dropped() {
    let mut fixture = parse("a; b;");
    let opt = options(&fixture.context);
    let none: TransformerFactory<'_> = Box::new(|_| None);
    let tx = chain(vec![none, remover(b"a", Rc::default())])(&opt).expect("one transformer");
    let file = tx
        .transform_source_file(&mut fixture.factory, fixture.root)
        .expect("no failure");
    assert_eq!(statement_names(&fixture.factory, file), [b"b".to_vec()]);
    let nothing: Vec<TransformerFactory<'_>> = vec![Box::new(|_| None), Box::new(|_| None)];
    assert!(chain(nothing)(&opt).is_none());
}

#[test]
#[should_panic(expected = "Expected some number of transforms to chain, but got none")]
fn an_empty_chain_panics() {
    let _ = chain(Vec::new());
}

#[test]
fn the_first_failure_stops_the_chain_and_no_root_escapes() {
    let mut fixture = parse("a; b;");
    let opt = options(&fixture.context);
    let ran = Rc::new(Cell::new(false));
    let failing: TransformerFactory<'_> = Box::new(|opt| {
        let failure = opt.failure.clone();
        Some(Transformer::new(
            move |_: &mut NodeVisitor<'_>, node: Option<NodeId>| {
                failure.record(tsr_arena::Error::InvalidGraph);
                failure.record(tsr_arena::Error::WrongOwner);
                node
            },
            Some(opt.context.clone()),
            opt.failure.clone(),
        ))
    });
    let observed = Rc::clone(&ran);
    let later: TransformerFactory<'_> = Box::new(move |opt| {
        let observed = Rc::clone(&observed);
        Some(Transformer::new(
            move |_: &mut NodeVisitor<'_>, node: Option<NodeId>| {
                observed.set(true);
                node
            },
            Some(opt.context.clone()),
            opt.failure.clone(),
        ))
    });
    let tx = chain(vec![failing, later])(&opt).expect("a chain of two");
    let error = tx
        .transform_source_file(&mut fixture.factory, fixture.root)
        .expect_err("the recorded failure");
    assert!(matches!(
        error,
        crate::Error::Arena(tsr_arena::Error::InvalidGraph)
    ));
    assert!(!ran.get());
    // The slot is empty again for the next file.
    assert!(!opt.failure.is_set());
}

#[test]
fn extract_modifiers_keeps_the_allowed_modifiers_in_order() {
    let context = EmitContext::new();
    let mut factory = AstBuilder::with_hooks(
        SourceText::default(),
        &Counters::new(),
        context.factory_hooks(),
    );
    let modifiers: Vec<_> = [K::PublicKeyword, K::StaticKeyword, K::ReadonlyKeyword]
        .into_iter()
        .map(|kind| Some(factory.new_modifier(kind.into())))
        .collect();
    let nodes = factory.alloc_nodes(modifiers.clone());
    let list = factory.new_modifier_list(nodes);

    let kept = extract_modifiers(
        &context,
        &mut factory,
        Some(list),
        modifier_flags::STATIC | modifier_flags::READONLY,
    )
    .expect("a list");
    let read = factory.read_list(kept);
    assert_eq!(
        factory.read_nodes(read.nodes()).iter().collect::<Vec<_>>(),
        modifiers[1..]
    );
    assert_eq!(
        read.modifier_flags(),
        modifier_flags::STATIC | modifier_flags::READONLY
    );

    // Nothing removed: the same list. A nil list stays nil.
    let all = modifier_flags::PUBLIC | modifier_flags::STATIC | modifier_flags::READONLY;
    assert_eq!(
        extract_modifiers(&context, &mut factory, Some(list), all),
        Some(list)
    );
    assert_eq!(extract_modifiers(&context, &mut factory, None, all), None);
}
