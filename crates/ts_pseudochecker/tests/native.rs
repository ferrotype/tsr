use serde_json::{json, Value};
use std::ops::ControlFlow;
use ts_arena::NodeId;
use ts_ast::{
    AstView, ChildVisitor, CompletedFile, JsString, NodeListId, NodeSlice, SourceFileParseOptions,
    SyntaxKind as K,
};
use ts_pseudochecker::*;

#[derive(Debug)]
struct TestError(String);
impl From<ts_pseudochecker::Error> for TestError {
    fn from(e: ts_pseudochecker::Error) -> Self {
        Self(e.to_string())
    }
}
impl From<ts_arena::Error> for TestError {
    fn from(e: ts_arena::Error) -> Self {
        Self(e.to_string())
    }
}
struct TestHost(CompletedFile);
impl Host for TestHost {
    type Error = TestError;
    fn ast(&self, n: NodeId) -> Result<AstView<'_>, TestError> {
        let v = self.0.view().ast();
        v.node(n)?;
        Ok(v)
    }
    fn raw_symbol_declarations(&self, n: NodeId) -> Result<Option<Vec<NodeId>>, TestError> {
        let v = self.0.view();
        let Some(symbol) = v.node_binding(n)?.and_then(|b| b.symbol) else {
            return Ok(None);
        };
        Ok(Some(
            v.result()
                .declarations()
                .get(v.symbol(symbol)?.declarations())?
                .iter()
                .flatten()
                .collect(),
        ))
    }
}
impl TestHost {
    fn node(&self, n: NodeId) -> Value {
        let n = self.0.view().node(n).unwrap();
        json!([n.kind().raw(), n.pos(), n.end()])
    }
    fn nodes(&self, ns: &[NodeId]) -> Value {
        Value::Array(ns.iter().map(|n| self.node(*n)).collect())
    }
    fn parameter(&self, p: &PseudoParameter) -> Value {
        json!({"rest":p.rest,"name":self.node(p.name),"optional":p.optional,"type":self.tree(&p.ty)})
    }
    fn parameters(&self, ps: &[PseudoParameter]) -> Value {
        Value::Array(ps.iter().map(|p| self.parameter(p)).collect())
    }
    fn signature(&self, s: &PseudoSignature, r: &mut Value) {
        r["node"] = self.node(s.signature);
        r["parameters"] = self.parameters(&s.parameters);
        r["type_parameters"] = self.nodes(&s.type_parameters);
        r["return"] = self.tree(&s.return_type);
    }
    fn tree(&self, t: &PseudoType) -> Value {
        let mut r = json!({"kind":t.kind() as i16});
        match t.as_ref(){
   PseudoTypeData::Direct{type_node}=>r["node"]=self.node(*type_node),
   PseudoTypeData::Inferred{expression,error_nodes,is_signature_return}=>{r["node"]=self.node(*expression);r["errors"]=self.nodes(error_nodes);r["return"]=json!(is_signature_return);}
   PseudoTypeData::NoResult{declaration}=>r["node"]=self.node(*declaration),
   PseudoTypeData::MaybeConstLocation{node,const_type,regular_type}=>{r["node"]=self.node(*node);r["const"]=self.tree(const_type);r["regular"]=self.tree(regular_type);}
   PseudoTypeData::Union{types}|PseudoTypeData::Tuple{elements:types}=>r["types"]=Value::Array(types.iter().map(|t|self.tree(t)).collect()),
   PseudoTypeData::StringLiteral{node}|PseudoTypeData::NumericLiteral{node}|PseudoTypeData::BigIntLiteral{node}=>r["node"]=self.node(*node),
   PseudoTypeData::SingleCallSignature(s)=>self.signature(s,&mut r),
   PseudoTypeData::ObjectLiteral{elements}=>r["elements"]=Value::Array(elements.iter().map(|e|{
    let mut m=json!({"kind":e.kind() as i8,"name":self.node(e.name),"optional":e.optional});match &e.data {
     PseudoObjectElementData::Method(s)=>self.signature(s,&mut m),
     PseudoObjectElementData::PropertyAssignment{readonly,ty}=>{m["readonly"]=json!(readonly);m["type"]=self.tree(ty);}
     PseudoObjectElementData::SetAccessor{signature,parameter}=>{m["node"]=self.node(*signature);m["parameter"]=self.parameter(parameter);}
     PseudoObjectElementData::GetAccessor{signature,ty}=>{m["node"]=self.node(*signature);m["type"]=self.tree(ty);}
    }m
   }).collect()),
   PseudoTypeData::Undefined|PseudoTypeData::Null|PseudoTypeData::Any|PseudoTypeData::String|PseudoTypeData::Number|PseudoTypeData::BigInt|PseudoTypeData::Boolean|PseudoTypeData::True|PseudoTypeData::False=>{}
  }
        r
    }
    fn children(&self, node: NodeId) -> Vec<NodeId> {
        struct Collect<'a> {
            view: AstView<'a>,
            nodes: Vec<NodeId>,
        }
        impl ChildVisitor for Collect<'_> {
            fn visit_node(&mut self, n: NodeId) -> ControlFlow<()> {
                self.nodes.push(n);
                ControlFlow::Continue(())
            }
            fn visit_list(&mut self, l: NodeListId) -> ControlFlow<()> {
                self.visit_node_slice(self.view.list(l).unwrap().nodes())
            }
            fn visit_node_slice(&mut self, s: NodeSlice) -> ControlFlow<()> {
                self.nodes
                    .extend(self.view.node_slice(s).unwrap().iter().flatten());
                ControlFlow::Continue(())
            }
        }
        let mut c = Collect {
            view: self.0.view().ast(),
            nodes: vec![],
        };
        let _ = c.view.node(node).unwrap().for_each_child(&mut c);
        c.nodes
    }
}
fn operations(k: K) -> Vec<&'static str> {
    let mut ops = vec![];
    if matches!(
        k,
        K::Parameter
            | K::VariableDeclaration
            | K::PropertySignature
            | K::PropertyDeclaration
            | K::JSDocPropertyTag
            | K::BindingElement
            | K::ExportAssignment
            | K::PropertyAccessExpression
            | K::ElementAccessExpression
            | K::BinaryExpression
            | K::PropertyAssignment
            | K::ShorthandPropertyAssignment
            | K::CallExpression
    ) {
        ops.push("declaration");
    }
    if matches!(
        k,
        K::OmittedExpression
            | K::ParenthesizedExpression
            | K::Identifier
            | K::NullKeyword
            | K::ArrowFunction
            | K::FunctionExpression
            | K::TypeAssertionExpression
            | K::AsExpression
            | K::PrefixUnaryExpression
            | K::ArrayLiteralExpression
            | K::ObjectLiteralExpression
            | K::ClassExpression
            | K::TemplateExpression
            | K::NumericLiteral
            | K::NoSubstitutionTemplateLiteral
            | K::StringLiteral
            | K::BigIntLiteral
            | K::TrueKeyword
            | K::FalseKeyword
            | K::CallExpression
    ) {
        ops.push("expression");
    }
    if matches!(
        k,
        K::GetAccessor
            | K::MethodDeclaration
            | K::FunctionDeclaration
            | K::Constructor
            | K::MethodSignature
            | K::CallSignature
            | K::ConstructSignature
            | K::SetAccessor
            | K::IndexSignature
            | K::FunctionType
            | K::ConstructorType
            | K::FunctionExpression
            | K::ArrowFunction
            | K::JSDocSignature
    ) {
        ops.push("return");
    }
    if matches!(k, K::GetAccessor | K::SetAccessor) {
        ops.push("accessor");
    }
    ops
}
#[allow(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "The pinned test filenames and upstream extension matching are case sensitive"
)]
fn parse(file: &str, source: &str) -> TestHost {
    let parsed = ts_parser::parse_source_file(
        ts_jsstring::SourceText::from_loaded_bytes(source.as_bytes()),
        if file.ends_with(".js") {
            ts_core::ScriptKind::JS
        } else {
            ts_core::ScriptKind::TS
        },
        SourceFileParseOptions {
            file_name: JsString::from_bytes(file.as_bytes()),
            ..Default::default()
        },
    );
    TestHost(ts_binder::bind_parsed_file(parsed).unwrap())
}
#[test]
fn complete_trees_and_error_nodes_match_pinned_native_pseudochecker() {
    let requests: Value = serde_json::from_str(include_str!(
        "../../../tools/s08/p4/pseudochecker/requests.json"
    ))
    .unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "../../../tools/s08/p4/pseudochecker/native.json"
    ))
    .unwrap();
    let expected = expected["rows"].as_array().unwrap();
    let mut index = 0;
    for case in requests.as_array().unwrap() {
        let host = parse(
            case["file"].as_str().unwrap_or("/pseudo.ts"),
            case["source"].as_str().unwrap(),
        );
        for strict in [false, true] {
            let checker = PseudoChecker::new(strict, false);
            let mut stack = vec![host.0.source()];
            while let Some(node) = stack.pop() {
                for op in operations(host.0.view().node(node).unwrap().kind().known().unwrap()) {
                    let ty = match op {
                        "declaration" => checker.get_type_of_declaration(&host, node),
                        "expression" => checker.get_type_of_expression(&host, node),
                        "return" => checker.get_return_type_of_signature(&host, node),
                        "accessor" => checker.get_type_of_accessor(&host, node),
                        _ => unreachable!(),
                    }
                    .unwrap_or_else(|e| panic!("{}", e.0));
                    let actual = json!({"case":case["id"],"strict":strict,"node":host.node(node),"operation":op,"const_context":is_in_const_context(&host,node).unwrap(),"could_be_undefined":could_already_refer_to_undefined_type(&host,&ty).unwrap(),"type":host.tree(&ty)});
                    assert_eq!(
                        &actual,
                        expected.get(index).unwrap(),
                        "native observation {index}"
                    );
                    index += 1;
                }
                stack.extend(host.children(node).into_iter().rev());
            }
        }
    }
    assert_eq!(index, expected.len());
    assert_eq!(index, 850);
}

#[test]
fn nested_const_tuple_construction_and_last_owner_drop_fit_small_stack() {
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(|| {
            let depth = 4_000;
            let source = format!(
                "const x = {}1{} as const;",
                "[".repeat(depth),
                "]".repeat(depth)
            );
            let host = parse("/pseudo.ts", &source);
            let mut pending = vec![host.0.source()];
            let declaration = loop {
                let node = pending.pop().expect("variable declaration");
                if host.0.view().node(node).unwrap().kind() == K::VariableDeclaration {
                    break node;
                }
                pending.extend(host.children(node));
            };
            let ty = PseudoChecker::new(true, false)
                .get_type_of_declaration(&host, declaration)
                .unwrap();
            let mut cursor = &ty;
            for _ in 0..depth {
                let PseudoTypeData::Tuple { elements } = cursor.as_ref() else {
                    panic!("expected nested tuple")
                };
                assert_eq!(elements.len(), 1);
                cursor = &elements[0];
            }
            assert_eq!(cursor.kind(), PseudoTypeKind::MaybeConstLocation);
            let retained = std::sync::Arc::clone(&ty);
            drop(ty);
            assert_eq!(retained.kind(), PseudoTypeKind::Tuple);
            drop(retained);
        })
        .unwrap()
        .join()
        .unwrap();
}
