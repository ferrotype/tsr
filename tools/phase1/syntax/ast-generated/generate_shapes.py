"""Generate actual per-shape calls and inputs, not factory or visitor logic."""
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
PREFIX = 'tsc/internal/ast/ast_generated.go:'
SKIPPED = {'SourceFile': 'handwritten source metadata requires its own owner fixture',
           'SyntheticExpression': 'checker-owned any payload is not an AST factory port'}


def snake(name):
    value = re.sub(r'([a-z0-9])([A-Z])', r'\1_\2', re.sub(r'([A-Z])([A-Z][a-z])', r'\1_\2', name)).lower()
    return 'r#' + value if value in {'type', 'await', 'yield'} else value


def signatures(source):
    return {identity: (name, [tuple(p.strip().split(': ', 1)) for p in params.split(',') if p.strip() and p.strip() != '&mut self'])
            for identity, name, params in re.findall(r'// upstream: (tsc/internal/ast/ast_generated.go:[^\n]+)\s+fn (\w+)\((.*?)\) -> NodeId', source, re.S)}


def inventory():
    schema = json.loads((ROOT / 'data/s03/schema/ast.json').read_text())
    go = (ROOT / 'upstream/tsc/internal/ast/ast_generated.go').read_text()
    rust = signatures((ROOT / 'crates/tsr_ast/src/factory_generated.rs').read_text())
    rows = []
    for shape in schema['nodes']:
        name = shape['name']
        if name in SKIPPED:
            continue
        members = [m for m in shape['members'] if not m['noFactory']]
        identity = PREFIX + 'NodeFactory.New' + name
        if identity not in rust:
            raise ValueError(f'factory missing actual Rust dispatch: {identity}')
        new_name, params = rust[identity]
        match = re.search(r'func \(f \*NodeFactory\) New' + name + r'\(([^\n]*)\) \*Node', go)
        if not match:
            raise ValueError(f'factory missing native dispatch: {name}')
        go_params = [tuple(p.split(' ', 1)) for p in match[1].split(', ')] if match[1] else []
        if len(members) != len(params) or len(params) != len(go_params):
            raise ValueError(f'factory/schema parameter count drift: {name}')
        for i, (member, (_, ty), (_, go_ty)) in enumerate(zip(members, params, go_params)):
            if ty not in {'Option<NodeId>', 'Option<NodeListId>', 'NodeKind', 'NodeSlice', 'TextSlice', 'JsString', 'bool', 'u32', 'i32'}:
                raise ValueError(f'unsupported actual factory argument: {name} {member["name"]} {ty}')
            member = dict(member)
            members[i] = {**member, 'rust_type': ty, 'go_type': go_ty, 'index': i}
        update = rust.get(PREFIX + 'NodeFactory.Update' + name)
        update_members = []
        if update:
            by_param = {param[0]: m for param, m in zip(params, members)}
            for param_name, ty in update[1][1:]:
                if param_name not in by_param or by_param[param_name]['rust_type'] != ty:
                    raise ValueError(f'update/schema signature drift: {name} {param_name}')
                update_members.append(by_param[param_name])
        capabilities = {
            'clone': bool(re.search(r'func \(node \*' + name + r'\) Clone\(', go)),
            'children': bool(re.search(r'func \(node \*' + name + r'\) ForEachChild\(', go)),
            'visitor': bool(re.search(r'func \(node \*' + name + r'\) VisitEachChild\(', go)),
            'facts': bool(re.search(r'func \(node \*' + name + r'\) computeSubtreeFacts\(', go)),
            'name': bool(re.search(r'func \(node \*' + name + r'\) Name\(', go)),
        }
        rows.append({**shape, 'members': members, 'new_name': new_name, 'update': update,
                     'update_members': update_members, 'capabilities': capabilities})
    if len(rows) != len(schema['nodes']) - len(SKIPPED):
        raise ValueError('generated shape inventory shrank')
    return rows


def getter(m, language):
    field = m['name']
    if language == 'rust':
        if m['kindParameter']:
            return 'n.kind()'
        if field == 'Flags':
            return 'n.flags()'
        method = snake(field) + ('_owned' if m['rust_type'] == 'JsString' else '')
        return f'd.{method}()'
    return 'd.' + field


def input_arg(m, shape, language, changed=False):
    i, ty = m['index'], m['rust_type']
    alter = 'true' if changed else 'false'
    prefix = f'b.{"" if language == "rust" else ""}'
    call = {'Option<NodeId>':'node', 'Option<NodeListId>':'list', 'NodeSlice':'nodes', 'TextSlice':'texts', 'JsString':'text', 'bool':'boolean', 'u32':'number', 'i32':'number', 'NodeKind':'kind'}[ty]
    if ty == 'Option<NodeListId>' and m['go_type'] == '*ModifierList':
        call = 'modifiers'
    if ty == 'NodeKind':
        if m['kindParameter']:
            kind = {'Token':'PlusToken', 'KeywordExpression':'TrueKeyword', 'KeywordTypeNode':'StringKeyword'}.get(shape['name'], shape['kinds'][0])
        else:
            kind = 'TypeKeyword' if m['name'] == 'PhaseModifier' else 'PlusToken'
        if language == 'rust':
            return f'Inputs::kind(SyntaxKind::{kind}.into(), {alter})'
        return f'b.kind(Kind{kind}, {alter})'
    if language == 'rust':
        call = 'list' if call == 'modifiers' else call
        if ty == 'Option<NodeListId>':
            return f'b.list({i}, {str(m["go_type"] == "*ModifierList").lower()}, {alter})'
        receiver = 'Inputs::' if ty in {'JsString', 'bool', 'u32', 'i32'} else prefix
        result = f'{receiver}{call}({i}, {alter})'
        return result + (' as i32' if ty == 'i32' else '')
    result = f'{prefix}{call}({i}, {alter})'
    if ty in ('u32', 'i32'):
        return m['go_type'] + '(' + result + ')'
    return result


def observe_field(m, language):
    g = getter(m, language)
    ty = m['rust_type']
    if language == 'rust':
        if ty == 'Option<NodeId>': return f'pos(f, {g})'
        if ty == 'Option<NodeListId>': return f'list_snapshot(f, {g})'
        if ty == 'NodeSlice': return f'nodes_snapshot(f, {g})'
        if ty == 'TextSlice': return f'texts_snapshot(f, {g})'
        if ty == 'NodeKind': return f'json!({g}.raw())'
        if ty == 'JsString': return f'json!(String::from_utf8({g}.as_bytes().to_vec()).unwrap())'
        return f'json!({g})'
    if ty == 'Option<NodeId>': return f'phase1GeneratedPos({g})'
    if ty == 'Option<NodeListId>':
        return f'phase1Shape{"Modifiers" if m["go_type"] == "*ModifierList" else "List"}Snapshot({g})'
    if ty == 'NodeSlice': return f'phase1ShapeNodesSnapshot({g})'
    if ty == 'TextSlice': return f'phase1ShapeTextsSnapshot({g})'
    return g


def actions(shape):
    n = shape['name']; cap = shape['capabilities']
    rows = [('new',[PREFIX+'NodeFactory.New'+n]), ('cast',[PREFIX+'Node.As'+n])]
    if cap['name']: rows.append(('name',[PREFIX+n+'.Name']))
    if cap['children']: rows.append(('children-stop',[PREFIX+n+'.ForEachChild']))
    if shape['update']:
        rows.append(('update-same',[PREFIX+'NodeFactory.Update'+n]))
        rows.extend((f'update-{m["name"]}',[PREFIX+'NodeFactory.Update'+n]) for m in shape['update_members'])
    if cap['clone']: rows.append(('clone',[PREFIX+n+'.Clone']))
    if cap['visitor']:
        rows += [('visit-same',[PREFIX+n+'.VisitEachChild']),('visit-replace',[PREFIX+n+'.VisitEachChild'])]
    if cap['facts']: rows.append(('facts',[PREFIX+n+'.computeSubtreeFacts']))
    rows.append(('counts',[]))
    return rows


def requests():
    rows = []
    for shape in inventory():
        linked = actions(shape)
        for mode in ('nil','empty','nodes','nil-element'):
            rows.append({'case':f'syntax/generated-shape/{shape["name"]}/{mode}',
                'subject':'generatedShape', 'shape':shape['name'], 'mode':mode,
                'operation':PREFIX+'NodeFactory.New'+shape['name'],
                'operations':sorted({op for _, ops in linked for op in ops}),
                'operation_actions':dict(linked), 'actions':[{'op':label} for label,_ in linked],
                'discriminates':'actual per-shape constructors and payload access; distinct fields, individual update changes, clone and visitor identity/hooks, child order and nonzero facts'})
    for shape in SKIPPED:
        if shape == "SourceFile":
            linked = [("new",["tsc/internal/ast/ast.go:NodeFactory.NewSourceFile"]),("cast",[PREFIX+"Node.AsSourceFile"])]
        else:
            linked = [("cast",[PREFIX+"Node.AsSyntheticExpression"]),("children-stop",[PREFIX+"SyntheticExpression.ForEachChild"])]
        for mode in ("nil","empty","nodes","nil-element"):
            rows.append({"case":f"syntax/generated-special/{shape}/{mode}","subject":"generatedSpecial","shape":shape,"mode":mode,
                "operation":linked[0][1][0],"operations":[op for _, ops in linked for op in ops],"operation_actions":dict(linked),"actions":[{"op":label} for label,_ in linked],
                "discriminates":"SourceFile factory metadata/text/syntax, or SyntheticExpression payload cast and child traversal only; no semantic Type payload contract is claimed"})
    return rows


def sources():
    rust = ['// Generated actual API dispatch by generate_shapes.py; no expected behavior.', '#[rustfmt::skip]', 'use super::{json, list_snapshot, nodes_snapshot, pos, texts_snapshot, AstBuilder, Factory, FactoryMethods, Inputs, NodeId, SyntaxKind, Value};', '#[rustfmt::skip]', 'pub fn make(f: &mut AstBuilder, b: &Inputs, shape: &str) -> NodeId {', 'match shape {']
    go = ['// Code generated by generate_shapes.py: actual API dispatch, no expected behavior.', 'package ast', '', 'func phase1ShapeMake(f *NodeFactory, b *phase1ShapeInputs, shape string) *Node {', 'switch shape {']
    shapes = inventory()
    for s in shapes:
        n=s['name']; argsr=', '.join(input_arg(m,s,'rust') for m in s['members']); argsg=', '.join(input_arg(m,s,'go') for m in s['members'])
        rust.append(f'"{n}" => f.{s["new_name"]}({argsr}),')
        go += [f'case "{n}":',f'return f.New{n}({argsg})']
    rust += ['_ => panic!("unknown generated shape"),','}', '}']
    go += ['default: panic("unknown generated shape")','}', '}']
    rust += ['#[rustfmt::skip]','pub fn fields(f: &AstBuilder, id: NodeId, shape: &str) -> Value {','let n=f.node(id);','match shape {']
    go += ['func phase1ShapeFields(node *Node, shape string) any {','switch shape {']
    for s in shapes:
        n=s['name']; rsnake=snake(n)
        rf=', '.join(observe_field(m,'rust') for m in s['members']); gf=', '.join(observe_field(m,'go') for m in s['members'])
        # Even zero-field payloads are cast through the actual generated accessor.
        rust.append(f'"{n}" => {{ let d=n.as_{rsnake}().expect("matching payload"); let _ = &d; json!([{rf}]) }},')
        go += [f'case "{n}":',f'd:=node.As{n}(); _=d; return []any{{{gf}}}']
    rust += ['_ => panic!("unknown generated shape"),','}', '}']
    go += ['default: panic("unknown generated shape")','}', '}']
    rust += ['#[rustfmt::skip]','pub fn update(f: &mut AstBuilder, b: &Inputs, id: NodeId, shape: &str, changed: Option<usize>) -> NodeId {','let n=f.node(id);','match shape {']
    go += ['func phase1ShapeUpdate(f *NodeFactory, b *phase1ShapeInputs, node *Node, shape string, changed int) *Node {','switch shape {']
    for s in shapes:
        if not s['update']:continue
        n=s['name']; rm=[]; gm=[]
        for m in s['update_members']:
            i=m['index']; rm.append(f'let arg{i}=if changed==Some({i}) {{ {input_arg(m,s,"rust",True)} }} else {{ {getter(m,"rust")} }};')
            gm.append(f'arg{i}:={getter(m,"go")}; if changed=={i} {{ arg{i}={input_arg(m,s,"go",True)} }}')
        rs=', '.join(f'arg{m["index"]}' for m in s['update_members'])
        rust.append(f'"{n}" => {{ let d=n.as_{snake(n)}().unwrap(); {" ".join(rm)} drop(n); f.{s["update"][0]}(id{", " if rs else ""}{rs}) }},')
        go += [f'case "{n}":',f'd:=node.As{n}(); '+ '; '.join(gm),f'return f.Update{n}(d{", " if rs else ""}{rs})']
    rust += ['_ => panic!("shape has no update"),','}', '}']
    go += ['default: panic("shape has no update")','}', '}']
    rust += ['#[rustfmt::skip]',"pub fn action_labels(shape: &str) -> &'static [(&'static str, Option<usize>)] {", 'match shape {']
    go += ['type phase1ShapeAction struct { name string; field int }','func phase1ShapeActions(shape string) []phase1ShapeAction {','switch shape {']
    action_groups = {}
    for s in shapes:
        index={f'update-{m["name"]}':m['index'] for m in s['update_members']}
        rr=', '.join(f'("{label}", {"Some("+str(index[label])+")" if label in index else "None"})' for label,_ in actions(s))
        gg=', '.join(f'{{"{label}", {index.get(label,-1)}}}' for label,_ in actions(s))
        action_groups.setdefault(rr, []).append(s['name'])
        go += [f'case "{s["name"]}":',f'return []phase1ShapeAction{{{gg}}}']
    for row, names in action_groups.items():
        rust.append(' | '.join(json.dumps(name) for name in names) + f' => &[{row}],')
    rust += ['_ => panic!("unknown generated shape"),','}', '}']
    go += ['default: panic("unknown generated shape")','}', '}']
    return {'shapes.rs':'\n'.join(rust)+'\n','shapes_test.go':'\n'.join(go)+'\n'}
