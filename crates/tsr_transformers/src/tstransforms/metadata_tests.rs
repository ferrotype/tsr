//! The metadata transformer over parsed files, with a resolver that answers
//! type-reference serialization kinds in query order. Upstream has no tests
//! for `metadata.go` or `typeserializer.go`; the native probes
//! (`tests/fixtures/phase3/transforms/metadata.*`) witness them against the
//! pin, and these expectations follow the Go source.
use super::new_metadata_transformer;
use crate::{Failure, TransformOptions};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_ast::{AstBuilder, Factory, JsString, NodeId, RuntimeFactory, SourceFileParseOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptKind, ScriptTarget, Tristate};
use tsr_jsstring::SourceText;
use tsr_printer::emit_resolver::{ConstantValue, EnumMemberValue};
use tsr_printer::script_resolver::{
    EmitResolver, ReferenceResolver, ResolverResult, TypeReferenceSerializationKind as Kind,
};
use tsr_printer::{EmitContext, EmitTextWriter, Printer, PrinterOptions, TextWriter};

/// Answers each type-reference query with the next kind; asks nothing else.
struct Kinds(VecDeque<Kind>);
impl ReferenceResolver for Kinds {
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
impl EmitResolver for Kinds {
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
        name: Option<NodeId>,
        scope: Option<NodeId>,
    ) -> ResolverResult<Kind> {
        assert!(name.is_some() && scope.is_some(), "parse-tree arguments");
        Ok(self.0.pop_front().expect("an expected query"))
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

/// The metadata transformer's result over `text`: the factory, the context
/// and the transformed file.
fn run(
    text: &str,
    target: ScriptTarget,
    legacy: bool,
    kinds: &[Kind],
) -> (AstBuilder, EmitContext, NodeId) {
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
    let compiler_options = CompilerOptions {
        experimental_decorators: if legacy {
            Tristate::TRUE
        } else {
            Tristate::FALSE
        },
        emit_decorator_metadata: Tristate::TRUE,
        target,
        ..CompilerOptions::default()
    };
    let resolver = Rc::new(RefCell::new(Kinds(kinds.iter().copied().collect())));
    let opts = TransformOptions {
        context: context.clone(),
        compiler_options: Arc::new(compiler_options),
        resolver: resolver.clone(),
        emit_resolver: resolver.clone(),
        get_emit_module_format_of_file: Rc::new(|_| Ok(ModuleKind::NONE)),
        failure: Failure::default(),
    };
    let tx = new_metadata_transformer(&opts).expect("a transformer");
    let file = tx
        .transform_source_file(&mut factory, root)
        .expect("no failure");
    assert!(resolver.borrow().0.is_empty(), "every expected query asked");
    drop(tx);
    drop(opts);
    (factory, context, file)
}

/// The printed result of the metadata transformer over `text`.
fn transform(text: &str, target: ScriptTarget, legacy: bool, kinds: &[Kind]) -> String {
    let (factory, context, file) = run(text, target, legacy, kinds);
    let mut writer = TextWriter::new(b"\n", 0);
    Printer::new(
        PrinterOptions {
            target,
            ..PrinterOptions::default()
        },
        &context,
    )
    .write(factory.view(), file, Some(file), &mut writer)
    .expect("printed");
    String::from_utf8(writer.text().to_vec()).expect("UTF-8")
}

/// The printed lines that carry metadata.
fn metadata_lines(printed: &str) -> Vec<String> {
    printed
        .lines()
        .filter(|line| line.contains("@__metadata(\"design"))
        .map(|line| line.trim().to_string())
        .collect()
}

#[test]
fn keyword_and_literal_types_serialize_to_their_constructors() {
    let printed = transform(
        "class C {\n    @dec a: string;\n    @dec b: 1;\n    @dec c: void;\n    @dec d: bigint;\n    @dec e: -1n;\n    @dec f: string[];\n    @dec g: (true);\n    @dec h;\n}\n",
        ScriptTarget::ES2015,
        true,
        &[],
    );
    assert_eq!(
        metadata_lines(&printed),
        [
            "@__metadata(\"design:type\", String)",
            "@__metadata(\"design:type\", Number)",
            "@__metadata(\"design:type\", void 0)",
            "@__metadata(\"design:type\", typeof BigInt === \"function\" ? BigInt : Object)",
            "@__metadata(\"design:type\", typeof BigInt === \"function\" ? BigInt : Object)",
            "@__metadata(\"design:type\", Array)",
            "@__metadata(\"design:type\", Boolean)",
            "@__metadata(\"design:type\", Object)",
        ]
    );
}

#[test]
fn the_file_carries_the_metadata_helper() {
    let (_, context, file) = run(
        "class C {\n    @dec a: string;\n}\n",
        ScriptTarget::ES2015,
        true,
        &[],
    );
    let helpers = context.get_emit_helpers(file);
    assert_eq!(helpers.len(), 1);
    assert_eq!(helpers[0].name, b"typescript:metadata");
}

#[test]
fn unions_reduce_to_one_constructor_or_object() {
    let printed = transform(
        "class C {\n    @dec a: string | null;\n    @dec b: string | number;\n    @dec c: never | unknown;\n    @dec d: string & never;\n    @dec e: null | undefined;\n}\n",
        ScriptTarget::ES2020,
        true,
        &[],
    );
    assert_eq!(
        metadata_lines(&printed),
        [
            // strictNullChecks follows `strict`, on by default: `null` is a constituent.
            "@__metadata(\"design:type\", Object)",
            "@__metadata(\"design:type\", Object)",
            "@__metadata(\"design:type\", Object)",
            "@__metadata(\"design:type\", void 0)",
            "@__metadata(\"design:type\", void 0)",
        ]
    );
}

#[test]
fn methods_record_type_parameter_types_and_return_type() {
    let printed = transform(
        "class C {\n    @dec m(this: C, x: string, ...y: number[]): boolean { return true; }\n    @dec async n() {}\n}\n",
        ScriptTarget::ES2015,
        true,
        &[],
    );
    assert_eq!(
        metadata_lines(&printed),
        [
            "@__metadata(\"design:type\", Function)",
            "@__metadata(\"design:paramtypes\", [String, Number])",
            "@__metadata(\"design:returntype\", Boolean)",
            "@__metadata(\"design:type\", Function)",
            "@__metadata(\"design:paramtypes\", [])",
            "@__metadata(\"design:returntype\", Promise)",
        ]
    );
}

#[test]
fn accessors_read_the_set_accessor_parameter() {
    let printed = transform(
        "class C {\n    @dec get a(): string { return \"\"; }\n    set a(v: number) {}\n    @dec get b(): boolean { return true; }\n}\n",
        ScriptTarget::ES2015,
        true,
        &[],
    );
    assert_eq!(
        metadata_lines(&printed),
        [
            "@__metadata(\"design:type\", Number)",
            "@__metadata(\"design:paramtypes\", [Number])",
            "@__metadata(\"design:type\", Boolean)",
            "@__metadata(\"design:paramtypes\", [])",
        ]
    );
}

#[test]
fn type_references_follow_the_resolver() {
    let printed = transform(
        "@dec\nexport class C {\n    constructor(a: A, b: N.B, c: S) {}\n    @dec x: P;\n}\n",
        ScriptTarget::ES2015,
        true,
        &[
            Kind::TypeWithConstructSignatureAndValue,
            Kind::TypeWithConstructSignatureAndValue,
            Kind::StringLikeType,
            Kind::Promise,
        ],
    );
    assert_eq!(
        metadata_lines(&printed),
        [
            "@__metadata(\"design:paramtypes\", [A, N.B, String])",
            "@__metadata(\"design:type\", Promise)",
        ]
    );
    assert!(
        printed.contains("@dec\n@__metadata(\"design:paramtypes\""),
        "{printed}"
    );
}

/// The printer does not print generated names yet; an unresolved reference
/// hoists its temps into one `var` statement at the top of the file.
#[test]
fn unresolved_references_hoist_their_temps() {
    let (factory, _, file) = run(
        "class C {\n    @dec x: Missing;\n    @dec y: P.Q.R;\n}\n",
        ScriptTarget::ES2015,
        true,
        &[Kind::Unknown, Kind::Unknown],
    );
    let statements = Factory::node(&factory, file)
        .as_source_file()
        .expect("source file")
        .statements()
        .expect("statements");
    let nodes: Vec<_> = factory
        .read_nodes(factory.read_list(statements).nodes())
        .iter()
        .flatten()
        .collect();
    assert_eq!(nodes.len(), 2);
    let declarations = Factory::node(&factory, nodes[0])
        .as_variable_statement()
        .expect("a hoisted var statement")
        .declaration_list()
        .expect("declarations");
    let list = Factory::node(&factory, declarations)
        .as_variable_declaration_list()
        .expect("a declaration list")
        .declarations()
        .expect("declarations");
    // `_a` for `Missing`, `_b` for `P.Q` and `_c` for the whole name.
    assert_eq!(factory.read_nodes(factory.read_list(list).nodes()).len(), 3);
}

#[test]
fn es_decorators_record_no_metadata() {
    let printed = transform(
        "class C {\n    @dec a: string;\n}\n",
        ScriptTarget::ES2015,
        false,
        &[],
    );
    assert!(metadata_lines(&printed).is_empty(), "{printed}");
}
