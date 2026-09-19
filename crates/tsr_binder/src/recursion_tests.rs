use crate::{recursion::take_observations, Binder};
use ts_ast::{AstFile, NodeId, SourceFileParseOptions};
use ts_core::ScriptKind;
use ts_jsstring::SourceText;

fn parse(bytes: Vec<u8>, kind: ScriptKind) -> (AstFile, NodeId) {
    let parsed = ts_parser::parse_source_file(
        SourceText::from_loaded_bytes(bytes),
        kind,
        SourceFileParseOptions {
            file_name: ts_ast::JsString::from_bytes(b"/depth.ts".as_slice()),
            ..Default::default()
        },
    );
    let source = parsed.root();
    (parsed.publish_unbound(), source)
}
fn inventory() -> serde_json::Value {
    serde_json::from_str(include_str!("../../../data/s07/binder-depth-cases.json")).unwrap()
}
fn bytes(case: &serde_json::Value) -> Vec<u8> {
    case["source_hex"]
        .as_str()
        .unwrap()
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(text, 16).unwrap()
        })
        .collect()
}

#[test]
fn binder_depth_small_stack_growth_and_binary_continuations() {
    let inventory = inventory();
    let cases = inventory["small_stack"].as_array().unwrap();
    let mut report = Vec::new();
    for case in cases {
        let name = case["id"].as_str().unwrap();
        let kind = ScriptKind(case["script_kind"].as_i64().unwrap() as i32);
        let growth = case["require_growth"].as_bool().unwrap();
        let binary = case["require_binary"].as_bool().unwrap();
        let (file, source) = parse(bytes(case), kind);
        let observation = std::thread::Builder::new()
            .name(format!("binder-small-{name}"))
            .stack_size(512 * 1024)
            .spawn(move || {
                take_observations();
                crate::bind_source_file_worker(&file, source).unwrap();
                let bound = file.retain_bound(source).unwrap();
                assert_eq!(
                    bound.view().node(source).unwrap().kind(),
                    ts_ast::SyntaxKind::SourceFile
                );
                take_observations()
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(observation.entries > 500, "{name}: {observation:?}");
        if growth {
            assert!(observation.growths > 0, "{name}: {observation:?}");
        }
        if binary {
            assert!(
                observation.binary_nodes >= 20_000,
                "{name}: {observation:?}"
            );
            assert!(
                observation.binary_frames > 20_000,
                "{name}: {observation:?}"
            );
        }
        report.push(serde_json::json!({"id":name,"guard_entries":observation.entries,"actual_segment_growths":observation.growths,"binary_nodes":observation.binary_nodes,"max_binary_frames":observation.binary_frames}));
    }
    println!(
        "S07_BINDER_DEPTH:{}",
        serde_json::to_string(&report).unwrap()
    );
}

#[test]
fn binder_depth_combines_shared_flow_tail_with_real_stack_growth() {
    let (file, source) = parse(b"x;".to_vec(), ScriptKind::TS);
    let observation = std::thread::Builder::new()
        .name("binder-flow-list-small".to_owned())
        .stack_size(512 * 1024)
        .spawn(move || {
            let mut observed = None;
            file.bind_with(source, |builder| {
                let mut binder = Binder::new(builder);
                let flow = binder.new_flow_node(ts_ast::flow_flags::START);
                let tail = binder.new_flow_list(Some(flow), None);
                let mut head = None;
                for index in 0..20_000 {
                    head = Some(binder.new_flow_list((index % 2 == 0).then_some(flow), head));
                }
                take_observations();
                let combined = binder.combine_flow_lists(head, Some(tail));
                observed = Some(take_observations());
                let mut original = head;
                let mut copy = combined;
                while let Some(source) = original {
                    assert_ne!(copy, Some(source));
                    let before = binder.flow_list(source);
                    let after = binder.flow_list(copy.unwrap());
                    assert_eq!(before.flow, after.flow);
                    original = before.next;
                    copy = after.next;
                }
                assert_eq!(copy, Some(tail));
                Ok(())
            })
            .unwrap();
            observed.unwrap()
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(observation.entries >= 20_001, "{observation:?}");
    assert!(observation.growths > 0, "{observation:?}");
    println!(
        "S07_BINDER_DEPTH:{}",
        serde_json::json!([{"id":"shared-flow-list","guard_entries":observation.entries,"actual_segment_growths":observation.growths}])
    );
}

#[test]
fn binder_depth_growth_unwind_publishes_terminal_failure() {
    let inventory = inventory();
    let case = inventory["small_stack"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "functions")
        .unwrap();
    let (file, source) = parse(bytes(case), ScriptKind::TS);
    let observation = std::thread::Builder::new()
        .name("binder-growth-unwind".to_owned())
        .stack_size(512 * 1024)
        .spawn(move || {
            take_observations();
            crate::recursion::panic_after_next_growth();
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::bind_source_file_worker(&file, source).unwrap();
            }))
            .expect_err("fault must unwind through the grown segment");
            assert_eq!(
                panic.downcast_ref::<&str>().copied(),
                Some("S07 injected binder failure after stack growth")
            );
            let observed = take_observations();
            assert!(observed.growths > 0);
            assert!(matches!(
                file.retain_bound(source),
                Err(ts_ast::BindError::Failed)
            ));
            assert!(matches!(
                file.bind_with(source, |_| panic!("terminal failure must not retry")),
                Err(ts_ast::BindError::Failed)
            ));
            observed
        })
        .unwrap()
        .join()
        .unwrap();
    println!(
        "S07_BINDER_DEPTH:{}",
        serde_json::json!([{"id":"growth-unwind","guard_entries":observation.entries,"actual_segment_growths":observation.growths,"terminal_failure":true}])
    );
}
