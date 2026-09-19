use serde_json::{json, Value};
fn decode(value: &Value) -> Vec<u8> {
    value
        .as_str()
        .unwrap()
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn hex(value: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(value.len() * 2);
    for byte in value {
        write!(result, "{byte:02x}").expect("writing to a String is infallible");
    }
    result
}
fn text(callback: impl FnOnce() -> Vec<u8> + std::panic::UnwindSafe) -> Value {
    match std::panic::catch_unwind(callback) {
        Ok(bytes) => json!({"text":hex(&bytes)}),
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap();
            assert_eq!(
                message,
                "paths must either both be absolute or both be relative"
            );
            json!({"panic":message})
        }
    }
}
#[test]
fn path_comparison_folding_ranges_and_failures_match_pinned_go() {
    use ts_tspath as p;
    let requests: Vec<Value> =
        serde_json::from_str(include_str!("../../../data/s07/path-requests.json")).unwrap();
    let expected: Vec<Value> =
        serde_json::from_str(include_str!("../../../data/s07/path-observations.json")).unwrap();
    assert_eq!(requests.len(), 4096);
    assert_eq!(requests.len(), expected.len());
    for (request, expected) in requests.iter().zip(expected) {
        let a = decode(&request["a"]);
        let b = decode(&request["b"]);
        let cwd = decode(&request["cwd"]);
        let sensitive = request["sensitive"].as_bool().unwrap();
        let trim = p::trim_file_path_prefix(&a, &b, sensitive);
        let parts = p::normalized_components(&a, &cwd);
        let actual = json!({"id":request["id"],"fold":p::equal_fold(&a,&b),"relative":p::is_relative(&a),"compare":p::compare_paths(&a,&b,&cwd,sensitive) as i32,"contains":p::contains_path(&a,&b,&cwd,sensitive),"trim":[hex(trim.unwrap_or(&a)),trim.is_some()],"components":parts.iter().map(|part|hex(part)).collect::<Vec<_>>(),"reconstruct":hex(&p::path_from_components(&parts)),"directory":text(||p::relative_from_directory(&a,&b,&cwd,sensitive)),"file":text(||p::relative_from_file(&a,&b,&cwd,sensitive))});
        assert_eq!(actual, expected, "request {request}");
    }
}
