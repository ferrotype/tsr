use super::*;
use serde_json::{json, Value};

fn send(session: &mut Session, value: &Value) -> Vec<Value> {
    let text = value.to_string();
    session
        .receive(&crate::framing::parse_json(text.as_bytes()).unwrap())
        .unwrap()
        .iter()
        .map(|r| serde_json::from_str(r.get()).unwrap())
        .collect()
}
fn init(session: &mut Session) {
    assert!(send(session,&json!({"jsonrpc":"2.0","id":0,"method":"test/initialize","params":{
        "version":2,"caseSensitive":true,"base":{},"symlinks":{},"callbacks":["readFile"],"options":{},"plugins":[{"name":"mapper","options":{}}]
    }})).is_empty());
}
fn commit(session: &mut Session) {
    let token = session.pending_options().unwrap().token;
    session.complete_options(token, Ok(())).unwrap();
}
fn request(session: &mut Session, id: Value, method: &str, params: Value) -> Vec<Value> {
    send(
        session,
        &Value::Object(serde_json::Map::from_iter([
            ("jsonrpc".into(), json!("2.0")),
            ("id".into(), id),
            ("method".into(), json!(method)),
            ("params".into(), params),
        ])),
    )
}
fn cancel(session: &mut Session, id: Value) -> Vec<Value> {
    send(
        session,
        &Value::Object(serde_json::Map::from_iter([
            ("jsonrpc".into(), json!("2.0")),
            ("method".into(), json!("$/cancelRequest")),
            (
                "params".into(),
                Value::Object(serde_json::Map::from_iter([("id".into(), id)])),
            ),
        ])),
    )
}
#[test]
fn internal_options_barrier_blocks_new_work_and_publishes_only_at_completion() {
    let mut session = Session::default();
    init(&mut session);
    assert_eq!(
        request(&mut session, json!(1), "test/state", json!({}))[0]["error"]["code"],
        -32002
    );
    commit(&mut session);
    assert!(request(
        &mut session,
        json!("options"),
        "test/setOptions",
        json!({"options":{"strict":true}})
    )
    .is_empty());
    for (method, params) in [
        ("test/fs", json!({"operation":"readFile","path":"/x"})),
        ("test/openPlugin", json!({"name":"mapper"})),
        ("test/streamWrite", json!({"stream":"x","data":"YQ=="})),
    ] {
        assert_eq!(
            request(&mut session, json!(2), method, params)[0]["error"]["code"],
            -32002
        );
    }
    assert_eq!(
        request(&mut session, json!(2), "test/state", json!({}))[0]["result"]["options"],
        json!({})
    );
    commit(&mut session);
    assert_eq!(
        request(&mut session, json!(2), "test/state", json!({}))[0]["result"]["options"],
        json!({"strict":true})
    );
}
#[test]
fn canceled_options_hold_barrier_until_unapplied_or_committed_outcome() {
    for applied in [false, true] {
        let mut session = Session::default();
        init(&mut session);
        commit(&mut session);
        request(
            &mut session,
            json!(-1),
            "test/setOptions",
            json!({"options":{"strict":true}}),
        );
        let token = session.pending_options().unwrap().token;
        assert!(cancel(&mut session, json!(-1)).is_empty());
        assert!(session.pending_options().unwrap().cancellation_requested);
        assert_eq!(
            request(
                &mut session,
                json!(2),
                "test/fs",
                json!({"operation":"readFile","path":"/x"})
            )[0]["error"]["code"],
            -32002
        );
        let outcome = if applied {
            Ok(())
        } else {
            Err("aborted before application".into())
        };
        let messages = session.complete_options(token, outcome).unwrap();
        let message: Value = serde_json::from_str(messages[0].get()).unwrap();
        if applied {
            assert_eq!(message["result"], json!({"options":{"strict":true}}));
        } else {
            assert_eq!(message["error"]["code"], -32800);
        }
        let state = request(&mut session, json!(-1), "test/state", json!({}));
        assert_eq!(
            state[0]["result"]["options"],
            if applied {
                json!({"strict":true})
            } else {
                json!({})
            }
        );
        assert!(session.complete_options(token, Ok(())).is_err());
    }
}
#[test]
fn failed_initialization_retries_and_tokens_do_not_alias_across_sessions_or_shutdown() {
    let mut first = Session::default();
    init(&mut first);
    let old = first.pending_options().unwrap().token;
    let mut second = Session::default();
    init(&mut second);
    assert!(second.complete_options(old, Ok(())).is_err());
    cancel(&mut first, json!(0));
    first
        .complete_options(old, Err("not applied".into()))
        .unwrap();
    init(&mut first);
    let current = first.pending_options().unwrap().token;
    assert!(first.complete_options(old, Ok(())).is_err());
    request(&mut first, json!(1), "test/shutdown", json!({}));
    assert!(first.complete_options(current, Ok(())).is_err());
    commit(&mut second);
}
#[test]
fn canceled_callback_cannot_complete_reused_id_or_conflict_with_directional_ids() {
    let mut session = Session::default();
    init(&mut session);
    commit(&mut session);
    let params = json!({"operation":"readFile","path":"/x"});
    let first = request(&mut session, json!("callback:1"), "test/fs", params.clone());
    assert_eq!(first[1]["id"], "callback:1");
    cancel(&mut session, json!("callback:1"));
    let second = request(&mut session, json!("callback:1"), "test/fs", params);
    assert_eq!(second[1]["id"], "callback:2");
    assert!(send(
        &mut session,
        &json!({"jsonrpc":"2.0","id":"callback:1","result":{"content":"late"}})
    )
    .is_empty());
    let done = send(
        &mut session,
        &json!({"jsonrpc":"2.0","id":"callback:2","result":{"content":"new"}}),
    );
    assert_eq!(done[1]["id"], "callback:1");
    assert_eq!(done[1]["result"]["content"], "new");
}
#[test]
fn exact_opaque_numbers_survive_internal_options_commit() {
    let mut session = Session::default();
    init(&mut session);
    commit(&mut session);
    let message=crate::framing::parse_json(br#"{"jsonrpc":"2.0","id":"","method":"test/setOptions","params":{"options":{"n":1e400,"m":-0,"d":0.1234567890123456789}}}"#).unwrap();
    session.receive(&message).unwrap();
    let token = session.pending_options().unwrap().token;
    let output = session.complete_options(token, Ok(())).unwrap();
    assert!(output[0]
        .get()
        .contains(r#"{"n":1e400,"m":-0,"d":0.1234567890123456789}"#));
}

#[test]
fn non_lsp_ids_and_duplicate_live_ids_are_terminal() {
    for id in [
        "null",
        "true",
        "1.5",
        "2147483648",
        "-2147483649",
        "{}",
        "[]",
    ] {
        let mut session = Session::default();
        let text = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"test/state","params":{{}}}}"#);
        assert!(
            session
                .receive(&crate::framing::parse_json(text.as_bytes()).unwrap())
                .is_err(),
            "{id}"
        );
        assert!(session.is_closed());
    }
    let mut session = Session::default();
    init(&mut session);
    let token = session.pending_options().unwrap().token;
    let message = crate::framing::parse_json(
        br#"{"jsonrpc":"2.0","id":0,"method":"test/state","params":{}}"#,
    )
    .unwrap();
    assert!(session.receive(&message).is_err());
    assert!(session.is_closed());
    assert!(session.complete_options(token, Ok(())).is_err());
}
