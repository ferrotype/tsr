//! The program as the declaration transformers' emit host where no checker
//! host is at hand: declaration diagnostics and the transform's probes. The
//! emitter's host is [`crate::EmitHost`]; both answer from the same program
//! operations. Files are named by the root node of their parsed source.
use crate::{output_paths, Program, ProgramFile};
use std::sync::OnceLock;
use tsr_ast::{FileReference, NodeId};
use tsr_jsstring::JsString;
use tsr_transformers::declarations::{DeclarationEmitHost, OutputPaths};
use tsr_tsoptions::output_paths::ForceEmitPaths;
use tsr_tspath as path;

pub struct ProgramDeclarationHost<'a> {
    program: &'a Program,
    common_source_directory: OnceLock<Result<Vec<u8>, tsr_arena::Error>>,
}

impl<'a> ProgramDeclarationHost<'a> {
    pub fn new(program: &'a Program) -> Self {
        Self {
            program,
            common_source_directory: OnceLock::new(),
        }
    }

    fn program_file(&self, file: NodeId) -> &'a ProgramFile {
        program_file(self.program, file)
    }

    /// `Program.CommonSourceDirectory`, computed once.
    fn common_source_directory(&self) -> Result<&[u8], tsr_arena::Error> {
        match self
            .common_source_directory
            .get_or_init(|| output_paths::common_source_directory(self.program))
        {
            Ok(directory) => Ok(directory),
            Err(error) => Err(*error),
        }
    }

    /// The declaration output path of `file` with declaration emit forced,
    /// as `emitDeclarationFile` and the probe compute it.
    pub fn declaration_file_path(&self, file: NodeId) -> Result<Vec<u8>, tsr_arena::Error> {
        let common = self.common_source_directory()?.to_vec();
        let paths = output_paths::output_paths_for(
            self.program_file(file),
            self.program,
            &common,
            ForceEmitPaths {
                dts: true,
                ..ForceEmitPaths::default()
            },
        )?;
        Ok(paths.declaration_file_path().to_vec())
    }
}

impl DeclarationEmitHost for ProgramDeclarationHost<'_> {
    fn get_current_directory(&self) -> &[u8] {
        self.program.current_directory()
    }

    fn use_case_sensitive_file_names(&self) -> bool {
        self.program.use_case_sensitive_file_names()
    }

    fn get_source_file_from_reference(
        &self,
        origin: NodeId,
        reference: &FileReference,
    ) -> Option<NodeId> {
        get_source_file_from_reference(self.program, self.program_file(origin), reference)
            .map(ProgramFile::source)
    }

    fn get_source_file(&self, file_name: &[u8]) -> Option<NodeId> {
        self.program.source_file(file_name).map(ProgramFile::source)
    }

    fn get_output_paths_for(&self, file: NodeId, force_dts_paths: bool) -> OutputPaths {
        let common = self
            .common_source_directory()
            .expect("the program's files are readable")
            .to_vec();
        output_paths::output_paths_for(
            self.program_file(file),
            self.program,
            &common,
            ForceEmitPaths {
                dts: force_dts_paths,
                ..ForceEmitPaths::default()
            },
        )
        .expect("the program's files are readable")
    }

    fn source_file_may_be_emitted(&self, file: NodeId, force_dts_emit: bool) -> bool {
        output_paths::may_emit_with_force_dts(self.program_file(file), self.program, force_dts_emit)
            .expect("the program's files are readable")
    }
}

/// The program file whose parsed source is `file`.
pub(crate) fn program_file(program: &Program, file: NodeId) -> &ProgramFile {
    program
        .files()
        .iter()
        .find(|candidate| candidate.source() == file)
        .expect("a declaration emit host file is one of the program's")
}

// port: tsc/internal/compiler/program.go:Program.GetSourceFileFromReference
pub(crate) fn get_source_file_from_reference<'a>(
    program: &'a Program,
    origin: &ProgramFile,
    reference: &FileReference,
) -> Option<&'a ProgramFile> {
    // TODO: The module loader in corsa is fairly different than strada, it should probably be able to expose this functionality at some point,
    // rather than redoing the logic approximately here, since most of the related logic now lives in module.Resolver
    // Still, without the failed lookup reporting that only the loader does, this isn't terribly complicated
    let origin_name = origin
        .bound()
        .view()
        .source_file()
        .expect("the program's files are readable")
        .parse_options()
        .file_name
        .clone();
    let file_name = path::resolve(
        &path::directory(origin_name.as_bytes()),
        &[reference.file_name.as_bytes()],
    );
    let content_mapper_extensions = program.content_mapper_extensions();
    let supported_extensions = tsr_tsoptions::supported_extensions_with_json(
        program.options(),
        &content_mapper_extensions,
    );
    let allow_non_ts_extensions = program.options().allow_non_ts_extensions.is_true();
    if path::has_extension(&file_name) {
        if !allow_non_ts_extensions {
            let canonical_file_name =
                path::canonical(&file_name, program.use_case_sensitive_file_names());
            let mut supported = false;
            for group in &supported_extensions {
                if path::file_extension_is_one_of(&canonical_file_name, group) {
                    supported = true;
                    break;
                }
            }
            if !supported {
                return None; // unsupported extensions are forced to fail
            }
        }

        return source_file_for_resolved_module(program, &file_name);
    }
    if allow_non_ts_extensions {
        if let Some(extensionless) = source_file_for_resolved_module(program, &file_name) {
            return Some(extensionless);
        }
    }

    // Only try adding extensions from the first supported group (which should be .ts/.tsx/.d.ts)
    for ext in &supported_extensions[0] {
        let mut candidate = file_name.clone();
        candidate.extend_from_slice(ext.as_bytes());
        if let Some(result) = source_file_for_resolved_module(program, &candidate) {
            return Some(result);
        }
    }
    None
}

/// `Program.GetSourceFileForResolvedModule`.
fn source_file_for_resolved_module<'a>(
    program: &'a Program,
    file_name: &[u8],
) -> Option<&'a ProgramFile> {
    program.source_file(file_name).or_else(|| {
        let output: JsString = program.parse_file_redirect(file_name)?;
        program.source_file(output.as_bytes())
    })
}
