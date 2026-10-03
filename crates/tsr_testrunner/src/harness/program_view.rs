//! The baseline writers' [`ProgramView`] over a loaded Rust [`Program`]: the
//! source files' names, texts and mappers in program order, `GetSourceFile`
//! through the program's path lookup, and the common source directory the
//! checker host computes (`Program.CommonSourceDirectory`).
use super::baselines::{Failure, ProgramView, SourceFileView};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tsr_compiler::{Program, ProgramCheckerHost};
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;

/// One source file's facts, retained from the program's files (the texts
/// share the program's backing).
struct FileFacts {
    file_name: JsString,
    path: JsString,
    text: tsr_jsstring::SourceText,
    original_text: Option<tsr_jsstring::SourceText>,
    content_mapper: JsString,
}

pub struct ProgramFacts {
    program: Arc<Program>,
    files: Vec<FileFacts>,
    by_path: HashMap<Vec<u8>, usize>,
    common_source_directory: OnceLock<Result<Vec<u8>, String>>,
}

impl ProgramFacts {
    pub fn new(program: Arc<Program>) -> Result<Self, Failure> {
        let mut files = Vec::with_capacity(program.files().len());
        let mut by_path = HashMap::new();
        for file in program.files() {
            let view = file.bound().view();
            let source = view
                .source_file()
                .map_err(|error| Failure::Input(format!("program source file: {error:?}")))?;
            let original_text = source
                .content_mapper_info()
                .filter(|_| !source.content_mapper().is_empty())
                .map(|info| info.original_text.clone());
            by_path.insert(source.path().to_vec(), files.len());
            files.push(FileFacts {
                file_name: source.parse_options().file_name.clone(),
                path: source.parse_options().path.clone(),
                text: source.text().clone(),
                original_text,
                content_mapper: JsString::from_bytes(source.content_mapper()),
            });
        }
        Ok(Self {
            program,
            files,
            by_path,
            common_source_directory: OnceLock::new(),
        })
    }

    fn view(facts: &FileFacts) -> SourceFileView<'_> {
        SourceFileView {
            file_name: facts.file_name.as_bytes(),
            path: facts.path.as_bytes(),
            text: facts.text.as_bytes(),
            original_text: facts
                .original_text
                .as_ref()
                .unwrap_or(&facts.text)
                .as_bytes(),
            content_mapper: facts.content_mapper.as_bytes(),
        }
    }
}

impl ProgramView for ProgramFacts {
    fn source_files(&self) -> Vec<SourceFileView<'_>> {
        self.files.iter().map(Self::view).collect()
    }
    fn source_file(&self, file_name: &[u8]) -> Option<SourceFileView<'_>> {
        let file = self.program.source_file(file_name)?;
        let source = file.bound().view().source_file().ok()?;
        self.by_path
            .get(source.path())
            .map(|&index| Self::view(&self.files[index]))
    }
    fn options(&self) -> &CompilerOptions {
        self.program.options()
    }
    fn common_source_directory(&self) -> Result<Vec<u8>, Failure> {
        self.common_source_directory
            .get_or_init(|| {
                let host = ProgramCheckerHost::new(self.program.clone());
                tsr_checker::CheckerHost::common_source_directory(&host)
                    .map(<[u8]>::to_vec)
                    .map_err(|error| format!("{error:?}"))
            })
            .clone()
            .map_err(|error| Failure::Input(format!("common source directory: {error}")))
    }
    fn current_directory(&self) -> &[u8] {
        self.program.current_directory()
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        self.program.use_case_sensitive_file_names()
    }
    fn content_mapper_extensions(&self) -> Vec<JsString> {
        self.program.content_mapper_extensions()
    }
}
