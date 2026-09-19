use super::{graph, protocol::Session};
use serde_json::{json, Value};
use ts_ast::{AstView, BindResult, NodeId};

pub fn dump(
    s: &Session,
    stage: &str,
    view: AstView<'_>,
    source: NodeId,
    result: Option<&BindResult>,
) {
    for (kind, value) in graph::collect(view, source, result) {
        observe(s, stage, kind, value);
    }
}
fn observe(s: &Session, stage: &str, kind: &str, value: Value) {
    const CHUNK: usize = 128 * 1024;
    let raw = match serde_json::to_vec(&value) {
        Ok(raw) => raw,
        Err(error) => {
            s.failure(format!("S07 graph serialization: {error}"));
            return;
        }
    };
    if raw.len() <= CHUNK {
        s.observe(stage, kind, value);
        return;
    }
    let parts = raw.len().div_ceil(CHUNK);
    for (part, bytes) in raw.chunks(CHUNK).enumerate() {
        s.observe(
            stage,
            "fragment",
            json!({"record_kind":kind,"part":part,"parts":parts,"payload_hex":graph::hex(bytes)}),
        );
    }
}
