//! Global workspace symbols load referenced projects; current-project scope does not.
use super::*;
struct QuietClient;
impl Client for QuietClient {
    fn notify(&self, _: &str, _: RawValue) -> Result<(), lsp::ResponseError> {
        Ok(())
    }
    fn request(
        &self,
        _: &Context,
        method: &str,
        _: RawValue,
    ) -> Result<RawValue, lsp::ResponseError> {
        Err(crate::error(
            -32603,
            format!("unexpected callback: {method}"),
        ))
    }
    fn request_without_waiting(&self, _: &str, _: RawValue) -> Result<(), lsp::ResponseError> {
        Ok(())
    }
}
fn fixture() -> (Runtime, Arc<dyn FileSystem>) {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in [
        (
            "/user/tsconfig.json",
            r#"{"compilerOptions":{"noLib":true},"files":["main.ts"],"references":[{"path":"../a"},{"path":"../b"}]}"#,
        ),
        ("/user/main.ts", "import { fnA } from '../a/a'; fnA();"),
        (
            "/a/tsconfig.json",
            r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["a.ts"]}"#,
        ),
        ("/a/a.ts", "export function fnA() {}"),
        (
            "/b/tsconfig.json",
            r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["b.ts","c.ts"]}"#,
        ),
        ("/b/b.ts", "export function fnB() {}"),
        ("/b/c.ts", "export function fnC() {}"),
    ] {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    let host: Arc<dyn FileSystem> = Arc::new(fs.finish());
    let session = Session::new(
        SessionOptions::default(),
        host.clone(),
        &tsr_arena::Counters::new(),
    );
    session
        .did_open_file(
            lsp::DocumentUri("file:///user/main.ts".into()),
            1,
            JsString::from_bytes(b"import { fnA } from '../a/a'; fnA();".as_slice()),
            lsp::LanguageKind("typescript".into()),
        )
        .unwrap();
    let mut runtime = Runtime::new(
        Options::new(SessionOptions::default(), host.clone()),
        Context::background(),
        Arc::new(QuietClient),
        Box::new(std::io::sink()),
    );
    runtime.server = Some(Server::new(session));
    runtime.initialize = Some(lsp::InitializeParams::default());
    runtime.initialized = true;
    (runtime, host)
}
fn names(runtime: &mut Runtime, host: Arc<dyn FileSystem>, current: bool) -> Vec<String> {
    runtime.settings.lock().unwrap().workspace_current_project = current;
    let request = lsp::Message {
        id: Some(lsp::Id::int(1)),
        method: "workspace/symbol".into(),
        params: Some(RawValue(
            br#"{"query":"fn","textDocument":{"uri":"file:///user/main.ts"}}"#.to_vec(),
        )),
        ..Default::default()
    };
    let Dispatch::Work(work) = runtime
        .prepare(&Context::background(), &request, host)
        .unwrap()
    else {
        panic!("workspace symbols must produce checker work")
    };
    let value = work().unwrap();
    let mut response = lsp::SymbolInformationsOrWorkspaceSymbolsOrNull::default();
    tsr_json::unmarshal(&value.0, &mut response, tsr_json::Options::default()).unwrap();
    response
        .symbol_informations
        .unwrap()
        .into_iter()
        .flatten()
        .map(|symbol| symbol.name)
        .collect()
}
#[test]
fn global_workspace_symbols_load_unopened_reference_files_while_current_scope_does_not() {
    let (mut runtime, host) = fixture();
    let current = names(&mut runtime, host.clone(), true);
    assert!(current.contains(&"fnA".into()));
    assert!(!current.contains(&"fnC".into()));
    assert!(runtime
        .server()
        .unwrap()
        .session()
        .snapshot()
        .unwrap()
        .project_by_path(b"/b/tsconfig.json")
        .is_none());
    let global = names(&mut runtime, host, false);
    assert!(
        global.contains(&"fnC".into()),
        "global symbols must include the unopened referenced file: {global:?}"
    );
    assert!(runtime
        .server()
        .unwrap()
        .session()
        .snapshot()
        .unwrap()
        .project_by_path(b"/b/tsconfig.json")
        .is_some());
}
