mod baseline;
mod diagnostics;
mod graph;
mod merges;
mod queries;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_ast::{AstView, CompletedFile};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

#[derive(Debug)]
pub enum Error {
    Checker(tsr_checker::Error),
    Compiler(tsr_compiler::Error),
    Ast(tsr_arena::Error),
    Protocol(String),
    Unsupported(&'static str),
}
impl From<tsr_checker::Error> for Error {
    fn from(v: tsr_checker::Error) -> Self {
        Self::Checker(v)
    }
}
impl From<tsr_compiler::Error> for Error {
    fn from(v: tsr_compiler::Error) -> Self {
        Self::Compiler(v)
    }
}
impl From<tsr_arena::Error> for Error {
    fn from(v: tsr_arena::Error) -> Self {
        Self::Ast(v)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Checker(error) => write!(f, "checker: {error}"),
            Self::Compiler(error) => write!(f, "compiler: {error}"),
            Self::Ast(error) => write!(f, "AST: {error}"),
            Self::Protocol(message) => write!(f, "protocol: {message}"),
            Self::Unsupported(operation) => write!(f, "unsupported: {operation}"),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

pub fn text(value: &Value) -> Result<&str> {
    value
        .as_str()
        .ok_or_else(|| Error::Protocol("expected string".into()))
}
pub fn array(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| Error::Protocol("expected array".into()))
}
pub fn utf8(value: &[u8]) -> Result<&str> {
    std::str::from_utf8(value)
        .map_err(|_| Error::Unsupported("P2 diagnostic JSON argument requires valid UTF-8"))
}
pub fn hex(value: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(value.len() * 2);
    for &byte in value {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    output
}
pub fn panic_error(payload: &(dyn std::any::Any + Send)) -> Error {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload");
    Error::Protocol(format!("panic: {message}"))
}
pub fn failure(error: &Error, operation: &str) -> Value {
    let unsupported = match error {
        Error::Checker(tsr_checker::Error::Unsupported(name))
        | Error::Compiler(
            tsr_compiler::Error::Checker(tsr_checker::Error::Unsupported(name))
            | tsr_compiler::Error::Unsupported(name),
        )
        | Error::Unsupported(name) => Some(*name),
        _ => None,
    };
    match unsupported {
        Some(reason) => json!({"state":"unsupported","operation":operation,"reason":reason}),
        None => json!({"state":"failed","operation":operation,"reason":error.to_string()}),
    }
}
pub fn source(program: &Program, node: NodeId) -> Result<&CompletedFile> {
    program
        .files()
        .iter()
        .find(|f| f.source().arena() == node.arena())
        .map(|f| f.bound())
        .ok_or(Error::Ast(tsr_arena::Error::WrongOwner))
}
pub fn view(program: &Program, node: NodeId) -> Result<AstView<'_>> {
    Ok(source(program, node)?.view().ast())
}
pub fn file_name(program: &Program, node: NodeId) -> Result<&[u8]> {
    Ok(source(program, node)?
        .view()
        .source_file()?
        .parse_options()
        .file_name
        .as_bytes())
}
pub fn node_json(program: &Program, node: Option<NodeId>) -> Result<Value> {
    let Some(id) = node else {
        return Ok(Value::Null);
    };
    let n = view(program, id)?.node(id)?;
    Ok(
        json!({"file":utf8(file_name(program,id)?)?,"kind":n.kind().raw(),"pos":n.pos(),"end":n.end()}),
    )
}
pub fn load(
    files: &Value,
    roots: &Value,
    cache: &mut FileCache,
    counters: &Counters,
) -> Result<Arc<Program>> {
    load_with_libraries(
        files,
        roots,
        cache,
        counters,
        false,
        FixtureOptions::default(),
    )
}

#[derive(Clone, Copy, Default)]
pub struct FixtureOptions {
    unchecked: bool,
    isolated: bool,
    verbatim: bool,
    unreachable: Option<bool>,
    allow_js: bool,
    check_js: bool,
    implicit_override: bool,
    synthetic_defaults: Option<bool>,
    module_interop: Option<bool>,
    resolve_json: Option<bool>,
    declaration: bool,
    isolated_declarations: bool,
    strip_internal: bool,
    target: Option<ScriptTarget>,
    module: Option<ModuleKind>,
    use_define: Option<bool>,
    import_helpers: Option<bool>,
    no_emit: Option<bool>,
}

fn tristate(value: Option<bool>) -> Tristate {
    match value {
        Some(true) => Tristate::TRUE,
        Some(false) => Tristate::FALSE,
        None => Tristate::UNKNOWN,
    }
}

pub fn load_with_libraries(
    files: &Value,
    roots: &Value,
    cache: &mut FileCache,
    counters: &Counters,
    libraries: bool,
    overrides: FixtureOptions,
) -> Result<Arc<Program>> {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, source) in files
        .as_object()
        .ok_or_else(|| Error::Protocol("expected files".into()))?
    {
        fs.insert_loaded(name.as_bytes(), text(source)?.as_bytes());
    }
    let options = CompilerOptions {
        allow_synthetic_default_imports: tristate(overrides.synthetic_defaults),
        es_module_interop: tristate(overrides.module_interop),
        resolve_json_module: tristate(overrides.resolve_json),
        declaration: if overrides.declaration {
            Tristate::TRUE
        } else {
            Tristate::UNKNOWN
        },
        isolated_declarations: if overrides.isolated_declarations {
            Tristate::TRUE
        } else {
            Tristate::UNKNOWN
        },
        strip_internal: if overrides.strip_internal {
            Tristate::TRUE
        } else {
            Tristate::UNKNOWN
        },
        use_define_for_class_fields: tristate(overrides.use_define),
        import_helpers: tristate(overrides.import_helpers),
        no_emit: tristate(overrides.no_emit),
        target: overrides.target.unwrap_or(ScriptTarget::ESNEXT),
        module: overrides.module.unwrap_or(ModuleKind::ESNEXT),
        strict: Tristate::TRUE,
        allow_js: if overrides.allow_js {
            Tristate::TRUE
        } else {
            Tristate::UNKNOWN
        },
        no_implicit_override: if overrides.implicit_override {
            Tristate::TRUE
        } else {
            Tristate::UNKNOWN
        },
        check_js: if overrides.check_js {
            Tristate::TRUE
        } else {
            Tristate::UNKNOWN
        },
        allow_unreachable_code: match overrides.unreachable {
            Some(true) => Tristate::TRUE,
            Some(false) => Tristate::FALSE,
            None => Tristate::UNKNOWN,
        },
        no_unchecked_indexed_access: if overrides.unchecked {
            Tristate::TRUE
        } else {
            Tristate::UNKNOWN
        },
        isolated_modules: if overrides.isolated {
            Tristate::TRUE
        } else {
            Tristate::UNKNOWN
        },
        verbatim_module_syntax: if overrides.verbatim {
            Tristate::TRUE
        } else {
            Tristate::UNKNOWN
        },
        no_lib: if libraries {
            Tristate::FALSE
        } else {
            Tristate::TRUE
        },
        skip_lib_check: if libraries {
            Tristate::TRUE
        } else {
            Tristate::UNKNOWN
        },
        ..Default::default()
    };
    let config = tsr_tsoptions::ParsedCommandLine::new(
        options,
        array(roots)?
            .iter()
            .map(|name| Ok(JsString::from_bytes(text(name)?.as_bytes())))
            .collect::<Result<Vec<_>>>()?,
    );
    let host: Arc<dyn tsr_vfs::FileSystem> = if libraries {
        Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish())))
    } else {
        Arc::new(fs.finish())
    };
    Ok(Arc::new(Program::load(
        ProgramOptions {
            config,
            host,
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(if libraries {
                tsr_bundled::LIB_PATH
            } else {
                b"/no-default-lib"
            }),
            skip_module_resolution: false,
        },
        cache,
        counters,
    )?))
}
pub fn owner(
    program: Arc<Program>,
    generation: &Generation,
    counters: &Counters,
) -> Result<Arc<CheckerOwner>> {
    Ok(Arc::new(CheckerOwner::for_program(
        CheckerIdentity::new(generation.clone(), counters),
        counters,
        Arc::new(ProgramCheckerHost::new(program)),
    )?))
}

pub fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: p2_checker canonical-native-requests.json observations.json".into());
    }
    let raw = std::fs::read(&args[1])?;
    let spec: Value = serde_json::from_slice(&raw)?;
    // Match the native producer's ensure_ascii=True wire representation.
    // Serde's default formatter leaves non-ASCII text unescaped.
    let mut canonical = canonical_ascii_request(&spec)?;
    canonical.push(b'\n');
    // A canonical wire request rejects duplicate keys as well as trailing data;
    // the checked Python producer supplies this exact representation.
    if raw != canonical {
        return Err("P2 requires the canonical native request bytes".into());
    }
    let mut selected_options = spec["options"].clone();
    let target = match text(&spec["options"]["target"])? {
        "ES5" => ScriptTarget::ES5,
        "ES2015" => ScriptTarget::ES2015,
        "ES2016" => ScriptTarget::ES2016,
        "ES2017" => ScriptTarget::ES2017,
        "ES2018" => ScriptTarget::ES2018,
        "ES2019" => ScriptTarget::ES2019,
        "ES2020" => ScriptTarget::ES2020,
        "ES2021" => ScriptTarget::ES2021,
        "ES2022" => ScriptTarget::ES2022,
        "ES2023" => ScriptTarget::ES2023,
        "ES2024" => ScriptTarget::ES2024,
        "ES2025" => ScriptTarget::ES2025,
        "ESNext" => ScriptTarget::ESNEXT,
        _ => return Err("unknown target".into()),
    };
    let module = match text(&spec["options"]["module"])? {
        "None" => ModuleKind::NONE,
        "CommonJS" => ModuleKind::COMMON_JS,
        "AMD" => ModuleKind::AMD,
        "UMD" => ModuleKind::UMD,
        "System" => ModuleKind::SYSTEM,
        "ES2015" => ModuleKind::ES2015,
        "ES2020" => ModuleKind::ES2020,
        "ES2022" => ModuleKind::ES2022,
        "ESNext" => ModuleKind::ESNEXT,
        "Node16" => ModuleKind::NODE16,
        "Node18" => ModuleKind::NODE18,
        "Node20" => ModuleKind::NODE20,
        "NodeNext" => ModuleKind::NODE_NEXT,
        "Preserve" => ModuleKind::PRESERVE,
        _ => return Err("unknown module format".into()),
    };
    selected_options["target"] = json!("ESNext");
    selected_options["module"] = json!("ESNext");
    let mut option = |name: &str| -> std::result::Result<bool, Box<dyn std::error::Error>> {
        match selected_options
            .as_object_mut()
            .and_then(|options| options.remove(name))
        {
            Some(value) => value
                .as_bool()
                .ok_or_else(|| format!("{name} must be boolean").into()),
            None => Ok(false),
        }
    };
    let overrides = FixtureOptions {
        target: Some(target),
        module: Some(module),
        use_define: spec["options"]
            .get("useDefineForClassFields")
            .map(|_| option("useDefineForClassFields"))
            .transpose()?,
        import_helpers: spec["options"]
            .get("importHelpers")
            .map(|_| option("importHelpers"))
            .transpose()?,
        no_emit: spec["options"]
            .get("noEmit")
            .map(|_| option("noEmit"))
            .transpose()?,
        declaration: option("declaration")?,
        isolated_declarations: option("isolatedDeclarations")?,
        strip_internal: option("stripInternal")?,
        unchecked: option("noUncheckedIndexedAccess")?,
        isolated: option("isolatedModules")?,
        verbatim: option("verbatimModuleSyntax")?,
        allow_js: option("allowJs")?,
        check_js: option("checkJs")?,
        implicit_override: option("noImplicitOverride")?,
        synthetic_defaults: spec["options"]
            .get("allowSyntheticDefaultImports")
            .map(|_| option("allowSyntheticDefaultImports"))
            .transpose()?,
        module_interop: spec["options"]
            .get("esModuleInterop")
            .map(|_| option("esModuleInterop"))
            .transpose()?,
        resolve_json: spec["options"]
            .get("resolveJsonModule")
            .map(|_| option("resolveJsonModule"))
            .transpose()?,
        unreachable: match selected_options
            .as_object_mut()
            .and_then(|options| options.remove("allowUnreachableCode"))
        {
            Some(value) => Some(
                value
                    .as_bool()
                    .ok_or("allowUnreachableCode must be boolean")?,
            ),
            None => None,
        },
    };
    if spec["version"] != 1
        || (selected_options
            != json!({"target":"ESNext","module":"ESNext","strict":true,"noLib":true})
            && selected_options
                != json!({"target":"ESNext","module":"ESNext","strict":true,"noLib":false,"skipLibCheck":true}))
    {
        return Err("unsupported P2 protocol/options".into());
    }
    let program_diagnostics = match spec.get("diagnostic_mode") {
        None => false,
        Some(Value::String(mode)) if mode == "checker" => false,
        Some(Value::String(mode)) if mode == "program" => true,
        _ => return Err("invalid diagnostic mode".into()),
    };
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let mut programs = Vec::new();
    let mut ownership = Vec::new();
    for request in array(&spec["programs"])? {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            queries::program(
                request,
                &generation,
                &counters,
                spec["options"]["noLib"] == false,
                overrides,
                program_diagnostics,
            )
        }))
        .map_err(|payload| panic_error(payload.as_ref()))
        .and_then(|value| value);
        let mut row = match result {
            Ok((row, lifetime)) => {
                ownership.push(lifetime);
                row
            }
            Err(error) => {
                let mut lifetime = failure(
                    &Error::Unsupported("program operation did not yield a retained type"),
                    "retain_type",
                );
                lifetime["id"] = request["id"].clone();
                ownership.push(lifetime);
                failure(&error, "program")
            }
        };
        row["id"] = request["id"].clone();
        programs.push(row);
    }
    let mut modes = Vec::new();
    for mode in array(&spec["merge"]["modes"])? {
        let name = text(mode)?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            merges::mode(&spec["merge"], name, &generation, &counters)
        }))
        .map_err(|payload| panic_error(payload.as_ref()))
        .and_then(|value| value);
        let mut row = match result {
            Ok(row) => row,
            Err(error) => failure(&error, "checker_local_merge"),
        };
        row["mode"] = mode.clone();
        modes.push(row);
    }
    let out = json!({"version":1,"request_sha256":format!("{:x}",Sha256::digest(&raw)),"programs":programs,"merges":modes,
        "rust_ownership":{"scope":"Executed Rust-only retained-result lifetime checks; no native ownership equivalence claim","programs":ownership}});
    let mut bytes = serde_json::to_vec(&out)?;
    bytes.push(b'\n');
    std::fs::write(&args[2], bytes)?;
    Ok(())
}

fn canonical_ascii_request(value: &Value) -> serde_json::Result<Vec<u8>> {
    let serialized = serde_json::to_string(value)?;
    let mut bytes = Vec::with_capacity(serialized.len());
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for unit in serialized.encode_utf16() {
        if unit < 0x7f {
            bytes.push(unit as u8);
        } else {
            bytes.extend_from_slice(b"\\u");
            for shift in [12, 8, 4, 0] {
                bytes.push(HEX[usize::from((unit >> shift) & 15)]);
            }
        }
    }
    Ok(bytes)
}
