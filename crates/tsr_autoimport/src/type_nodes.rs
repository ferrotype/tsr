use std::{cell::RefCell, collections::HashMap};
use tsr_ast::{
    FactoryMethods, JsString, NodeId, NodeVisitor, NodeVisitorHooks, RuntimeFactory,
    SyntaxKind as K,
};
use tsr_checker::{GeneratedTypeNodes, Operation, SymbolRef};

// port: tsc/internal/ls/autoimport/import_adder.go:getNameForExportedSymbol
fn export_name(
    checker: &Operation<'_>,
    symbol: SymbolRef,
    program: &tsr_compiler::Program,
) -> Result<JsString, tsr_checker::Error> {
    let read = checker.symbol(symbol)?;
    let name = JsString::from_bytes(read.name_bytes());
    if !matches!(name.as_bytes(), b"default" | b"export=") {
        return Ok(name);
    }
    let declarations = checker
        .symbol_declarations(symbol)?
        .iter()
        .flatten()
        .collect::<Vec<_>>();
    let local = crate::registry::declaration_name(program, &declarations)?;
    if !local.as_bytes().is_empty() {
        return Ok(local);
    }
    let parent = checker
        .symbol(symbol)?
        .parent()
        .ok_or(tsr_checker::Error::MissingLink("export parent"))?;
    let parent = checker.symbol_ref(parent)?;
    let name = checker.symbol(parent)?.name_bytes();
    let name = tsr_jsstring::text::unquote_string(name);
    Ok(JsString::from_bytes(crate::registry::module_identifier(
        &name,
    )))
}

// port: tsc/internal/ls/autoimport/import_adder.go:TryGetAutoImportableReferenceFromTypeNode
pub fn importable_references(
    nodes: &mut GeneratedTypeNodes,
    roots: &mut [NodeId],
    checker: &Operation<'_>,
    program: &tsr_compiler::Program,
) -> Result<Vec<SymbolRef>, tsr_checker::Error> {
    let mut names = HashMap::new();
    for (&id, &symbol) in &nodes.identifier_symbols {
        names.insert(id, (symbol, export_name(checker, symbol, program)?));
    }
    let symbols = RefCell::new(Vec::new());
    let visit = |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
        let node = node?;
        let import = {
            let read = visitor.factory().node(node);
            read.data_source()
                .as_import_type_node()
                .map(|d| (d.argument(), d.qualifier(), d.type_arguments()))
        };
        if let Some((Some(argument), Some(qualifier), arguments)) = import {
            let literal = {
                let f = visitor.factory();
                let arg = f.node(argument);
                arg.kind() == K::LiteralType
                    && arg
                        .data_source()
                        .as_literal_type_node()
                        .and_then(|d| d.literal())
                        .is_some_and(|l| f.node(l).kind() == K::StringLiteral)
            };
            if literal {
                let mut first = qualifier;
                while visitor.factory().node(first).kind() == K::QualifiedName {
                    first = visitor
                        .factory()
                        .node(first)
                        .data_source()
                        .as_qualified_name()
                        .and_then(|d| d.left())
                        .expect("qualified name left");
                }
                if let Some((symbol, name)) = names.get(&first) {
                    let old = {
                        JsString::from_bytes(
                            visitor
                                .factory()
                                .node(first)
                                .data_source()
                                .as_identifier()
                                .expect("import qualifier identifier")
                                .text(),
                        )
                    };
                    let qualifier = if name == &old {
                        qualifier
                    } else {
                        let new = visitor.factory_mut().new_identifier(name.clone());
                        replace_first(visitor.factory_mut(), qualifier, new)
                    };
                    symbols.borrow_mut().push(*symbol);
                    let arguments = visitor.visit_nodes(arguments);
                    return Some(
                        visitor
                            .factory_mut()
                            .new_type_reference_node(Some(qualifier), arguments),
                    );
                }
            }
        }
        visitor.visit_each_child(Some(node))
    };
    let mut visitor = NodeVisitor::new(
        Some(&visit),
        Some(&mut nodes.ast),
        NodeVisitorHooks::default(),
    );
    for root in roots {
        *root = visitor.visit_node(Some(*root)).expect("type syntax root");
    }
    Ok(symbols.into_inner())
}
// port: tsc/internal/ls/autoimport/import_adder.go:replaceFirstIdentifierOfEntityName
fn replace_first(factory: &mut dyn RuntimeFactory, node: NodeId, new: NodeId) -> NodeId {
    let mut current = node;
    let mut rights = Vec::new();
    while factory.node(current).kind() != K::Identifier {
        let read = factory.node(current);
        let d = read.data_source();
        let d = d.as_qualified_name().expect("entity name");
        rights.push(d.right());
        current = d.left().expect("qualified left");
    }
    let mut result = new;
    for right in rights.into_iter().rev() {
        result = factory.new_qualified_name(Some(result), right);
    }
    result
}
