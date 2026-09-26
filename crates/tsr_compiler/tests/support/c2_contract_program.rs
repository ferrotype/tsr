//! C1 production-program helpers shared by the C2 direct contracts.
#![allow(dead_code)]
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_ast::SyntaxKind as K;
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

pub fn program(files: &[(&str, &str)]) -> Arc<Program> {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in files {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    let roots = files
        .iter()
        .map(|(name, _)| JsString::from_bytes(name.as_bytes()))
        .collect();
    let options = CompilerOptions {
        target: ScriptTarget::ESNEXT,
        module: ModuleKind::ESNEXT,
        strict: Tristate::TRUE,
        ..Default::default()
    };
    Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(options, roots),
                host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish()))),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            &Counters::new(),
        )
        .unwrap(),
    )
}

pub fn checker(program: &Arc<Program>) -> (Counters, Generation, Arc<CheckerOwner>) {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let owner = Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(generation.clone(), &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    );
    (counters, generation, owner)
}

/// The `VariableDeclaration` node named `name` in `file`.
pub fn declaration(program: &Program, file: &str, name: &str) -> NodeId {
    let file = program.file(file.as_bytes()).unwrap();
    let view = file.bound().view().ast();
    for statement in view
        .node_slice(view.node(file.source()).unwrap().statements(view).unwrap())
        .unwrap()
        .iter()
        .flatten()
    {
        let read = view.node(statement).unwrap();
        if read.kind() != K::VariableStatement {
            continue;
        }
        let list = read
            .as_variable_statement()
            .unwrap()
            .declaration_list()
            .unwrap();
        let list = view
            .node(list)
            .unwrap()
            .as_variable_declaration_list()
            .unwrap()
            .declarations()
            .unwrap();
        for declaration in view
            .node_slice(view.list(list).unwrap().nodes())
            .unwrap()
            .iter()
            .flatten()
        {
            let read = view.node(declaration).unwrap();
            if view.node_text(read.name().unwrap()).unwrap().as_bytes() == name.as_bytes() {
                return declaration;
            }
        }
    }
    panic!("missing declaration {name}");
}

pub fn name_of(program: &Program, file: &str, name: &str) -> NodeId {
    let declaration = declaration(program, file, name);
    let file = program.file(file.as_bytes()).unwrap();
    let view = file.bound().view().ast();
    view.node(declaration).unwrap().name().unwrap()
}

pub fn codes(diagnostics: &[tsr_ast::Diagnostic]) -> Vec<i32> {
    let mut codes: Vec<i32> = diagnostics.iter().map(|d| d.code).collect();
    codes.sort_unstable();
    codes
}
