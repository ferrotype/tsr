use super::{array, diagnostics, failure, graph, hex, node_json, owner, text, view, Error, Result};
use serde_json::{json, Value};
use std::sync::Arc;
use ts_arena::{Counters, Generation, NodeId};
use ts_ast::SyntaxKind;
use ts_checker::{type_format_flags, Operation, RetainedType, TypeRef};
use ts_compiler::{FileCache, Program};

pub fn declaration(program: &Program, path: &str, name: &str) -> Result<NodeId> {
    let file = program
        .file(path.as_bytes())
        .ok_or_else(|| Error::Protocol("query source absent".into()))?;
    let mut found = None;
    for id in graph::walk(program, file.source())? {
        let ast = view(program, id)?;
        let node = ast.node(id)?;
        if matches!(
            node.kind().known(),
            Some(
                SyntaxKind::TypeAliasDeclaration
                    | SyntaxKind::InterfaceDeclaration
                    | SyntaxKind::VariableDeclaration
            )
        ) {
            if let Some(node_name) = node.name() {
                if ast.node(node_name)?.kind() == SyntaxKind::Identifier
                    && ast.node_text(node_name)?.as_bytes() == name.as_bytes()
                    && found.replace(id).is_some()
                {
                    return Err(Error::Protocol("ambiguous declaration selector".into()));
                }
            }
        }
    }
    found.ok_or_else(|| Error::Protocol("declaration selector absent".into()))
}
const DEFAULT_DISPLAY_FLAGS: ts_checker::TypeFormatFlags =
    type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
        | type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE;

pub fn basic_type(op: &mut Operation<'_>, typ: TypeRef) -> Result<Value> {
    let flags = op.type_flags(typ)?;
    let object_flags = op.type_object_flags(typ)?;
    let display = op.type_to_string(typ, DEFAULT_DISPLAY_FLAGS)?;
    let alias = op.type_to_string(typ, type_format_flags::IN_TYPE_ALIAS)?;
    Ok(
        json!({"flags":flags,"object_flags":object_flags,"display_hex":hex(display.as_bytes()),"in_alias_display_hex":hex(alias.as_bytes())}),
    )
}
pub fn type_value(
    program: &Program,
    op: &mut Operation<'_>,
    typ: TypeRef,
    graph: &mut graph::Graph,
) -> Result<Value> {
    let mut value = basic_type(op, typ)?;
    let mut properties = Vec::new();
    for property in op.properties_of_type(typ)? {
        let symbol = graph.snapshot(program, Some(op), Some(property.id()))?;
        let typ = op.get_type_of_symbol(property)?;
        properties.push(json!({"symbol":symbol,"type":basic_type(op,typ)?}));
    }
    value["properties"] = json!(properties);
    Ok(value)
}
fn query(program: &Program, op: &mut Operation<'_>, request: &Value) -> Result<(Value, TypeRef)> {
    let decl = declaration(
        program,
        text(&request["file"])?,
        text(&request["declaration"])?,
    )?;
    let ast = view(program, decl)?;
    let declaration = ast.node(decl)?;
    let node = match text(&request["target"])? {
        "name" => declaration.name(),
        "annotation" => declaration.type_node(),
        "annotation_name" => {
            let annotation = declaration
                .type_node()
                .ok_or_else(|| Error::Protocol("query annotation absent".into()))?;
            ast.node(annotation)?
                .data_source()
                .as_type_reference_node()
                .ok_or_else(|| Error::Protocol("query annotation is not a reference".into()))?
                .type_name()
        }
        "initializer" => declaration.initializer(),
        _ => return Err(Error::Protocol("unknown query target".into())),
    }
    .ok_or_else(|| Error::Protocol("query target absent".into()))?;
    let symbol = op.get_symbol_at_location(node)?;
    let typ = match text(&request["operation"])? {
        "type_at_location" => op.get_type_at_location(node)?,
        "declared_type" | "declared_type_summary" => op.get_declared_type_of_symbol(
            symbol.ok_or_else(|| Error::Protocol("declared type symbol absent".into()))?,
        )?,
        _ => return Err(Error::Protocol("unknown query operation".into())),
    };
    let mut graph = graph::Graph::new(program, "query", None)?;
    let symbol = graph.snapshot(program, Some(op), symbol.map(ts_checker::SymbolRef::id))?;
    let value = if request["operation"] == "declared_type_summary" {
        basic_type(op, typ)?
    } else {
        type_value(program, op, typ, &mut graph)?
    };
    Ok((
        json!({"id":request["id"],"state":"executed","node":node_json(program,Some(node))?,"symbol":symbol,"type":value}),
        typ,
    ))
}
pub fn program(
    request: &Value,
    generation: &Generation,
    counters: &Counters,
    libraries: bool,
    overrides: super::FixtureOptions,
    program_diagnostics: bool,
) -> Result<(Value, Value)> {
    let program = super::load_with_libraries(
        &request["files"],
        &request["roots"],
        &mut FileCache::new(),
        counters,
        libraries,
        overrides,
    )?;
    let weak = Arc::downgrade(&program);
    let owner = owner(program.clone(), generation, counters)?;
    let mut op = owner.operation()?;
    let mut queries = Vec::new();
    let mut retained: Option<(RetainedType, Value)> = None;
    for request in array(&request["queries"])? {
        let mut value = match query(&program, &mut op, request) {
            Ok((value, typ)) => {
                if retained.is_none() {
                    retained = Some((op.retain_type(typ)?, value["type"]["display_hex"].clone()));
                }
                value
            }
            Err(error) => failure(&error, text(&request["operation"])?),
        };
        value["id"] = request["id"].clone();
        queries.push(value);
    }
    let diagnostics = diagnostics::all_mode(&program, &mut op, program_diagnostics)?;
    drop(op);
    drop(owner);
    drop(program);
    let lifetime = if let Some((retained, expected)) = retained {
        let survives = weak.upgrade().is_some();
        let actual = {
            let mut op = retained.owner().operation()?;
            let typ = op.import_type(&retained)?;
            hex(op.type_to_string(typ, DEFAULT_DISPLAY_FLAGS)?.as_bytes())
        };
        drop(retained);
        json!({"id":request["id"],"state":"executed","program_survives_retained_result":survives,"retained_display_unchanged":json!(actual)==expected,"program_released_after_result_drop":weak.upgrade().is_none()})
    } else {
        json!({"id":request["id"],"state":"unsupported","operation":"retain_type","reason":"no query produced a retainable type"})
    };
    Ok((
        json!({"id":request["id"],"state":"executed","queries":queries,"diagnostics":diagnostics}),
        lifetime,
    ))
}
