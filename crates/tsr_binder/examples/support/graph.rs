use serde_json::{json, Value};
use std::collections::HashMap;
use ts_ast::{
    AstView, BindResult, DeclarationRead, DeclarationSlice, FlowData, FlowId, FlowListId,
    NodeDataRead, NodeId, NodeListId, NodeSlice, SymbolId, SymbolRead, SymbolTableId, TextSlice,
};

#[path = "syntax_generated.rs"]
mod syntax_generated;

#[derive(Clone, Copy)]
enum Work {
    Node(NodeId),
    List(NodeListId, bool),
    Nodes(NodeSlice),
    Texts(TextSlice),
    Symbol(SymbolId),
    Table(SymbolTableId),
    Declarations(DeclarationSlice),
    Flow(FlowId),
    FlowList(FlowListId),
}
impl Work {
    fn kind(self) -> &'static str {
        match self {
            Self::Node(_) => "node",
            Self::List(_, _) => "list",
            Self::Nodes(_) => "nodes",
            Self::Texts(_) => "texts",
            Self::Symbol(_) => "symbol",
            Self::Table(_) => "table",
            Self::Declarations(_) => "declarations",
            Self::Flow(_) => "flow",
            Self::FlowList(_) => "flow_list",
        }
    }
}
struct Graph<'a> {
    view: AstView<'a>,
    source: NodeId,
    result: Option<&'a BindResult>,
    queue: Vec<(usize, Work)>,
    nodes: HashMap<NodeId, usize>,
    lists: HashMap<NodeListId, usize>,
    node_slices: HashMap<NodeSlice, usize>,
    text_slices: HashMap<TextSlice, usize>,
    symbols: HashMap<SymbolId, usize>,
    tables: HashMap<SymbolTableId, usize>,
    declarations: HashMap<(u64, u32, usize, usize), usize>,
    flows: HashMap<FlowId, usize>,
    flow_lists: HashMap<FlowListId, usize>,
}
macro_rules! reference {
    ($this:ident,$map:ident,$key:expr,$value:expr) => {{
        let key = $key;
        if let Some(&id) = $this.$map.get(&key) {
            id
        } else {
            let id = $this.$map.len() + 1;
            $this.$map.insert(key, id);
            $this.queue.push((id, $value));
            id
        }
    }};
}
impl<'a> Graph<'a> {
    fn new(view: AstView<'a>, source: NodeId, result: Option<&'a BindResult>) -> Self {
        Self {
            view,
            source,
            result,
            queue: Vec::new(),
            nodes: HashMap::new(),
            lists: HashMap::new(),
            node_slices: HashMap::new(),
            text_slices: HashMap::new(),
            symbols: HashMap::new(),
            tables: HashMap::new(),
            declarations: HashMap::new(),
            flows: HashMap::new(),
            flow_lists: HashMap::new(),
        }
    }
    fn node_ref(&mut self, value: Option<NodeId>) -> usize {
        value.map_or(0, |id| reference!(self, nodes, id, Work::Node(id)))
    }
    fn list_ref(&mut self, value: Option<NodeListId>, modifier: bool) -> usize {
        value.map_or(0, |id| {
            reference!(self, lists, id, Work::List(id, modifier))
        })
    }
    fn node_slice_ref(&mut self, value: NodeSlice) -> usize {
        if value.is_nil() {
            0
        } else {
            reference!(self, node_slices, value, Work::Nodes(value))
        }
    }
    fn text_slice_ref(&mut self, value: TextSlice) -> usize {
        if value.is_nil() {
            0
        } else {
            reference!(self, text_slices, value, Work::Texts(value))
        }
    }
    fn symbol_ref(&mut self, value: Option<SymbolId>) -> usize {
        value.map_or(0, |id| reference!(self, symbols, id, Work::Symbol(id)))
    }
    fn table_ref(&mut self, value: Option<SymbolTableId>) -> usize {
        value.map_or(0, |id| reference!(self, tables, id, Work::Table(id)))
    }
    fn declaration_ref(&mut self, value: DeclarationSlice) -> usize {
        if value.is_nil() {
            0
        } else {
            let key = (
                value.backing_id().unwrap().bits(),
                value.start(),
                value.len(),
                value.capacity(),
            );
            reference!(self, declarations, key, Work::Declarations(value))
        }
    }
    fn flow_ref(&mut self, value: Option<FlowId>) -> usize {
        value.map_or(0, |id| reference!(self, flows, id, Work::Flow(id)))
    }
    fn flow_list_ref(&mut self, value: Option<FlowListId>) -> usize {
        value.map_or(0, |id| reference!(self, flow_lists, id, Work::FlowList(id)))
    }
    fn node_refs(&mut self, nodes: DeclarationRead<'_>) -> Vec<usize> {
        nodes.iter().map(|id| self.node_ref(id)).collect()
    }
    fn result(&self) -> &'a BindResult {
        self.result
            .expect("S07 observer: binding-owned graph reached before binding")
    }
    fn name(&mut self, raw: &[u8], symbol: Option<SymbolRead<'_>>) -> Value {
        let mut result = json!({"raw_hex":hex(raw),"identity":null});
        let Some(symbol) = symbol else { return result };
        if !raw.starts_with(b"\xfe#") && !raw.starts_with(b"\xfe\"") {
            return result;
        }
        let view = self.view;
        for declaration in self
            .result()
            .declarations()
            .get(symbol.declarations())
            .expect("symbol declarations")
            .iter()
            .flatten()
        {
            let Some(name) =
                ts_ast::get_name_of_declaration(view, Some(declaration)).expect("declaration name")
            else {
                continue;
            };
            if ts_ast::is_ambient_module(view, declaration).expect("ambient module") {
                let node = view.node(declaration).expect("module declaration");
                let attributes = node
                    .data_source()
                    .as_module_declaration()
                    .expect("module payload")
                    .attributes();
                let name = view.node_text(name).expect("module name");
                let pattern = ts_core::pattern::Pattern::parse(name.as_bytes());
                if let Some(attributes) =
                    attributes.filter(|_| pattern.is_valid() && pattern.star_index >= 0)
                {
                    let mut prefix = b"\xfe\"".to_vec();
                    prefix.extend_from_slice(name.as_bytes());
                    prefix.extend_from_slice(b"\"pattern@");
                    let runtime = ts_ast::existing_runtime_node_id(
                        &view.node(attributes).expect("module attributes"),
                    );
                    let mut expected = prefix.clone();
                    expected.extend_from_slice(runtime.to_string().as_bytes());
                    if raw == expected {
                        result["identity"] = json!({"kind":"node","ref":self.node_ref(Some(attributes)),"prefix_hex":hex(&prefix),"suffix_hex":""});
                        return result;
                    }
                }
            }
            if view.node(name).expect("declaration name").kind()
                == ts_ast::SyntaxKind::PrivateIdentifier
            {
                let Some(class) = ts_ast::utilities::get_containing_class(view, declaration)
                    .expect("private class")
                else {
                    continue;
                };
                let Some(owner) = self
                    .result()
                    .node_binding(view, class)
                    .and_then(|binding| binding.symbol)
                else {
                    continue;
                };
                let owner_symbol = self.result().symbols().get(owner).expect("class symbol");
                let runtime = ts_ast::existing_runtime_symbol_id(&owner_symbol);
                let prefix = b"\xfe#";
                let mut suffix = b"@".to_vec();
                suffix.extend_from_slice(view.node_text(name).expect("private name").as_bytes());
                let mut expected = prefix.to_vec();
                expected.extend_from_slice(runtime.to_string().as_bytes());
                expected.extend_from_slice(&suffix);
                if raw == expected {
                    result["identity"] = json!({"kind":"symbol","ref":self.symbol_ref(Some(owner)),"prefix_hex":hex(prefix),"suffix_hex":hex(&suffix)});
                    return result;
                }
            }
        }
        result
    }
    fn diagnostic(&mut self, d: &ts_ast::Diagnostic) -> Value {
        json!({"file":self.node_ref(d.file),"pos":d.loc.pos(),"end":d.loc.end(),"code":d.code,"category":d.category,"source_hex":hex(d.source.as_bytes()),"key_hex":hex(d.message_key.as_bytes()),"text_hex":hex(d.message_text.as_bytes()),"args_hex":d.message_args.iter().map(|s|hex(s.as_bytes())).collect::<Vec<_>>(),"chain":d.message_chain.iter().map(|item|self.diagnostic(item)).collect::<Vec<_>>(),"related":d.related_information.iter().map(|item|self.diagnostic(item)).collect::<Vec<_>>(),"unnecessary":d.reports_unnecessary,"deprecated":d.reports_deprecated,"skipped":d.skipped_on_no_emit})
    }
    fn source_record(&mut self) -> Value {
        let root = self.node_ref(Some(self.source));
        let view = self.view;
        let file = view.source_file(self.source).expect("source metadata");
        let patterns=self.result.map_or_else(Vec::new,|result|result.pattern_ambient_modules().iter().map(|item|json!({"text_hex":hex(&item.pattern.text),"star_index":item.pattern.star_index,"symbol":self.symbol_ref(item.symbol)})).collect());
        let mut diagnostics = Vec::new();
        for (collection, items) in [
            ("parse", file.diagnostics.as_slice()),
            ("js", file.js_diagnostics.as_slice()),
            ("jsdoc", file.jsdoc_diagnostics.as_slice()),
            ("bind", self.result.map_or(&[][..], BindResult::diagnostics)),
        ] {
            for (index, diagnostic) in items.iter().enumerate() {
                diagnostics.push(json!([collection, index, self.diagnostic(diagnostic)]));
            }
        }
        json!({"root":root,"node_count":file.node_count,"text_count":file.text_count,"identifier_count":file.identifier_count,"symbol_count":self.result.map_or(0,BindResult::symbol_count),"script_kind":file.script_kind.0,"language_variant":file.language_variant.0,"declaration_file":file.is_declaration_file,"is_bound":self.result.is_some(),"common_js":self.node_ref(file.common_js_module_indicator()),"external_module":self.node_ref(file.external_module_indicator),"global_exports":self.table_ref(self.result.and_then(BindResult::global_exports)),"patterns":patterns,"diagnostics":diagnostics})
    }
    fn record(&mut self, id: usize, work: Work) -> Value {
        let view = self.view;
        match work {
            Work::Node(node_id) => {
                let node = view.node(node_id).expect("graph node");
                let parent = self.node_ref(node.parent());
                let (payload, fields) = self.syntax_fields(&node);
                let docs = if node.flags() & ts_ast::node_flags::HAS_JS_DOC != 0 {
                    view.source_eager_jsdoc(self.source, node_id)
                        .expect("eager JSDoc")
                        .map_or_else(Vec::new, |nodes| {
                            nodes
                                .iter()
                                .map(|&node| self.node_ref(Some(node)))
                                .collect()
                        })
                } else {
                    Vec::new()
                };
                let binding = self
                    .result
                    .and_then(|result| result.node_binding(view, node_id))
                    .unwrap_or_default();
                json!({"id":id,"kind":node.kind().raw(),"flags":node.flags(),"pos":node.pos(),"end":node.end(),"parent":parent,"payload":payload,"fields":fields,"jsdoc":docs,"symbol":self.symbol_ref(binding.symbol),"local_symbol":self.symbol_ref(binding.local_symbol),"locals":self.table_ref(binding.locals),"next_container":self.node_ref(binding.next_container),"flow_node":self.flow_ref(binding.flow_node),"end_flow_node":self.flow_ref(binding.end_flow_node),"return_flow_node":self.flow_ref(binding.return_flow_node),"fallthrough_flow_node":self.flow_ref(binding.fallthrough_flow_node)})
            }
            Work::List(list_id, modifier) => {
                let list = view.list(list_id).expect("syntax list");
                json!({"id":id,"pos":list.loc().pos(),"end":list.loc().end(),"nodes":self.node_slice_ref(list.nodes()),"missing":list.is_missing(),"modifier_flags":list.modifier_flags(),"is_modifier":modifier})
            }
            Work::Nodes(nodes) => {
                json!({"id":id,"values":view.node_slice(nodes).expect("syntax node slice").iter().map(|id| self.node_ref(id)).collect::<Vec<_>>()})
            }
            Work::Texts(texts) => {
                json!({"id":id,"values_hex":view.text_slice(texts).expect("syntax text slice").iter().map(|value|hex(value.as_bytes())).collect::<Vec<_>>()})
            }
            Work::Symbol(symbol_id) => {
                let symbol = self
                    .result()
                    .symbols()
                    .get(symbol_id)
                    .expect("graph symbol");
                json!({"id":id,"flags":symbol.flags(),"check_flags":symbol.check_flags(),"name":self.name(symbol.name_bytes(),Some(symbol)),"declarations":self.declaration_ref(symbol.declarations()),"value_declaration":self.node_ref(symbol.value_declaration()),"members":self.table_ref(symbol.members()),"exports":self.table_ref(symbol.exports()),"parent":self.symbol_ref(symbol.parent()),"export_symbol":self.symbol_ref(symbol.export_symbol())})
            }
            Work::Table(table_id) => {
                let table = self.result().tables().get(table_id).expect("symbol table");
                let mut entries = table.iter().collect::<Vec<_>>();
                entries.sort_by_key(|(name, _)| *name);
                let entries = entries
                    .into_iter()
                    .map(|(key, symbol)| {
                        let record =
                            symbol.map(|id| self.result().symbols().get(id).expect("table symbol"));
                        json!([self.name(key, record), self.symbol_ref(symbol)])
                    })
                    .collect::<Vec<_>>();
                json!({"id":id,"entries":entries})
            }
            Work::Declarations(declarations) => {
                let nodes = self
                    .result()
                    .declarations()
                    .get(declarations)
                    .expect("declaration slice");
                json!({"id":id,"values":self.node_refs(nodes),"capacity":declarations.capacity(),"capacity_values":self.node_refs(self.result().declarations().get(declarations.slice(0..declarations.capacity()).expect("declaration capacity header")).expect("declaration capacity backing"))})
            }
            Work::Flow(flow_id) => {
                let flow = self.result().flows().get(flow_id).expect("flow node");
                let data = match flow.node() {
                    None => Value::Null,
                    Some(FlowData::Ast(node)) => json!(["ast", self.node_ref(Some(node))]),
                    Some(FlowData::SwitchClause(data)) => json!([
                        "switch",
                        self.node_ref(data.switch_statement),
                        data.clause_start,
                        data.clause_end
                    ]),
                    Some(FlowData::ReduceLabel(data)) => json!([
                        "reduce",
                        self.flow_ref(data.target),
                        self.flow_list_ref(data.antecedents)
                    ]),
                };
                json!({"id":id,"flags":flow.flags(),"data":data,"synthetic":match flow.node() {Some(FlowData::SwitchClause(v))=>json!({"owner_flow":id,"payload":"switch","kind":v.kind().raw(),"flags":0,"pos":v.range().pos(),"end":v.range().end(),"parent":0}),Some(FlowData::ReduceLabel(v))=>json!({"owner_flow":id,"payload":"reduce","kind":v.kind().raw(),"flags":0,"pos":v.range().pos(),"end":v.range().end(),"parent":0}),_=>Value::Null},"antecedent":self.flow_ref(flow.antecedent()),"antecedents":self.flow_list_ref(flow.antecedents())})
            }
            Work::FlowList(list_id) => {
                let list = self.result().flow_lists().get(list_id).expect("flow list");
                json!({"id":id,"flow":self.flow_ref(list.flow()),"next":self.flow_list_ref(list.next())})
            }
        }
    }
}
// Observe shared storage only where exported slice ranges overlap. Positions are
// relative to the first visible element in each connected component, never an
// allocator address or an inaccessible backing allocation boundary.
fn aliases(queue: &[(usize, Work)], records: &mut [Value]) {
    let mut spans = Vec::new();
    for (index, &(_, work)) in queue.iter().enumerate() {
        let (domain, backing, start, length) = match work {
            Work::Nodes(v) => (
                0,
                v.backing_id().map(ts_arena::AuxId::bits),
                v.start(),
                v.len(),
            ),
            Work::Texts(v) => (
                1,
                v.backing_id().map(ts_arena::AuxId::bits),
                v.start(),
                v.len(),
            ),
            Work::Declarations(v) => (
                0,
                v.backing_id().map(ts_arena::AuxId::bits),
                v.start(),
                v.capacity(),
            ),
            _ => continue,
        };
        records[index]["backing_group"] = json!(0);
        records[index]["backing_start"] = json!(0);
        if length > 0 {
            spans.push((
                index,
                domain,
                backing.expect("nonempty slice backing"),
                u64::from(start),
                u64::from(start) + length as u64,
            ));
        }
    }
    let mut order = (0..spans.len()).collect::<Vec<_>>();
    order.sort_by_key(|&i| (spans[i].1, spans[i].2, spans[i].3));
    let mut components = vec![0; spans.len()];
    let mut minima = HashMap::new();
    let mut begin = 0;
    while begin < order.len() {
        let first = order[begin];
        let mut end = spans[first].4;
        let mut next = begin + 1;
        components[first] = first;
        minima.insert(first, spans[first].3);
        while next < order.len() {
            let other = order[next];
            if (spans[other].1, spans[other].2) != (spans[first].1, spans[first].2)
                || spans[other].3 >= end
            {
                break;
            }
            components[other] = first;
            end = end.max(spans[other].4);
            next += 1;
        }
        begin = next;
    }
    let mut groups = HashMap::new();
    for (i, span) in spans.iter().enumerate() {
        let key = components[i];
        let next = groups.len() + 1;
        let group = *groups.entry(key).or_insert(next);
        records[span.0]["backing_group"] = json!(group);
        records[span.0]["backing_start"] = json!(span.3 - minima[&key]);
    }
}
/// Complete graph collection shared by the binder protocol and the untimed
/// benchmark reporting mode. No production node or runtime identity is mutated.
pub fn collect(
    view: AstView<'_>,
    source: NodeId,
    result: Option<&BindResult>,
) -> Vec<(&'static str, Value)> {
    let mut graph = Graph::new(view, source, result);
    let source = graph.source_record();
    let mut records = Vec::new();
    let mut index = 0;
    while let Some(&(id, item)) = graph.queue.get(index) {
        records.push(graph.record(id, item));
        index += 1;
    }
    aliases(&graph.queue, &mut records);
    let mut output = Vec::with_capacity(records.len() + 2);
    output.push(("source", source));
    for ((_, item), record) in graph.queue.iter().zip(records) {
        output.push((item.kind(), record));
    }
    output.push(("counts",json!({"node":graph.nodes.len(),"list":graph.lists.len(),"nodes":graph.node_slices.len(),"texts":graph.text_slices.len(),"symbol":graph.symbols.len(),"table":graph.tables.len(),"declarations":graph.declarations.len(),"flow":graph.flows.len(),"flow_list":graph.flow_lists.len()})));
    output
}
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    result
}
