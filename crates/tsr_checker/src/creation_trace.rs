//! Read-only snapshots at the actual comparator fallback and sort sites.
//! Never use type display, lazy members or semantic-id getters to label a trace.

use crate::{CheckerState, TypeId};
use serde_json::{json, Value};
use tsr_arena::SymbolId;

impl CheckerState {
    pub(crate) fn trace_symbol(&self, id: SymbolId) -> Value {
        let Ok(symbol) = self.symbol(id) else {
            return json!({"unobserved":"symbol read failed"});
        };
        let declarations = self.symbol_declarations(id).map(|values| {
            values
                .iter()
                .flatten()
                .map(|node| {
                    self.node(node).map_or_else(
                    |_| json!({"unobserved":"declaration read failed"}),
                    |node| json!({"kind":node.kind().raw(), "pos":node.pos(), "end":node.end()}),
                )
                })
                .collect::<Vec<_>>()
        });
        json!({"token":tsr_ast::creation_trace::symbol_token(&symbol),
            "semantic_id":tsr_ast::existing_runtime_symbol_id(&symbol),
            "flags":symbol.flags(), "check_flags":symbol.check_flags(),
            "name":symbol.name_bytes(), "declarations":declarations.ok()})
    }

    pub(crate) fn trace_type(&self, id: TypeId) -> Value {
        let Ok(record) = self.types.get(id) else {
            return json!({"unobserved":"type read failed"});
        };
        json!({"owner":self.types.trace_owner.token(), "token":id.get(), "semantic_id":id.get(),
            "flags":record.flags, "object_flags":record.object_flags,
            "kind":format!("{:?}",record.kind()),
            "symbol":record.symbol.map(|symbol| self.trace_symbol(symbol))})
    }

    pub(crate) fn trace_type_sort(&self, event: &str, values: &[TypeId]) {
        if tsr_ast::creation_trace::active() {
            tsr_ast::creation_trace::record(json!({"event":event,"kind":"type",
                "operation":"sort_types", "values":values.iter().map(|id|self.trace_type(*id)).collect::<Vec<_>>()}));
        }
    }

    pub(crate) fn trace_symbol_sort(&self, event: &str, values: &[SymbolId]) {
        if tsr_ast::creation_trace::active() {
            tsr_ast::creation_trace::record(json!({"event":event,"kind":"symbol",
                "operation":"sort_symbols", "values":values.iter().map(|id|self.trace_symbol(*id)).collect::<Vec<_>>()}));
        }
    }
}
