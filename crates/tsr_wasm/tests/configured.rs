//! Native controls for the production configured/retained session surface.
use serde_json::{json, Value};
use tsr_wasm::MemoryHost;

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::from_value(value.clone()).unwrap()
}

#[test]
fn configured_check_and_emit_retain_owned_observations() {
    let mut host = MemoryHost::new(b"/project", true);
    host.add_directory(b"/project");
    host.add_file(
        b"/project/base.json",
        br#"{"compilerOptions":{"strict":true,"declaration":true,"rootDir":"src","outDir":"out"}}"#,
        false,
    );
    host.add_file(
        b"/project/tsconfig.json",
        br#"{"extends":"./base.json","include":["src/**/*.ts"]}"#,
        false,
    );
    host.add_file(
        b"/project/src/main.ts",
        b"export const answer: number = 42;\n",
        false,
    );
    let prepared = host.prepare();
    let mut session = prepared
        .compile_config(b"/project/tsconfig.json", "[]")
        .unwrap();
    session.check().unwrap();
    session.emit_retained("{}").unwrap();
    let owned = session.retained_snapshot().unwrap();
    let snapshot: Value = serde_json::from_slice(&owned).unwrap();
    assert_eq!(snapshot["outcome"], "compiled");
    assert_eq!(snapshot["checkers"], "1");
    assert_ne!(snapshot["types"], "0");
    assert_eq!(snapshot["diagnostics"], json!([]));
    assert_eq!(snapshot["emit_diagnostics"], json!([]));
    assert_eq!(snapshot["emit_skipped"], false);
    assert!(snapshot["sources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source["default_library"] == true));
    let convenience: Value = serde_json::from_slice(&session.emit("{}").unwrap()).unwrap();
    let writes = snapshot["writes"].as_array().unwrap();
    assert_eq!(writes.len(), 2);
    assert_eq!(
        writes
            .iter()
            .map(|write| bytes(&write["name"]))
            .collect::<Vec<_>>(),
        vec![
            b"/project/out/main.js".to_vec(),
            b"/project/out/main.d.ts".to_vec()
        ]
    );
    let observations = |files: &[Value]| {
        files
            .iter()
            .map(|file| (file["name"].clone(), file["text"].clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        observations(writes),
        observations(convenience["files"].as_array().unwrap())
    );
    drop(session);
    assert_eq!(serde_json::from_slice::<Value>(&owned).unwrap(), snapshot);
}

#[test]
fn retained_diagnostics_include_full_message_values_and_nested_sources() {
    let mut host = MemoryHost::new(b"/p", true);
    host.add_file(
        b"/p/tsconfig.json",
        br#"{"compilerOptions":{"strict":true},"files":["main.ts"]}"#,
        false,
    );
    host.add_file(b"/p/main.ts", b"const value: string = 1;\n", false);
    let mut session = host
        .prepare()
        .compile_config(b"/p/tsconfig.json", "[]")
        .unwrap();
    session.check().unwrap();
    let snapshot: Value = serde_json::from_slice(&session.retained_snapshot().unwrap()).unwrap();
    let diagnostic = snapshot["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == 2322)
        .unwrap();
    assert!(!bytes(&diagnostic["message_key"]).is_empty());
    assert!(!bytes(&diagnostic["localized"]).is_empty());
    let index = diagnostic["file"].as_u64().unwrap() as usize;
    assert_eq!(bytes(&snapshot["sources"][index]["name"]), b"/p/main.ts");
    let convenience: Value = serde_json::from_slice(&session.diagnostics().unwrap()).unwrap();
    assert_eq!(
        snapshot["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["code"].clone())
            .collect::<Vec<_>>(),
        convenience
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["code"].clone())
            .collect::<Vec<_>>()
    );
}
