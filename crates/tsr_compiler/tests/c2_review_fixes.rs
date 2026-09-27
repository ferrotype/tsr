//! Regressions for the C2 review findings on symbol ids and two audited
//! functions. Each test names its pinned Go counterpart; the ids are the
//! lazily assigned `ast.GetSymbolId` values that unique-symbol property names
//! and the declarationless symbol comparison expose.
#[path = "support/c3_native_diagnostics.rs"]
mod native;
#[path = "support/c2_contract_program.rs"]
mod support;

use std::ops::ControlFlow;
use tsr_arena::NodeId;
use tsr_ast::{AstView, ChildVisitor, SyntaxKind as K};
use tsr_compiler::Program;

/// The first node of `kind` in `file`, or the first whose name reads `name`.
fn first(program: &Program, file: &str, kind: K, name: Option<&str>) -> NodeId {
    struct Walk<'a> {
        view: AstView<'a>,
        kind: K,
        name: Option<&'a str>,
        found: Option<NodeId>,
    }
    impl ChildVisitor for Walk<'_> {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            let read = self.view.node(node).unwrap();
            if read.kind() == self.kind {
                let matches = match self.name {
                    None => true,
                    Some(name) => read.name().is_some_and(|n| {
                        self.view.node_text(n).unwrap().as_bytes() == name.as_bytes()
                    }),
                };
                if matches {
                    self.found = Some(node);
                    return ControlFlow::Break(());
                }
            }
            read.for_each_child(self)
        }
        fn visit_list(&mut self, list: tsr_ast::NodeListId) -> ControlFlow<()> {
            self.visit_node_slice(self.view.list(list).unwrap().nodes())
        }
        fn visit_node_slice(&mut self, slice: tsr_ast::NodeSlice) -> ControlFlow<()> {
            for node in self.view.node_slice(slice).unwrap().iter().flatten() {
                self.visit_node(node)?;
            }
            ControlFlow::Continue(())
        }
    }
    let file = program.file(file.as_bytes()).unwrap();
    let mut walk = Walk {
        view: file.bound().view().ast(),
        kind,
        name,
        found: None,
    };
    let _ = walk.visit_node(file.source());
    walk.found.expect("node not found")
}

fn name_node(program: &Program, file: &str, node: NodeId) -> NodeId {
    let file = program.file(file.as_bytes()).unwrap();
    file.bound()
        .view()
        .ast()
        .node(node)
        .unwrap()
        .name()
        .unwrap()
}

/// `getLiteralTypeFromProperty(prop, include, includeNonPublic)`:
/// `checkIndexConstraints` passes true, so a private property keeps its
/// literal name, which is not applicable to a number index signature. The
/// pinned tsgo reports only the public property of `D`.
#[test]
fn a_private_property_is_not_checked_against_the_number_index_signature() {
    native::assert_fixture(
        "private_index_signature.ts",
        include_str!("fixtures/c2_review/private_index_signature.ts"),
        include_str!("fixtures/c2_review/private_index_signature.ts.native.json"),
    );
}

/// `instantiateSymbol`: a setter whose resolved and write types cannot contain
/// type variables is returned as itself, like any other such symbol.
#[test]
fn a_non_generic_setter_is_not_copied_by_instantiation() {
    let source = r#"class C<T> {
  m!: T;
  set s(v: string) {}
  constructor() { this.s = "a"; }
}
declare const x: C<number>;
x.s = "b";
"#;
    let program = support::program(&[("/main.ts", source)]);
    let (_counters, _generation, owner) = support::checker(&program);
    let mut op = owner.operation().unwrap();
    let file = program.file(b"/main.ts").unwrap();
    // Source order resolves the setter's write type in the constructor before
    // `C<number>` instantiates its members for `x.s`.
    assert_eq!(
        support::codes(&op.semantic_diagnostics(file.source()).unwrap()),
        Vec::<i32>::new()
    );
    let setter = first(&program, "/main.ts", K::SetAccessor, Some("s"));
    let setter_symbol = op
        .get_symbol_at_location(name_node(&program, "/main.ts", setter))
        .unwrap()
        .unwrap();
    let field = first(&program, "/main.ts", K::PropertyDeclaration, Some("m"));
    let field_symbol = op
        .get_symbol_at_location(name_node(&program, "/main.ts", field))
        .unwrap()
        .unwrap();
    let x = op
        .get_type_at_location(support::name_of(&program, "/main.ts", "x"))
        .unwrap();
    let properties = op.properties_of_type(x).unwrap();
    assert_eq!(properties.len(), 2);
    let ids: Vec<_> = properties.iter().map(|p| p.id()).collect();
    // The field depends on T and is instantiated; the setter is not copied.
    assert!(
        !ids.contains(&field_symbol.id()),
        "the generic field must be instantiated"
    );
    assert!(
        ids.contains(&setter_symbol.id()),
        "the setter symbol must be reused"
    );
}

/// `checkFunctionOrConstructorSymbol` keeps its once-only flag on the value
/// symbol links, whose access assigns the function symbol's id.
#[test]
fn checking_a_function_declaration_assigns_its_symbol_id() {
    let program = support::program(&[("/main.ts", "declare function f(): void;\n")]);
    let (_counters, _generation, owner) = support::checker(&program);
    let mut op = owner.operation().unwrap();
    let file = program.file(b"/main.ts").unwrap();
    let declaration = first(&program, "/main.ts", K::FunctionDeclaration, Some("f"));
    let symbol = op
        .get_symbol_at_location(name_node(&program, "/main.ts", declaration))
        .unwrap()
        .unwrap();
    assert_eq!(
        op.existing_symbol_runtime_id(symbol).unwrap(),
        0,
        "no id before checking"
    );
    op.semantic_diagnostics(file.source()).unwrap();
    assert_ne!(
        op.existing_symbol_runtime_id(symbol).unwrap(),
        0,
        "checked function symbols carry an id"
    );
}

/// `padObjectLiteralType` observes each padded property's link (assigning
/// its id) before it computes the element type. Computing `a`'s type creates
/// the nested pattern's contextual symbols, so `b`'s id is not the next one.
#[test]
fn a_padded_property_takes_its_id_before_its_element_type() {
    let source = "function f({ a: { x = 1 } = {}, b = 1 } = {}) {}\n";
    let program = support::program(&[("/main.ts", source)]);
    let (_counters, _generation, owner) = support::checker(&program);
    let mut op = owner.operation().unwrap();
    let pattern = first(&program, "/main.ts", K::ObjectBindingPattern, None);
    let padded = op.get_type_at_location(pattern).unwrap();
    let properties = op.properties_of_type(padded).unwrap();
    assert_eq!(properties.len(), 2);
    let a = op.existing_symbol_runtime_id(properties[0]).unwrap();
    let b = op.existing_symbol_runtime_id(properties[1]).unwrap();
    assert!(a != 0 && b != 0, "padded properties carry ids: {a} {b}");
    assert!(
        b > a + 1,
        "b's id ({b}) must follow the ids assigned while typing a ({a})"
    );
}

/// `getNameOfSymbolAsWritten` consults `remappedSymbolReferences` by symbol
/// id, so writing a type's name assigns the id of a symbol nothing else
/// observed.
#[test]
fn writing_a_symbol_name_assigns_its_id() {
    let program = support::program(&[("/main.ts", "interface I {}\ndeclare const v: I;\n")]);
    let (_counters, _generation, owner) = support::checker(&program);
    let mut op = owner.operation().unwrap();
    let interface = first(&program, "/main.ts", K::InterfaceDeclaration, Some("I"));
    let symbol = op
        .get_symbol_at_location(name_node(&program, "/main.ts", interface))
        .unwrap()
        .unwrap();
    let v = op
        .get_type_at_location(support::name_of(&program, "/main.ts", "v"))
        .unwrap();
    assert_eq!(
        op.existing_symbol_runtime_id(symbol).unwrap(),
        0,
        "no id before the display"
    );
    assert_eq!(op.type_to_string(v, 0).unwrap().as_bytes(), b"I");
    assert_ne!(
        op.existing_symbol_runtime_id(symbol).unwrap(),
        0,
        "the written name assigned the id"
    );
}
