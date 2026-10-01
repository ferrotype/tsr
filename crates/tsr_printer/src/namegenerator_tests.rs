//! Port of the pinned `printer/namegenerator_test.go`: same inputs, same
//! expected names.

use super::{NameGenerator, NameGeneratorHost};
use crate::generated_identifier_flags as g;
use crate::{AutoGenerateOptions, EmitContext, Error};
use std::rc::Rc;
use tsr_arena::Counters;
use tsr_ast::{
    AstBuilder, AstView, BoundFile, Factory, FactoryMethods, JsString, NodeBinding, NodeId,
    SourceFileParseOptions, SymbolFlags, SymbolId, SymbolTableId, SymbolTableRead,
};
use tsr_core::{ScriptKind, TextRange};
use tsr_jsstring::SourceText;

/// `ec.Factory` with its emit context, plus the parsed and bound file when a
/// test parses one.
struct Fixture {
    ec: EmitContext,
    factory: AstBuilder,
    file: Option<BoundFile>,
}

impl Fixture {
    /// `printer.NewEmitContext()`.
    fn new() -> Self {
        let ec = EmitContext::new();
        let factory =
            AstBuilder::with_hooks(SourceText::default(), &Counters::new(), ec.factory_hooks());
        Self {
            ec,
            factory,
            file: None,
        }
    }

    /// `parsetestutil.ParseTypeScript(text, false)` followed by
    /// `binder.BindSourceFile(file)`; the factory retains the bound file.
    fn parse(text: &str) -> Self {
        let mut fixture = Self::new();
        let parsed = tsr_parser::parse_source_file(
            SourceText::from_loaded_bytes(text.as_bytes()),
            ScriptKind::TS,
            SourceFileParseOptions {
                file_name: js("/main.ts"),
                path: js("/main.ts"),
                ..Default::default()
            },
        );
        let root = parsed.root();
        let file = parsed.publish_unbound();
        let bound = tsr_binder::bind_source_file(&file, root).expect("bind");
        fixture.factory.retain_file(file);
        fixture.file = Some(bound);
        fixture
    }

    fn view(&self) -> AstView<'_> {
        self.factory.view()
    }

    /// `file.Statements.Nodes[index]`.
    fn statement(&self, index: usize) -> NodeId {
        let file = self.file.as_ref().expect("parsed file");
        let source = self.view().node(file.source()).expect("source file");
        nth(
            self.view(),
            source.statements(self.view()).expect("statements"),
            index,
        )
    }

    /// `node.Body().Statements()[index]`.
    fn body_statement(&self, node: NodeId, index: usize) -> NodeId {
        let view = self.view();
        let body = view.node(node).expect("node").body().expect("body");
        let body = view.node(body).expect("body");
        nth(view, body.statements(view).expect("statements"), index)
    }

    /// `node.Members()[index]`.
    fn member(&self, node: NodeId, index: usize) -> NodeId {
        let view = self.view();
        let node = view.node(node).expect("node");
        nth(view, node.members(view).expect("members"), index)
    }

    fn name_of(&self, node: NodeId) -> NodeId {
        self.view().node(node).expect("node").name().expect("name")
    }

    fn expression_of(&self, node: NodeId) -> NodeId {
        self.view()
            .node(node)
            .expect("node")
            .expression()
            .expect("expression")
    }

    fn host(&self) -> Host<'_> {
        Host {
            factory: &self.factory,
            file: self.file.as_ref(),
        }
    }

    /// `&printer.NameGenerator{Context: ec}`.
    fn generator(&self) -> NameGenerator<'_> {
        NameGenerator::new(Some(self.ec.clone()))
    }

    /// `&printer.NameGenerator{Context: ec, GetTextOfNode: (*ast.Node).Text}`.
    fn generator_with_text(&self) -> NameGenerator<'_> {
        let mut generator = self.generator();
        let view = self.view();
        generator.get_text_of_node = Some(Rc::new(move |node| {
            Ok(JsString::from_bytes(view.node_text(node)?.to_vec()))
        }));
        generator
    }
}

fn nth(view: AstView<'_>, nodes: tsr_ast::NodeSlice, index: usize) -> NodeId {
    view.node_slice(nodes)
        .expect("node slice")
        .get(index)
        .expect("index in range")
        .expect("present node")
}

fn js(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}

fn affixes(prefix: &str, suffix: &str) -> AutoGenerateOptions {
    AutoGenerateOptions {
        prefix: js(prefix),
        suffix: js(suffix),
        ..AutoGenerateOptions::default()
    }
}

struct Host<'a> {
    factory: &'a AstBuilder,
    file: Option<&'a BoundFile>,
}

impl NameGeneratorHost for Host<'_> {
    fn factory(&self) -> &dyn Factory {
        self.factory
    }
    fn view(&self) -> AstView<'_> {
        self.factory.view()
    }
    fn binding(&self, node: NodeId) -> Result<Option<NodeBinding>, tsr_arena::Error> {
        match self.file {
            Some(file) => file.view().node_binding(node),
            None => Ok(None),
        }
    }
    fn table(&self, table: SymbolTableId) -> Result<SymbolTableRead<'_>, tsr_arena::Error> {
        self.file
            .ok_or(tsr_arena::Error::InvalidGraph)?
            .view()
            .result()
            .tables()
            .get(table)
    }
    fn symbol_flags(&self, symbol: SymbolId) -> Result<SymbolFlags, tsr_arena::Error> {
        Ok(self
            .file
            .ok_or(tsr_arena::Error::InvalidGraph)?
            .view()
            .symbol(symbol)?
            .flags())
    }
}

fn generate(generator: &mut NameGenerator<'_>, host: &Host<'_>, name: NodeId) -> Vec<u8> {
    let text: Result<JsString, Error> = generator.generate_name(host, name);
    text.expect("generated name").as_bytes().to_vec()
}

#[test]
fn temp_variable1() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_temp_variable(&mut f.factory);
    let name2 = f.ec.new_temp_variable(&mut f.factory);

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    let text2 = generate(&mut g, &host, name2);

    assert_eq!(text1, b"_a");
    assert_eq!(text2, b"_b");
}

#[test]
fn temp_variable2() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_temp_variable_ex(&mut f.factory, affixes("A", "B"));
    let name2 = f.ec.new_temp_variable_ex(&mut f.factory, affixes("A", "B"));

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    let text2 = generate(&mut g, &host, name2);

    assert_eq!(text1, b"A_aB");
    assert_eq!(text2, b"A_bB");
}

#[test]
fn temp_variable3() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_temp_variable(&mut f.factory);

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    let text2 = generate(&mut g, &host, name1);

    assert_eq!(text1, b"_a");
    assert_eq!(text2, b"_a");
}

#[test]
fn temp_variable_scoped() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_temp_variable(&mut f.factory);
    let name2 = f.ec.new_temp_variable(&mut f.factory);

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    g.push_scope(false);
    let text2 = generate(&mut g, &host, name2);
    g.pop_scope(false);

    assert_eq!(text1, b"_a");
    assert_eq!(text2, b"_a");
}

#[test]
fn temp_variable_scoped_reserved() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_temp_variable_ex(
        &mut f.factory,
        AutoGenerateOptions {
            flags: g::RESERVED_IN_NESTED_SCOPES,
            ..AutoGenerateOptions::default()
        },
    );
    let name2 = f.ec.new_temp_variable(&mut f.factory);

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    g.push_scope(false);
    let text2 = generate(&mut g, &host, name2);
    g.pop_scope(false);

    assert_eq!(text1, b"_a");
    assert_eq!(text2, b"_b");
}

#[test]
fn loop_variable1() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_loop_variable(&mut f.factory);
    let name2 = f.ec.new_loop_variable(&mut f.factory);

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    let text2 = generate(&mut g, &host, name2);

    assert_eq!(text1, b"_i");
    assert_eq!(text2, b"_a");
}

#[test]
fn loop_variable2() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_loop_variable_ex(&mut f.factory, affixes("A", "B"));
    let name2 = f.ec.new_loop_variable_ex(&mut f.factory, affixes("A", "B"));

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    let text2 = generate(&mut g, &host, name2);

    assert_eq!(text1, b"A_iB");
    assert_eq!(text2, b"A_aB");
}

#[test]
fn loop_variable3() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_loop_variable(&mut f.factory);

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    let text2 = generate(&mut g, &host, name1);

    assert_eq!(text1, b"_i");
    assert_eq!(text2, b"_i");
}

#[test]
fn loop_variable_scoped() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_loop_variable(&mut f.factory);
    let name2 = f.ec.new_loop_variable(&mut f.factory);

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    g.push_scope(false);
    let text2 = generate(&mut g, &host, name2);
    g.pop_scope(false);

    assert_eq!(text1, b"_i");
    assert_eq!(text2, b"_i");
}

#[test]
fn unique_name1() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_unique_name(&mut f.factory, js("foo"));
    let name2 = f.ec.new_unique_name(&mut f.factory, js("foo"));

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    let text2 = generate(&mut g, &host, name2);

    assert_eq!(text1, b"foo_1");
    assert_eq!(text2, b"foo_2");
}

#[test]
fn unique_name2() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_unique_name(&mut f.factory, js("foo"));

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    let text2 = generate(&mut g, &host, name1);

    assert_eq!(text1, b"foo_1");
    // Expected to be same because GenerateName goes off object identity
    assert_eq!(text2, b"foo_1");
}

#[test]
fn unique_name_scoped() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_unique_name(&mut f.factory, js("foo"));
    let name2 = f.ec.new_unique_name(&mut f.factory, js("foo"));

    let mut g = f.generator();
    let host = f.host();
    assert_eq!(generate(&mut g, &host, name1), b"foo_1");

    g.push_scope(false);
    assert_eq!(generate(&mut g, &host, name2), b"foo_2"); // Matches Strada, but is incorrect
    g.pop_scope(false);
}

#[test]
fn unique_private_name1() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_unique_private_name(&mut f.factory, js("#foo"));
    let name2 = f.ec.new_unique_private_name(&mut f.factory, js("#foo"));

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    let text2 = generate(&mut g, &host, name2);

    assert_eq!(text1, b"#foo_1");
    assert_eq!(text2, b"#foo_2");
}

#[test]
fn unique_private_name2() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_unique_private_name(&mut f.factory, js("#foo"));

    let mut g = f.generator();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    let text2 = generate(&mut g, &host, name1);

    assert_eq!(text1, b"#foo_1");
    assert_eq!(text2, b"#foo_1");
}

#[test]
fn unique_private_name_scoped() {
    let mut f = Fixture::new();
    let name1 = f.ec.new_unique_private_name(&mut f.factory, js("#foo"));
    let name2 = f.ec.new_unique_private_name(&mut f.factory, js("#foo"));

    let mut g = f.generator();
    let host = f.host();
    assert_eq!(generate(&mut g, &host, name1), b"#foo_1");

    g.push_scope(false); // private names are always reserved in nested scopes
    assert_eq!(generate(&mut g, &host, name2), b"#foo_2");
    g.pop_scope(false);
}

#[test]
fn generated_name_for_identifier1() {
    let mut f = Fixture::parse("function f() {}");
    let n = f.name_of(f.statement(0));
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"f_1");
}

#[test]
fn generated_name_for_identifier2() {
    let mut f = Fixture::parse("function f() {}");
    let n = f.name_of(f.statement(0));
    let name1 =
        f.ec.new_generated_name_for_node_ex(&mut f.factory, n, affixes("a", "b"));

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"afb");
}

#[test]
fn generated_name_for_identifier3() {
    let mut f = Fixture::parse("function f() {}");
    let n = f.name_of(f.statement(0));
    let name1 =
        f.ec.new_generated_name_for_node_ex(&mut f.factory, n, affixes("a", "b"));
    let name2 = f.ec.new_generated_name_for_node(&mut f.factory, name1);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name2);

    assert_eq!(text1, b"afb_1");
}

/// namespace reuses name if it does not collide with locals
#[test]
fn generated_name_for_namespace1() {
    let mut f = Fixture::parse("namespace foo { }");
    let ns1 = f.statement(0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, ns1);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"foo");
}

/// namespace uses generated name if it collides with locals
#[test]
fn generated_name_for_namespace2() {
    let mut f = Fixture::parse("namespace foo { var foo; }");
    let ns1 = f.statement(0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, ns1);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"foo_1");
}

/// avoids collisions when unscoped
#[test]
fn generated_name_for_namespace3() {
    let mut f = Fixture::parse(
        "namespace ns1 { namespace foo { var foo; } } namespace ns2 { namespace foo { var foo; } }",
    );
    let ns1 = f.body_statement(f.statement(0), 0);
    let ns2 = f.body_statement(f.statement(1), 0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, ns1);
    let name2 = f.ec.new_generated_name_for_node(&mut f.factory, ns2);

    let mut g = f.generator_with_text();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    let text2 = generate(&mut g, &host, name2);

    assert_eq!(text1, b"foo_1");
    assert_eq!(text2, b"foo_2");
}

/// reuse name when scoped
#[test]
fn generated_name_for_namespace4() {
    let mut f = Fixture::parse(
        "namespace ns1 { namespace foo { var foo; } } namespace ns2 { namespace foo { var foo; } }",
    );
    let ns1 = f.body_statement(f.statement(0), 0);
    let ns2 = f.body_statement(f.statement(1), 0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, ns1);
    let name2 = f.ec.new_generated_name_for_node(&mut f.factory, ns2);

    let mut g = f.generator_with_text();
    let host = f.host();
    g.push_scope(false);
    let text1 = generate(&mut g, &host, name1);
    g.pop_scope(false);

    g.push_scope(false);
    let text2 = generate(&mut g, &host, name2);
    g.pop_scope(false);

    assert_eq!(text1, b"foo_1");
    assert_eq!(text2, b"foo_2"); // Matches Strada, but is incorrect
}

#[test]
fn generated_name_for_node_cached() {
    let mut f = Fixture::parse("namespace foo { var foo; }");
    let ns1 = f.statement(0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, ns1);
    let name2 = f.ec.new_generated_name_for_node(&mut f.factory, ns1);

    let mut g = f.generator_with_text();
    let host = f.host();
    let text1 = generate(&mut g, &host, name1);
    let text2 = generate(&mut g, &host, name2);

    assert_eq!(text1, b"foo_1");
    assert_eq!(text2, b"foo_1");
}

#[test]
fn generated_name_for_import() {
    let mut f = Fixture::parse("import * as foo from 'foo'");
    let n = f.statement(0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"foo_1");
}

#[test]
fn generated_name_for_export() {
    let mut f = Fixture::parse("export * as foo from 'foo'");
    let n = f.statement(0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"foo_1");
}

#[test]
fn generated_name_for_function_declaration1() {
    let mut f = Fixture::parse("export function f() {}");
    let n = f.statement(0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"f_1");
}

#[test]
fn generated_name_for_function_declaration2() {
    let mut f = Fixture::parse("export default function () {}");
    let n = f.statement(0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"default_1");
}

#[test]
fn generated_name_for_class_declaration1() {
    let mut f = Fixture::parse("export class C {}");
    let n = f.statement(0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"C_1");
}

#[test]
fn generated_name_for_class_declaration2() {
    let mut f = Fixture::parse("export default class {}");
    let n = f.statement(0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"default_1");
}

#[test]
fn generated_name_for_export_assignment() {
    let mut f = Fixture::parse("export default 0");
    let n = f.statement(0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"default_1");
}

#[test]
fn generated_name_for_class_expression() {
    let mut f = Fixture::parse("(class {})");
    let n = f.expression_of(f.expression_of(f.statement(0)));
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"class_1");
}

#[test]
fn generated_name_for_method1() {
    let mut f = Fixture::parse("class C { m() {} }");
    let n = f.member(f.statement(0), 0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"m_1");
}

#[test]
fn generated_name_for_method2() {
    let mut f = Fixture::parse("class C { 0() {} }");
    let n = f.member(f.statement(0), 0);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"_a");
}

#[test]
fn generated_private_name_for_method() {
    let mut f = Fixture::parse("class C { m() {} }");
    let n = f.member(f.statement(0), 0);
    let name1 = f.ec.new_generated_private_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"#m_1");
}

#[test]
fn generated_name_for_computed_property_name() {
    let mut f = Fixture::parse("class C { [x] }");
    let n = f.name_of(f.member(f.statement(0), 0));
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"_a");
}

#[test]
fn generated_name_for_other() {
    let mut f = Fixture::parse("class C { [x] }");
    let slice = f.factory.node_slice(Vec::new()).expect("node slice");
    let list = f
        .factory
        .new_list(TextRange::new(-1, -1), slice)
        .expect("node list");
    let n = f
        .factory
        .new_object_literal_expression(Some(list), false /*multiLine*/);
    let name1 = f.ec.new_generated_name_for_node(&mut f.factory, n);

    let mut g = f.generator_with_text();
    let text1 = generate(&mut g, &f.host(), name1);

    assert_eq!(text1, b"_a");
}
