use crate::{
    change_nodes::NodeTracker, organize_compare::Comparers, organize_imports::module_name, Result,
};
use tsr_ast::{AstBuilder, AstView, FactoryMethods, NodeId, NodeKind, NodeListId, SyntaxKind as K};

pub(crate) fn list(ast: &mut AstBuilder, nodes: &[NodeId]) -> Result<NodeListId> {
    crate::completion_snippets::list(ast, &nodes.iter().copied().map(Some).collect::<Vec<_>>())
}
pub(crate) struct Import {
    pub id: NodeId,
    pub modifiers: Option<NodeListId>,
    pub clause: Option<NodeId>,
    pub phase: NodeKind,
    pub name: Option<NodeId>,
    pub bindings: Option<NodeId>,
    pub specifier: Option<NodeId>,
    pub attributes: Option<NodeId>,
}
impl Import {
    pub fn read(view: AstView<'_>, id: NodeId) -> Result<Self> {
        let read = view.node(id)?;
        let data = read
            .data_source()
            .as_import_declaration()
            .expect("import declaration");
        let mut result = Self {
            id,
            modifiers: data.modifiers(),
            clause: data.import_clause(),
            phase: K::Unknown.into(),
            name: None,
            bindings: None,
            specifier: data.module_specifier(),
            attributes: data.attributes(),
        };
        if let Some(clause) = result.clause {
            let read = view.node(clause)?;
            let data = read
                .data_source()
                .as_import_clause()
                .expect("import clause");
            result.phase = data.phase_modifier();
            result.name = data.name();
            result.bindings = data.named_bindings();
        }
        Ok(result)
    }
    pub fn update(&self, t: &mut NodeTracker<'_>, clause: Option<NodeId>) -> NodeId {
        t.ast.update_import_declaration(
            self.id,
            self.modifiers,
            clause,
            self.specifier,
            self.attributes,
        )
    }
}
pub(crate) fn import_kind(view: AstView<'_>, node: NodeId) -> u8 {
    let i = Import::read(view, node).expect("retained import");
    if i.clause.is_none() {
        0
    } else if i.phase == K::TypeKeyword {
        1
    } else if i
        .bindings
        .is_some_and(|b| view.node(b).is_ok_and(|b| b.kind() == K::NamespaceImport))
    {
        2
    } else if i.name.is_some() {
        3
    } else {
        4
    }
}
pub(crate) fn preserve_multiline(
    t: &mut NodeTracker<'_>,
    source: NodeId,
    old: NodeId,
    new: NodeId,
) -> Result<()> {
    let syntax = t.syntax(source)?;
    let read = t.ast.view().node(old)?;
    if read.pos() >= 0 && !syntax.same_line(i64::from(read.pos()), i64::from(read.end())) {
        t.emit
            .set_emit_flags(new, tsr_printer::emit_flags::MULTI_LINE);
    }
    Ok(())
}
// port: tsc/internal/ls/organizeimports.go:getImportAttributesKey
fn attribute_key(view: AstView<'_>, attributes: Option<NodeId>) -> Result<Vec<u8>> {
    let Some(id) = attributes else {
        return Ok(Vec::new());
    };
    let read = view.node(id)?;
    let attrs = read
        .data_source()
        .as_import_attributes()
        .expect("import attributes");
    let mut result = format!("{:?} ", attrs.token()).into_bytes();
    let mut nodes: Vec<_> = attrs
        .attributes()
        .map(|l| {
            view.node_slice(view.list(l)?.nodes())
                .map(|s| s.iter().flatten().collect())
        })
        .transpose()?
        .unwrap_or_default();
    nodes.sort_by(|&a, &b| {
        view.node_text(view.node(a).unwrap().name().unwrap())
            .unwrap()
            .as_bytes()
            .cmp(
                view.node_text(view.node(b).unwrap().name().unwrap())
                    .unwrap()
                    .as_bytes(),
            )
    });
    for node in nodes {
        let read = view.node(node)?;
        let data = read.data_source().as_import_attribute().expect("attribute");
        let name = read.name().expect("attribute name");
        let value = data.value().expect("attribute value");
        result.extend_from_slice(view.node_text(name)?.as_bytes());
        result.push(b':');
        let quoted = tsr_ast::utilities::is_string_literal_like(&view.node(value)?);
        if quoted {
            result.push(b'"');
        }
        result.extend_from_slice(view.node_text(value)?.as_bytes());
        if quoted {
            result.push(b'"');
        }
        result.push(b' ');
    }
    Ok(result)
}
#[derive(Default)]
struct Group {
    default: Vec<NodeId>,
    namespace: Vec<NodeId>,
    named: Vec<NodeId>,
}
// port: tsc/internal/ls/organizeimports.go:coalesceImportsWorker
pub(crate) fn imports(
    t: &mut NodeTracker<'_>,
    source: NodeId,
    imports: &[NodeId],
    compare: Comparers,
) -> Result<Vec<NodeId>> {
    let mut groups: Vec<(Vec<u8>, Vec<NodeId>)> = Vec::new();
    let mut indices = std::collections::HashMap::new();
    for &id in imports {
        let key = attribute_key(t.ast.view(), Import::read(t.ast.view(), id)?.attributes)?;
        let index = *indices.entry(key.clone()).or_insert_with(|| {
            groups.push((key, Vec::new()));
            groups.len() - 1
        });
        groups[index].1.push(id);
    }
    let mut result = Vec::new();
    for (_, imports) in groups {
        let mut side_effect = None;
        let mut groups = [Group::default(), Group::default()];
        for id in imports {
            let import = Import::read(t.ast.view(), id)?;
            if import.clause.is_none() {
                side_effect.get_or_insert(id);
                continue;
            }
            let group = &mut groups[usize::from(import.phase == K::TypeKeyword)];
            if import.name.is_some() {
                group.default.push(id);
            }
            if let Some(binding) = import.bindings {
                match t.ast.view().node(binding)?.kind().known() {
                    Some(K::NamespaceImport) => group.namespace.push(id),
                    Some(K::NamedImports) => group.named.push(id),
                    _ => {}
                }
            }
        }
        result.extend(side_effect);
        for (index, mut group) in groups.into_iter().enumerate() {
            if index == 0
                && group.default.len() == 1
                && group.namespace.len() == 1
                && group.named.is_empty()
            {
                let default = Import::read(t.ast.view(), group.default[0])?;
                let namespace = Import::read(t.ast.view(), group.namespace[0])?;
                let clause = t.ast.update_import_clause(
                    default.clause.unwrap(),
                    default.phase,
                    default.name,
                    namespace.bindings,
                );
                result.push(default.update(t, Some(clause)));
                continue;
            }
            group.namespace.sort_by(|&a, &b| {
                let name = |id| {
                    let import = Import::read(t.ast.view(), id).unwrap();
                    t.ast
                        .view()
                        .node_text(
                            t.ast
                                .view()
                                .node(import.bindings.unwrap())
                                .unwrap()
                                .name()
                                .unwrap(),
                        )
                        .unwrap()
                };
                compare
                    .modules
                    .compare(name(a).as_bytes(), name(b).as_bytes())
            });
            for id in group.namespace {
                let import = Import::read(t.ast.view(), id)?;
                let clause = t.ast.update_import_clause(
                    import.clause.unwrap(),
                    import.phase,
                    None,
                    import.bindings,
                );
                result.push(import.update(t, Some(clause)));
            }
            let Some(&id) = group.default.first().or_else(|| group.named.first()) else {
                continue;
            };
            let import = Import::read(t.ast.view(), id)?;
            let first_named = group.named.first().copied();
            let mut default_name = None;
            let mut specifiers = Vec::new();
            if group.default.len() == 1 {
                default_name = Import::read(t.ast.view(), group.default[0])?.name;
            } else {
                for id in group.default {
                    let name = Import::read(t.ast.view(), id)?.name;
                    let property = t
                        .ast
                        .new_identifier(tsr_ast::JsString::from_bytes(b"default".as_slice()));
                    specifiers.push(t.ast.new_import_specifier(false, Some(property), name));
                }
            }
            for id in group.named {
                let bindings = Import::read(t.ast.view(), id)?
                    .bindings
                    .expect("named imports");
                let elements: Vec<_> = t
                    .ast
                    .view()
                    .node_slice(t.ast.view().node(bindings)?.elements(t.ast.view())?)?
                    .iter()
                    .flatten()
                    .collect();
                for element in elements {
                    let view = t.ast.view();
                    let read = view.node(element)?;
                    let property = read.property_name();
                    let name = read.name();
                    let type_only = read.is_type_only();
                    let same = if let (Some(p), Some(n)) = (property, name) {
                        view.node_text(p)?.as_bytes() == view.node_text(n)?.as_bytes()
                    } else {
                        false
                    };
                    specifiers.push(if same {
                        t.ast
                            .update_import_specifier(element, type_only, None, name)
                    } else {
                        element
                    });
                }
            }
            specifiers.sort_by(|&a, &b| compare.specifiers(t.ast.view(), a, b));
            let named = if specifiers.is_empty() {
                if default_name.is_some() {
                    None
                } else {
                    let list = list(&mut t.ast, &[])?;
                    Some(t.ast.new_named_imports(Some(list)))
                }
            } else {
                let sorted = list(&mut t.ast, &specifiers)?;
                if let Some(first) = first_named {
                    let old = Import::read(t.ast.view(), first)?
                        .bindings
                        .expect("named imports");
                    let old_list = t
                        .ast
                        .view()
                        .node(old)?
                        .data_source()
                        .as_named_imports()
                        .and_then(|d| d.elements())
                        .expect("named list");
                    if t.ast.view().list_has_trailing_comma(old_list)? {
                        let loc = t.ast.view().list(old_list)?.loc();
                        t.ast.list_mut(sorted)?.set_loc(loc);
                    }
                    Some(t.ast.update_named_imports(old, Some(sorted)))
                } else {
                    Some(t.ast.new_named_imports(Some(sorted)))
                }
            };
            if let (Some(first), Some(new)) = (first_named, named) {
                preserve_multiline(
                    t,
                    source,
                    Import::read(t.ast.view(), first)?.bindings.unwrap(),
                    new,
                )?;
            }
            if index == 1 && default_name.is_some() && named.is_some() {
                let clause = t.ast.new_import_clause(import.phase, default_name, None);
                result.push(import.update(t, Some(clause)));
                let named_import = Import::read(t.ast.view(), first_named.unwrap_or(id))?;
                let clause = t.ast.new_import_clause(named_import.phase, None, named);
                result.push(named_import.update(t, Some(clause)));
            } else {
                let clause = t.ast.update_import_clause(
                    import.clause.unwrap(),
                    import.phase,
                    default_name,
                    named,
                );
                result.push(import.update(t, Some(clause)));
            }
        }
    }
    Ok(result)
}
// port: tsc/internal/ls/organizeimports.go:coalesceExportsWorker
pub(crate) fn exports(
    t: &mut NodeTracker<'_>,
    source: NodeId,
    exports: &[NodeId],
    compare: Comparers,
) -> Result<Vec<NodeId>> {
    let mut groups: Vec<(Vec<u8>, Vec<NodeId>)> = Vec::new();
    let mut indices = std::collections::HashMap::new();
    for &id in exports {
        let key = module_name(t.ast.view(), id)?;
        let index = *indices.entry(key.clone()).or_insert_with(|| {
            groups.push((key, Vec::new()));
            groups.len() - 1
        });
        groups[index].1.push(id);
    }
    groups.sort_by(|a, b| {
        a.0.is_empty()
            .cmp(&b.0.is_empty())
            .then_with(|| compare.modules.compare(&a.0, &b.0))
    });
    let mut result = Vec::new();
    for (_, group) in groups {
        let mut side_effect = None;
        let mut groups = [Vec::new(), Vec::new()];
        for id in group {
            let read = t.ast.view().node(id)?;
            let data = read.data_source().as_export_declaration().expect("export");
            if data.export_clause().is_none() {
                side_effect.get_or_insert(id);
            } else {
                groups[usize::from(data.is_type_only())].push(id);
            }
        }
        result.extend(side_effect);
        for group in groups {
            let Some(&first) = group.first() else {
                continue;
            };
            let mut specifiers = Vec::new();
            for id in group {
                let clause = t
                    .ast
                    .view()
                    .node(id)?
                    .data_source()
                    .as_export_declaration()
                    .and_then(|d| d.export_clause())
                    .expect("export clause");
                if t.ast.view().node(clause)?.kind() == K::NamedExports {
                    specifiers.extend(
                        t.ast
                            .view()
                            .node_slice(t.ast.view().node(clause)?.elements(t.ast.view())?)?
                            .iter()
                            .flatten(),
                    );
                }
            }
            specifiers.sort_by(|&a, &b| compare.specifiers(t.ast.view(), a, b));
            let read = t.ast.view().node(first)?;
            let data = read.data_source().as_export_declaration().expect("export");
            let (modifiers, type_only, clause, specifier, attributes) = (
                data.modifiers(),
                data.is_type_only(),
                data.export_clause(),
                data.module_specifier(),
                data.attributes(),
            );
            let mut new_clause = clause;
            if let Some(clause) = clause {
                if t.ast.view().node(clause)?.kind() == K::NamedExports {
                    let list = list(&mut t.ast, &specifiers)?;
                    let new = t.ast.update_named_exports(clause, Some(list));
                    preserve_multiline(t, source, clause, new)?;
                    new_clause = Some(new);
                }
            }
            result.push(t.ast.update_export_declaration(
                first, modifiers, type_only, new_clause, specifier, attributes,
            ));
        }
    }
    Ok(result)
}
