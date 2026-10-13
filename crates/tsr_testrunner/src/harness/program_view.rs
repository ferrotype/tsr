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
pub struct FileFacts {
    pub file_name: JsString,
    pub path: JsString,
    pub text: tsr_jsstring::SourceText,
    pub original_text: Option<tsr_jsstring::SourceText>,
    pub content_mapper: JsString,
}

/// Owned observations returned by a compiler backend. Lookup keys are the
/// compiler's canonical paths, including aliases, rather than paths inferred
/// from the source-file names by the baseline writer.
pub struct ProgramSnapshot {
    pub files: Vec<FileFacts>,
    pub lookups: Vec<(Vec<u8>, usize)>,
    pub options: CompilerOptions,
    pub current_directory: Vec<u8>,
    pub case_sensitive: bool,
    pub common_source_directory: Result<Vec<u8>, String>,
    pub content_mapper_extensions: Vec<JsString>,
}

enum Backing {
    Native(Arc<Program>),
    Snapshot {
        options: Box<CompilerOptions>,
        current_directory: Vec<u8>,
        case_sensitive: bool,
        content_mapper_extensions: Vec<JsString>,
    },
}

pub struct ProgramFacts {
    backing: Backing,
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
            backing: Backing::Native(program),
            files,
            by_path,
            common_source_directory: OnceLock::new(),
        })
    }

    /// Restore observations only; this neither parses nor checks source text.
    pub fn from_snapshot(snapshot: ProgramSnapshot) -> Result<Self, Failure> {
        let mut by_path = HashMap::with_capacity(snapshot.lookups.len());
        for (path, index) in snapshot.lookups {
            if index >= snapshot.files.len() {
                return Err(Failure::Input(
                    "program lookup has a foreign file index".into(),
                ));
            }
            if by_path.insert(path, index).is_some() {
                return Err(Failure::Input(
                    "program lookup repeats a canonical path".into(),
                ));
            }
        }
        Ok(Self {
            backing: Backing::Snapshot {
                options: Box::new(snapshot.options),
                current_directory: snapshot.current_directory,
                case_sensitive: snapshot.case_sensitive,
                content_mapper_extensions: snapshot.content_mapper_extensions,
            },
            files: snapshot.files,
            by_path,
            common_source_directory: OnceLock::from(snapshot.common_source_directory),
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
        let path = match &self.backing {
            Backing::Native(program) => {
                let file = program.source_file(file_name)?;
                let source = file.bound().view().source_file().ok()?;
                source.parse_options().path.clone()
            }
            Backing::Snapshot {
                current_directory,
                case_sensitive,
                ..
            } => tsr_tspath::to_path(file_name, current_directory, *case_sensitive),
        };
        self.by_path
            .get(path.as_bytes())
            .map(|&index| Self::view(&self.files[index]))
    }
    fn options(&self) -> &CompilerOptions {
        match &self.backing {
            Backing::Native(program) => program.options(),
            Backing::Snapshot { options, .. } => options,
        }
    }
    fn common_source_directory(&self) -> Result<Vec<u8>, Failure> {
        self.common_source_directory
            .get_or_init(|| {
                let Backing::Native(program) = &self.backing else {
                    // Snapshot construction initializes this cell, including
                    // a backend refusal to compute the directory.
                    return Err("missing observed common source directory".into());
                };
                let host = ProgramCheckerHost::new(program.clone());
                tsr_checker::CheckerHost::common_source_directory(&host)
                    .map(<[u8]>::to_vec)
                    .map_err(|error| format!("{error:?}"))
            })
            .clone()
            .map_err(|error| Failure::Input(format!("common source directory: {error}")))
    }
    fn current_directory(&self) -> &[u8] {
        match &self.backing {
            Backing::Native(program) => program.current_directory(),
            Backing::Snapshot {
                current_directory, ..
            } => current_directory,
        }
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        match &self.backing {
            Backing::Native(program) => program.use_case_sensitive_file_names(),
            Backing::Snapshot { case_sensitive, .. } => *case_sensitive,
        }
    }
    fn content_mapper_extensions(&self) -> Vec<JsString> {
        match &self.backing {
            Backing::Native(program) => program.content_mapper_extensions(),
            Backing::Snapshot {
                content_mapper_extensions,
                ..
            } => content_mapper_extensions.clone(),
        }
    }
}
