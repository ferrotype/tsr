//! Direct pinned-Go observations. Symbol graphs are test fixtures, not a binder.
use super::*;
use crate::reference_resolver::{
    NoReferenceResolverHooks, ReferenceResolver, ReferenceResolverHooks,
};
use ts_arena::{Counters, SymbolArena};
use ts_ast::{AstFile, BindBuilder, BindResult, SourceFileParseOptions, Symbol};
use ts_core::ScriptKind;
use ts_jsstring::SourceText;

struct Host {
    file: AstFile,
    source: NodeId,
    transient: SymbolArena<Symbol>,
}
impl Host {
    fn fixture<T>(
        text: &str,
        js: bool,
        setup: impl FnOnce(&mut BindBuilder<'_>) -> T,
    ) -> (Self, T) {
        let counters = Counters::new();
        let parsed = ts_parser::parse_source_file_with_counters(
            SourceText::from_loaded_bytes(text.as_bytes()),
            if js { ScriptKind::JS } else { ScriptKind::TS },
            SourceFileParseOptions {
                file_name: JsString::from_bytes(if js {
                    b"/resolver.js".as_slice()
                } else {
                    b"/resolver.ts".as_slice()
                }),
                ..Default::default()
            },
            &counters,
        );
        let source = parsed.root();
        let file = parsed.publish_unbound();
        let mut value = None;
        file.bind_with(source, |builder| {
            value = Some(setup(builder));
            Ok(())
        })
        .unwrap();
        (
            Self {
                file,
                source,
                transient: SymbolArena::new(&counters),
            },
            value.unwrap(),
        )
    }
    fn empty(text: &str, js: bool) -> Self {
        Self::fixture(text, js, |_| ()).0
    }
    fn result(&self) -> &BindResult {
        self.file.bound_view(self.source).unwrap().unwrap().result()
    }
    fn find(&self, kind: K, nth: usize) -> NodeId {
        find(self.ast(self.source).unwrap(), self.source, kind, nth)
    }
}
impl ResolverHost for Host {
    fn ast(&self, node: NodeId) -> Result<AstView<'_>, Error> {
        let view = self.file.bound_view(self.source).unwrap().unwrap().ast();
        view.node(node)?;
        Ok(view)
    }
    fn binding(&self, node: NodeId) -> Result<Option<NodeBinding>, Error> {
        self.ast(node)?;
        Ok(self.result().node_binding(self.ast(node)?, node))
    }
    fn symbol(&self, id: SymbolId) -> Result<SymbolRef<'_>, Error> {
        if id.arena() == self.transient.id() {
            self.transient.get(id).map(SymbolRef::Owned)
        } else {
            self.result().symbols().get(id).map(SymbolRef::Stored)
        }
    }
    fn table(&self, id: SymbolTableId) -> Result<SymbolTableRead<'_>, Error> {
        self.result().tables().get(id)
    }
    fn declarations(&self, id: SymbolId) -> Result<DeclarationRead<'_>, Error> {
        self.result()
            .declarations()
            .get(self.symbol(id)?.declarations())
    }
    fn new_transient_symbol(
        &mut self,
        flags: SymbolFlags,
        name: JsString,
    ) -> Result<SymbolId, Error> {
        Ok(self.transient.push(Symbol::new(flags, name)))
    }
}
struct Find<'a> {
    view: AstView<'a>,
    kind: K,
    nth: usize,
    found: Option<NodeId>,
}
impl ChildVisitor for Find<'_> {
    fn visit_node(&mut self, id: NodeId) -> ControlFlow<()> {
        let node = self.view.node(id).unwrap();
        if node.kind() == self.kind {
            if self.nth == 0 {
                self.found = Some(id);
                return ControlFlow::Break(());
            }
            self.nth -= 1;
        }
        node.for_each_child(self)
    }
    fn visit_list(&mut self, id: NodeListId) -> ControlFlow<()> {
        self.visit_node_slice(self.view.list(id).unwrap().nodes())
    }
    fn visit_node_slice(&mut self, slice: NodeSlice) -> ControlFlow<()> {
        for id in self.view.node_slice(slice).unwrap().iter().flatten() {
            self.visit_node(id)?;
        }
        ControlFlow::Continue(())
    }
}
fn find(view: AstView<'_>, root: NodeId, kind: K, nth: usize) -> NodeId {
    let mut visitor = Find {
        view,
        kind,
        nth,
        found: None,
    };
    let _ = visitor.visit_node(root);
    visitor.found.expect("fixture node exists")
}
fn symbol(
    builder: &mut BindBuilder<'_>,
    name: &str,
    flags: SymbolFlags,
    declarations: &[NodeId],
) -> SymbolId {
    let mut s = Symbol::new(flags, JsString::from_bytes(name.as_bytes()));
    s.declarations = builder
        .declarations_mut()
        .alloc(declarations.iter().copied().map(Some).collect())
        .unwrap();
    s.value_declaration = declarations.first().copied();
    builder.symbols_mut().push(s)
}
fn options() -> ResolverOptions {
    ResolverOptions {
        emit_script_target: ScriptTarget::ES2022,
        isolated_modules: false,
        verbatim_module_syntax: false,
        emit_standard_class_fields: true,
    }
}
fn observed(label: &str, values: &[String]) {
    let row = format!("{label}\t{}", values.join("\t"));
    let expected = include_str!("../../../data/s07/resolver-observations.tsv")
        .lines()
        .find(|line| line.split('\t').next() == Some(label))
        .expect("frozen Go observation");
    assert_eq!(row, expected);
}
macro_rules! obs{($label:expr,$($value:expr),+ $(,)?)=>{observed($label,&[$(format!("{}",$value)),+])};}

#[test]
fn pinned_scope_changes_and_deferred_contexts() {
    for (i, text) in [
        "function f(p=x?.y) {}",
        "function f(p=x??y) {}",
        "function f({...p}) {}",
        "function f(p=class {static x=1}) {}",
        "function f(p=()=>x?.y) {}",
        "function f(p=class {x=x?.y}) {}",
        "function f(p={ [x?.y]: 0}) {}",
        "function f(p={ m() {x?.y} }) {}",
        "function f(p: typeof x) {}",
    ]
    .iter()
    .enumerate()
    {
        let host = Host::empty(text, false);
        let parameter = host.find(K::Parameter, 0);
        for target in [
            ScriptTarget::ES2016,
            ScriptTarget::ES2017,
            ScriptTarget::ES2020,
            ScriptTarget::ES2022,
        ] {
            let resolver = NameResolver::new(ResolverOptions {
                emit_script_target: target,
                emit_standard_class_fields: target >= ScriptTarget::ES2022,
                ..options()
            });
            obs!(
                &format!("scope/{i}/{}", target.0),
                resolver.requires_scope_change(&host, parameter).unwrap()
            );
        }
    }
    for (i, (text, kind)) in [
        ("const v=()=>x;", K::ArrowFunction),
        ("const v=(()=>x)();", K::ArrowFunction),
        ("const v=(async()=>x)();", K::ArrowFunction),
        ("const v=(function*(){yield x})();", K::FunctionExpression),
        ("function f(){x}", K::FunctionDeclaration),
        ("class C {p=x}", K::PropertyDeclaration),
        ("class C {static p=x}", K::PropertyDeclaration),
        ("type T=typeof x;", K::TypeQuery),
    ]
    .iter()
    .enumerate()
    {
        let host = Host::empty(text, false);
        let node = host.find(*kind, 0);
        obs!(
            &format!("deferred/{i}"),
            get_is_deferred_context(&host, node, None).unwrap(),
            get_is_deferred_context(&host, node, host.node(node).unwrap().name()).unwrap()
        );
    }
}
struct NameTrace {
    mode: usize,
    symbol: Option<SymbolId>,
    trace: Vec<String>,
    failed: bool,
    codes: Vec<i32>,
    state: Tristate,
}
impl Default for NameTrace {
    fn default() -> Self {
        Self {
            mode: 0,
            symbol: None,
            trace: Vec::new(),
            failed: false,
            codes: Vec::new(),
            state: Tristate::UNKNOWN,
        }
    }
}
impl NameResolverHooks for NameTrace {
    fn lookup(
        &mut self,
        table: Option<SymbolTableId>,
        _name: &[u8],
        meaning: SymbolFlags,
    ) -> Result<Hook<Option<SymbolId>>, Error> {
        if self.mode == 0 {
            return Ok(Hook::Absent);
        }
        self.trace
            .push(format!("lookup:{meaning}:{}", table.is_none()));
        Ok(Hook::Value(if self.mode == 1 { None } else { self.symbol }))
    }
    fn symbol_referenced(&mut self, _symbol: SymbolId, _meaning: SymbolFlags) -> Result<(), Error> {
        self.trace.push("referenced".into());
        Ok(())
    }
    fn on_failed_to_resolve_symbol(
        &mut self,
        _location: Option<NodeId>,
        _name: &[u8],
        _meaning: SymbolFlags,
        _message: &'static Message,
    ) -> Result<(), Error> {
        self.failed = true;
        self.trace.push("failed".into());
        Ok(())
    }
    fn on_successfully_resolved_symbol(&mut self, _resolution: ResolvedName) -> Result<(), Error> {
        self.trace.push("success".into());
        Ok(())
    }
    fn error(
        &mut self,
        _location: Option<NodeId>,
        message: &'static Message,
        _args: &[JsString],
    ) -> Result<(), Error> {
        self.codes.push(message.code);
        Ok(())
    }
    fn get_requires_scope_change_cache(&mut self, _node: NodeId) -> Result<Tristate, Error> {
        self.trace.push("get".into());
        Ok(self.state)
    }
    fn set_requires_scope_change_cache(
        &mut self,
        _node: NodeId,
        value: Tristate,
    ) -> Result<(), Error> {
        self.trace.push(format!("set:{}", value.0));
        Ok(())
    }
}
#[test]
fn pinned_globals_arguments_require_and_const() {
    for mode in 0..4 {
        let (mut host, (s, globals)) = Host::fixture("", false, |b| {
            let s = symbol(b, "g", flags::FUNCTION_SCOPED_VARIABLE, &[]);
            let globals = b.tables_mut().alloc(
                [(JsString::from_bytes(b"g".as_slice()), Some(s))]
                    .into_iter()
                    .collect(),
            );
            (s, globals)
        });
        let mut resolver = NameResolver::new(options());
        resolver.globals = (mode != 3).then_some(globals);
        let mut hooks = NameTrace {
            mode,
            symbol: Some(s),
            ..Default::default()
        };
        let got = resolver
            .resolve(
                &mut host,
                &mut hooks,
                None,
                b"g",
                0,
                Some(diagnostics::Cannot_find_name_0),
                true,
                false,
            )
            .unwrap();
        obs!(
            &format!("global/{mode}"),
            got.is_some(),
            hooks.trace.join(",")
        );
    }
    let mut host = Host::empty("function f(){arguments;}", false);
    let function = host.find(K::FunctionDeclaration, 0);
    let mut resolver = NameResolver::new(options());
    let a = resolver
        .resolve(
            &mut host,
            &mut NoNameResolverHooks,
            Some(function),
            b"arguments",
            flags::VARIABLE,
            None,
            false,
            true,
        )
        .unwrap()
        .unwrap();
    let b = resolver
        .resolve(
            &mut host,
            &mut NoNameResolverHooks,
            Some(function),
            b"arguments",
            flags::VARIABLE,
            None,
            false,
            true,
        )
        .unwrap()
        .unwrap();
    obs!(
        "arguments",
        a == b,
        host.symbol(a).unwrap().flags(),
        String::from_utf8_lossy(host.symbol(a).unwrap().name_bytes())
    );
    assert_eq!(host.transient.len(), 1);
    let mut host = Host::empty("require(dynamic)", true);
    let reference = host.find(K::Identifier, 0);
    let mut resolver = NameResolver::new(options());
    let mut hooks = NameTrace::default();
    let got = resolver
        .resolve(
            &mut host,
            &mut hooks,
            Some(reference),
            b"require",
            flags::VALUE,
            Some(diagnostics::Cannot_find_name_0),
            true,
            true,
        )
        .unwrap();
    obs!("require/nil", got.is_none(), hooks.failed);
    resolver.require_symbol = Some(
        host.new_transient_symbol(flags::FUNCTION, JsString::from_bytes(b"require".as_slice()))
            .unwrap(),
    );
    let got = resolver
        .resolve(
            &mut host,
            &mut hooks,
            Some(reference),
            b"require",
            flags::VALUE,
            Some(diagnostics::Cannot_find_name_0),
            true,
            true,
        )
        .unwrap();
    obs!(
        "require/symbol",
        got == resolver.require_symbol,
        hooks.failed
    );
    let mut host = Host::empty("0 as const", false);
    let reference = host.find(K::Identifier, 0);
    let mut resolver = NameResolver::new(options());
    let mut hooks = NameTrace::default();
    let got = resolver
        .resolve(
            &mut host,
            &mut hooks,
            Some(reference),
            b"const",
            flags::TYPE,
            Some(diagnostics::Cannot_find_name_0),
            false,
            false,
        )
        .unwrap();
    obs!("const", got.is_none(), hooks.failed);
}
#[test]
fn pinned_default_export_class_type_and_parameter_cache() {
    let (host, (local, exported)) =
        Host::fixture("export default function local() {}", false, |b| {
            let function = find(b.view(), b.source(), K::FunctionDeclaration, 0);
            let local = symbol(b, "local", flags::FUNCTION, &[function]);
            let exported = symbol(b, "default", flags::FUNCTION, &[function]);
            b.binding_mut(function).unwrap().local_symbol = Some(local);
            (local, exported)
        });
    obs!(
        "default",
        get_local_symbol_for_export_default(&host, Some(exported)).unwrap() == Some(local),
        get_local_symbol_for_export_default(&host, None)
            .unwrap()
            .is_none()
    );
    let (mut host, (class, tp)) = Host::fixture("class C<T> {static p:T}", false, |b| {
        let class = find(b.view(), b.source(), K::ClassDeclaration, 0);
        let parameter = find(b.view(), b.source(), K::TypeParameter, 0);
        let tp = symbol(b, "T", flags::TYPE_PARAMETER, &[parameter]);
        let cs = symbol(b, "C", flags::CLASS, &[class]);
        let table = b.tables_mut().alloc(
            [(JsString::from_bytes(b"T".as_slice()), Some(tp))]
                .into_iter()
                .collect(),
        );
        b.symbols_mut().get_mut(cs).unwrap().members = Some(table);
        b.binding_mut(class).unwrap().symbol = Some(cs);
        (class, tp)
    });
    let reference = host.find(K::TypeReference, 0);
    let mut resolver = NameResolver::new(options());
    let mut hooks = NameTrace::default();
    let got = resolver
        .resolve(
            &mut host,
            &mut hooks,
            Some(reference),
            b"T",
            flags::TYPE,
            Some(diagnostics::Cannot_find_name_0),
            false,
            true,
        )
        .unwrap();
    obs!(
        "static/type",
        got.is_none(),
        hooks
            .codes
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );
    obs!(
        "type/container",
        is_type_parameter_symbol_declared_in_container(&host, tp, class).unwrap(),
        is_type_parameter_symbol_declared_in_container(&host, tp, host.source).unwrap()
    );
    let (host, (function, parameter, vs)) = Host::fixture("function f(p=x){var x;}", false, |b| {
        let function = find(b.view(), b.source(), K::FunctionDeclaration, 0);
        let parameter = find(b.view(), b.source(), K::Parameter, 0);
        let value = find(b.view(), b.source(), K::VariableDeclaration, 0);
        (
            function,
            parameter,
            symbol(b, "x", flags::FUNCTION_SCOPED_VARIABLE, &[value]),
        )
    });
    for state in [Tristate::UNKNOWN, Tristate::TRUE, Tristate::FALSE] {
        let resolver = NameResolver::new(options());
        let mut hooks = NameTrace {
            state,
            ..Default::default()
        };
        let got = resolver
            .use_outer_variable_scope_in_parameter(&host, &mut hooks, vs, function, parameter)
            .unwrap();
        obs!(
            &format!("parameter/cache/{}", state.0),
            got,
            hooks.trace.join(",")
        );
    }
}
struct RefTrace {
    symbol: SymbolId,
    mode: usize,
    trace: Vec<String>,
}
impl ReferenceResolverHooks for RefTrace {
    fn get_resolved_symbol(&mut self, _node: NodeId) -> Result<Hook<Option<SymbolId>>, Error> {
        self.trace.push("resolved".into());
        Ok(Hook::Value((self.mode == 0).then_some(self.symbol)))
    }
    fn resolve_name(
        &mut self,
        _location: Option<NodeId>,
        _name: &[u8],
        _meaning: SymbolFlags,
        _message: Option<&'static Message>,
        _is_use: bool,
        _exclude_globals: bool,
    ) -> Result<Hook<Option<SymbolId>>, Error> {
        self.trace.push("resolve".into());
        Ok(Hook::Value((self.mode != 2).then_some(self.symbol)))
    }
    fn get_merged_symbol(&mut self, symbol: SymbolId) -> Result<Hook<Option<SymbolId>>, Error> {
        self.trace.push("merged".into());
        Ok(Hook::Value(Some(symbol)))
    }
}
#[test]
fn pinned_reference_alias_filtering_hook_order_and_names() {
    for (i, text) in [
        "import {A} from 'm'; A;",
        "import {type A} from 'm'; A;",
        "import type {A} from 'm'; A;",
        "import A = require('m'); A;",
        "import type A = require('m'); A;",
    ]
    .iter()
    .enumerate()
    {
        let (mut host, (declaration, symbol)) = Host::fixture(text, false, |b| {
            let declaration = find(
                b.view(),
                b.source(),
                if i < 3 {
                    K::ImportSpecifier
                } else {
                    K::ImportEqualsDeclaration
                },
                0,
            );
            (declaration, symbol(b, "A", flags::ALIAS, &[declaration]))
        });
        let reference = host.node(declaration).unwrap().name().unwrap();
        let mut resolver = ReferenceResolver::new(options());
        let mut hooks = RefTrace {
            symbol,
            mode: 0,
            trace: Vec::new(),
        };
        obs!(
            &format!("import/{i}"),
            resolver
                .get_referenced_import_declaration(&mut host, &mut hooks, reference)
                .unwrap()
                == Some(declaration)
        );
    }
    let (mut host, (reference, symbol)) =
        Host::fixture("let v=1; function f(){} type T=number;", false, |b| {
            let value = find(b.view(), b.source(), K::VariableDeclaration, 0);
            let function = find(b.view(), b.source(), K::FunctionDeclaration, 0);
            let alias = find(b.view(), b.source(), K::TypeAliasDeclaration, 0);
            let reference = b.node(value).unwrap().name().unwrap();
            (
                reference,
                symbol(
                    b,
                    "v",
                    flags::FUNCTION_SCOPED_VARIABLE,
                    &[value, alias, function],
                ),
            )
        });
    for mode in 0..4 {
        let mut resolver = ReferenceResolver::new(options());
        let mut hooks = RefTrace {
            symbol,
            mode,
            trace: Vec::new(),
        };
        let got = resolver
            .get_referenced_value_declarations(&mut host, &mut hooks, reference)
            .unwrap();
        obs!(
            &format!("reference/{mode}"),
            got.as_ref().map_or(0, Vec::len),
            got.is_none(),
            hooks.trace.join(",")
        );
    }
    struct Element(usize);
    impl ReferenceResolverHooks for Element {
        fn get_element_access_expression_name(
            &mut self,
            _expression: NodeId,
        ) -> Result<Hook<Option<JsString>>, Error> {
            Ok(match self.0 {
                0 => Hook::Absent,
                1 => Hook::Value(None),
                _ => Hook::Value(Some(JsString::from_bytes(b"member".as_slice()))),
            })
        }
    }
    let host = Host::empty("this[x]", false);
    let element = host.find(K::ElementAccessExpression, 0);
    for mode in 0..3 {
        let resolver = ReferenceResolver::new(options());
        let mut hooks = Element(mode);
        let got = resolver
            .get_element_access_expression_name(&mut hooks, Some(element))
            .unwrap();
        let nil = resolver
            .get_element_access_expression_name(&mut hooks, None)
            .unwrap();
        obs!(
            &format!("element/{mode}"),
            String::from_utf8_lossy(got.as_bytes()),
            String::from_utf8_lossy(nil.as_bytes())
        );
    }
}

#[test]
fn resolver_ids_are_checked_against_the_retained_host() {
    let mut first = Host::empty("function f(){}", false);
    let other = Host::empty("function g(){}", false);
    let foreign = other.find(K::FunctionDeclaration, 0);
    let mut resolver = NameResolver::new(options());
    assert!(matches!(
        resolver.resolve(
            &mut first,
            &mut NoNameResolverHooks,
            Some(foreign),
            b"arguments",
            flags::VARIABLE,
            None,
            false,
            false
        ),
        Err(Error::WrongOwner)
    ));
    let id = first
        .new_transient_symbol(flags::FUNCTION, JsString::default())
        .unwrap();
    assert!(matches!(other.symbol(id), Err(Error::WrongOwner)));
    assert!(ReferenceResolver::new(options())
        .get_element_access_expression_name(&mut NoReferenceResolverHooks, None)
        .unwrap()
        .is_empty());
}

#[test]
fn pinned_lexical_self_reference_and_deferred_callback_payloads() {
    struct Trace(Vec<String>);
    impl NameResolverHooks for Trace {
        fn symbol_referenced(&mut self, _: SymbolId, _: SymbolFlags) -> Result<(), Error> {
            self.0.push("referenced".into());
            Ok(())
        }
        fn on_successfully_resolved_symbol(&mut self, r: ResolvedName) -> Result<(), Error> {
            self.0.push(format!(
                "success:{}:{}",
                r.associated_declaration.is_some(),
                r.within_deferred_context
            ));
            Ok(())
        }
    }
    for (i, text) in [
        "export {}; function f(){f;}",
        "export {}; let x; function f(p=x){x;}",
    ]
    .iter()
    .enumerate()
    {
        let (mut host, symbol) = Host::fixture(text, false, |b| {
            let function = find(b.view(), b.source(), K::FunctionDeclaration, 0);
            let fs = symbol(b, "f", flags::FUNCTION, &[function]);
            b.binding_mut(function).unwrap().symbol = Some(fs);
            let (name, s) = if i == 0 {
                ("f", fs)
            } else {
                let declaration = find(b.view(), b.source(), K::VariableDeclaration, 0);
                (
                    "x",
                    symbol(b, "x", flags::BLOCK_SCOPED_VARIABLE, &[declaration]),
                )
            };
            let table = b.tables_mut().alloc(
                [(JsString::from_bytes(name.as_bytes()), Some(s))]
                    .into_iter()
                    .collect(),
            );
            let source = b.source();
            b.binding_mut(source).unwrap().locals = Some(table);
            s
        });
        let statement = host.find(K::ExpressionStatement, 0);
        let reference = host.node(statement).unwrap().expression().unwrap();
        let mut hooks = Trace(Vec::new());
        let mut resolver = NameResolver::new(options());
        let got = resolver
            .resolve(
                &mut host,
                &mut hooks,
                Some(reference),
                if i == 0 { b"f" } else { b"x" },
                flags::VALUE,
                Some(diagnostics::Cannot_find_name_0),
                true,
                true,
            )
            .unwrap();
        obs!(
            &format!("lexical/{i}"),
            got == Some(symbol),
            hooks.0.join(",")
        );
    }
}

#[test]
fn pinned_export_container_member_fallback_and_present_nil_hooks() {
    struct Hooks {
        symbol: SymbolId,
        mode: usize,
    }
    impl ReferenceResolverHooks for Hooks {
        fn get_resolved_symbol(&mut self, _: NodeId) -> Result<Hook<Option<SymbolId>>, Error> {
            Ok(Hook::Value(Some(self.symbol)))
        }
        fn get_parent_of_symbol(&mut self, _: SymbolId) -> Result<Hook<Option<SymbolId>>, Error> {
            Ok(if self.mode == 2 {
                Hook::Value(None)
            } else {
                Hook::Absent
            })
        }
        fn get_symbol_of_declaration(
            &mut self,
            _: NodeId,
        ) -> Result<Hook<Option<SymbolId>>, Error> {
            Ok(if self.mode == 3 {
                Hook::Value(None)
            } else {
                Hook::Absent
            })
        }
        fn get_merged_symbol(&mut self, _: SymbolId) -> Result<Hook<Option<SymbolId>>, Error> {
            Ok(if self.mode >= 4 {
                Hook::Value(None)
            } else {
                Hook::Absent
            })
        }
    }
    let (mut host, (module, function, local)) =
        Host::fixture("namespace N { export function f(){} f; }", false, |b| {
            let module = find(b.view(), b.source(), K::ModuleDeclaration, 0);
            let function = find(b.view(), b.source(), K::FunctionDeclaration, 0);
            let module_symbol = symbol(b, "N", flags::VALUE_MODULE, &[module]);
            let function_symbol = symbol(b, "f", flags::FUNCTION, &[function]);
            b.symbols_mut().get_mut(function_symbol).unwrap().parent = Some(module_symbol);
            let local = symbol(b, "f", flags::EXPORT_VALUE, &[function]);
            b.symbols_mut().get_mut(local).unwrap().export_symbol = Some(function_symbol);
            b.binding_mut(module).unwrap().symbol = Some(module_symbol);
            b.binding_mut(function).unwrap().symbol = Some(local);
            (module, function, local)
        });
    let statement = host.find(K::ExpressionStatement, 0);
    let reference = host.node(statement).unwrap().expression().unwrap();
    for mode in 0..6 {
        let mut hooks = Hooks {
            symbol: local,
            mode,
        };
        let mut resolver = ReferenceResolver::new(options());
        let mut got = None;
        let panic = panic_message(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || {
                got = resolver
                    .get_referenced_export_container(
                        &mut host,
                        &mut hooks,
                        reference,
                        mode != 0 && mode != 5,
                    )
                    .unwrap();
            },
        )));
        obs!(
            &format!("export/container/{mode}"),
            got == Some(module),
            panic
        );
    }
    struct Member {
        symbol: SymbolId,
        mode: usize,
    }
    impl ReferenceResolverHooks for Member {
        fn get_resolved_symbol(&mut self, _: NodeId) -> Result<Hook<Option<SymbolId>>, Error> {
            Ok(if self.mode == 0 {
                Hook::Absent
            } else {
                Hook::Value(Some(self.symbol))
            })
        }
        fn get_export_symbol_of_value_symbol_if_exported(
            &mut self,
            _: SymbolId,
        ) -> Result<Hook<Option<SymbolId>>, Error> {
            Ok(if self.mode == 2 {
                Hook::Value(None)
            } else {
                Hook::Absent
            })
        }
    }
    for mode in 0..3 {
        let mut hooks = Member {
            symbol: local,
            mode,
        };
        let resolver = ReferenceResolver::new(options());
        let mut got = None;
        let panic = panic_message(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || {
                got = resolver
                    .get_referenced_member_value_declaration(&host, &mut hooks, function)
                    .unwrap();
            },
        )));
        obs!(
            &format!("member/value/{mode}"),
            got == Some(function),
            panic
        );
    }
    let (mut host, (declaration, alias)) =
        Host::fixture("import type {A} from 'm'; A;", false, |b| {
            let declaration = find(b.view(), b.source(), K::ImportSpecifier, 0);
            (declaration, symbol(b, "A", flags::ALIAS, &[declaration]))
        });
    struct Alias(SymbolId);
    impl ReferenceResolverHooks for Alias {
        fn get_resolved_symbol(&mut self, _: NodeId) -> Result<Hook<Option<SymbolId>>, Error> {
            Ok(Hook::Value(Some(self.0)))
        }
        fn get_type_only_alias_declaration(
            &mut self,
            _: SymbolId,
            _: SymbolFlags,
        ) -> Result<Hook<Option<NodeId>>, Error> {
            Ok(Hook::Value(None))
        }
    }
    let reference = host.node(declaration).unwrap().name().unwrap();
    obs!(
        "import/hook-nil",
        ReferenceResolver::new(options())
            .get_referenced_import_declaration(&mut host, &mut Alias(alias), reference)
            .unwrap()
            == Some(declaration)
    );
    let (mut host, value) = Host::fixture("export {}; let x; x;", false, |b| {
        let value = find(b.view(), b.source(), K::VariableDeclaration, 0);
        let symbol = symbol(b, "x", flags::BLOCK_SCOPED_VARIABLE, &[value]);
        let table = b.tables_mut().alloc(
            [(JsString::from_bytes(b"x".as_slice()), Some(symbol))]
                .into_iter()
                .collect(),
        );
        let source = b.source();
        b.binding_mut(source).unwrap().locals = Some(table);
        value
    });
    let statement = host.find(K::ExpressionStatement, 0);
    let reference = host.node(statement).unwrap().expression().unwrap();
    obs!(
        "reference/fallback",
        ReferenceResolver::new(options())
            .get_referenced_value_declaration(&mut host, &mut NoReferenceResolverHooks, reference)
            .unwrap()
            == Some(value)
    );
}

fn panic_message(result: std::thread::Result<()>) -> String {
    match result {
        Ok(()) => String::new(),
        Err(reason) => reason
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| reason.downcast_ref::<&str>().map(ToString::to_string))
            .expect("source nil dereference panic payload"),
    }
}

#[test]
fn pinned_heritage_short_circuit_does_not_read_an_unneeded_parent() {
    let (mut host, detached) = Host::fixture("class C extends Base {}", false, |builder| {
        let node = find(
            builder.view(),
            builder.source(),
            K::ExpressionWithTypeArguments,
            0,
        );
        builder.node_mut(node).unwrap().set_parent(None);
        node
    });
    let mut resolver = NameResolver::new(options());
    let mut result = None;
    let panic = panic_message(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || {
            result = resolver
                .resolve(
                    &mut host,
                    &mut NoNameResolverHooks,
                    Some(detached),
                    b"x",
                    flags::VALUE,
                    None,
                    false,
                    true,
                )
                .unwrap();
        },
    )));
    obs!("heritage/detached", result.is_none(), panic);
}
