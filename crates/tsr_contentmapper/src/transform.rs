//! Turning a mapper's result into parsed source files (`transform.go`).
use crate::host::{transform_error, Error, Project, Request, TransformErrorKind, TransformResult};
use crate::mapper::{identity, is_supported_virtual_extension};
use tsr_arena::Counters;
use tsr_ast::{ContentMapperSourceFileInfo, ParsedFile, SourceFileParseOptions};
use tsr_core::ScriptKind;
use tsr_jsstring::{JsString, SourceText};
use tsr_tsoptions::config_mappers::ContentMapper;

/// A mapped file's canonical output and its unnamed supplemental inputs,
/// parsed and not yet bound.
pub struct SourceFiles {
    pub canonical: ParsedFile,
    pub supplemental: Vec<ParsedFile>,
}

/// Runs `mapper`'s transform for one content-mapped file and parses the
/// result, keeping the original name and text on the source file.
/// port: tsc/internal/contentmapper/transform.go:TransformAndParse
pub fn transform_and_parse(
    parse_options: &SourceFileParseOptions,
    content: &[u8],
    mapper: &ContentMapper,
    mapper_index: usize,
    project: &dyn Project,
    counters: &Counters,
) -> Result<SourceFiles, Error> {
    let transform_identity = project
        .identity(mapper_index)
        .map_err(|error| transform_error(TransformErrorKind::Project, Some(error)))?;
    let result = project.transform(
        mapper_index,
        &Request {
            file_name: parse_options.file_name.clone(),
            content: content.to_vec(),
        },
    )?;
    parse_result(
        parse_options,
        content,
        mapper,
        &transform_identity,
        result,
        counters,
    )
}

fn parse(
    options: SourceFileParseOptions,
    text: &str,
    file_name: &[u8],
    counters: &Counters,
) -> ParsedFile {
    tsr_parser::parse_source_file_with_counters(
        SourceText::from_bytes(text.as_bytes()),
        ScriptKind::from_file_name(file_name),
        options,
        counters,
    )
}

fn concat(left: &[u8], right: &[u8]) -> JsString {
    let mut bytes = left.to_vec();
    bytes.extend_from_slice(right);
    JsString::from_bytes(bytes)
}

/// Validates and parses one mapper result and all its supplemental outputs.
/// port: tsc/internal/contentmapper/transform.go:ParseResult
pub fn parse_result(
    parse_options: &SourceFileParseOptions,
    content: &[u8],
    mapper: &ContentMapper,
    transform_identity: &str,
    result: TransformResult,
    counters: &Counters,
) -> Result<SourceFiles, Error> {
    let Some(mappings) = result.mappings.clone() else {
        return Err(transform_error(TransformErrorKind::Mappings, None));
    };
    if let Some(problem) = mappings.validate(result.text.as_bytes(), content) {
        return Err(Error::Mapping(problem));
    }
    let virtual_extension = result.virtual_extension.as_bytes();
    if !is_supported_virtual_extension(virtual_extension) {
        return Err(transform_error(TransformErrorKind::Response, None));
    }
    let base = parse_options.clone();
    let virtual_file_name = concat(base.file_name.as_bytes(), virtual_extension);
    let mut options = base.clone();
    if is_module_virtual_extension(virtual_extension) {
        options.external_module_indicator_options.force = true;
    }
    let mut canonical = parse(
        options,
        &result.text,
        virtual_file_name.as_bytes(),
        counters,
    );
    let mut supplemental = Vec::with_capacity(result.supplemental.len());
    for (i, output) in result.supplemental.iter().enumerate() {
        let Some(mappings) = &output.mappings else {
            return Err(transform_error(TransformErrorKind::Mappings, None));
        };
        if let Some(problem) = mappings.validate(output.text.as_bytes(), content) {
            return Err(Error::Mapping(problem));
        }
        let extension = output.virtual_extension.as_bytes();
        if !is_supported_virtual_extension(extension) {
            return Err(transform_error(TransformErrorKind::Response, None));
        }
        let suffix = format!(".{i}{}", output.virtual_extension);
        let mut options = base.clone();
        options.file_name = concat(base.file_name.as_bytes(), suffix.as_bytes());
        options.path = concat(parse_options.path.as_bytes(), suffix.as_bytes());
        if is_module_virtual_extension(extension) {
            options.external_module_indicator_options.force = true;
        }
        let file_name = options.file_name.clone();
        supplemental.push(parse(options, &output.text, file_name.as_bytes(), counters));
    }
    let mapper_identity = identity(mapper);
    let transform_identity = JsString::from_bytes(transform_identity.as_bytes());
    let original_text = SourceText::from_bytes(content);
    let supplemental_file_names: Vec<JsString> = supplemental
        .iter()
        .map(|file| {
            file.view()
                .source_file(file.root())
                .map(|source| source.parse_options().file_name.clone())
        })
        .collect::<Result<_, _>>()
        .map_err(|error| Error::Message(format!("{error:?}")))?;
    let attach = |file: &mut ParsedFile,
                  info: ContentMapperSourceFileInfo,
                  directives,
                  diagnostics: Vec<tsr_ast::Diagnostic>|
     -> Result<(), tsr_arena::Error> {
        let directives = file.diagnostic_directives(directives)?;
        let root = file.root();
        let state = file.root_source_file_mut()?;
        if !diagnostics.is_empty() {
            // The mapper's diagnostics had no file yet; they belong to this one.
            let mut all = state.diagnostics().to_vec();
            all.extend(diagnostics.into_iter().map(|mut diagnostic| {
                diagnostic.file = Some(root);
                diagnostic
            }));
            state.set_diagnostics(all);
        }
        state.set_content_mapper_info(ContentMapperSourceFileInfo {
            diagnostic_directives: directives,
            ..info
        });
        Ok(())
    };
    let canonical_name = base.file_name.clone();
    attach(
        &mut canonical,
        ContentMapperSourceFileInfo {
            content_mapper: mapper_identity.clone(),
            transform_identity: transform_identity.clone(),
            parse_options: base.clone(),
            virtual_file_name,
            original_text: original_text.clone(),
            span_map: result.mappings,
            supplemental_file_names,
            ..ContentMapperSourceFileInfo::default()
        },
        result.diagnostic_directives,
        result.diagnostics,
    )
    .map_err(|error| Error::Message(format!("{error:?}")))?;
    for (file, output) in supplemental.iter_mut().zip(result.supplemental) {
        let name = file
            .view()
            .source_file(file.root())
            .map(|source| source.parse_options().file_name.clone())
            .map_err(|error| Error::Message(format!("{error:?}")))?;
        attach(
            file,
            ContentMapperSourceFileInfo {
                content_mapper: mapper_identity.clone(),
                transform_identity: transform_identity.clone(),
                parse_options: base.clone(),
                virtual_file_name: name,
                original_text: original_text.clone(),
                span_map: output.mappings,
                canonical_file_name: Some(canonical_name.clone()),
                ..ContentMapperSourceFileInfo::default()
            },
            output.diagnostic_directives,
            Vec::new(),
        )
        .map_err(|error| Error::Message(format!("{error:?}")))?;
    }
    Ok(SourceFiles {
        canonical,
        supplemental,
    })
}

/// port: tsc/internal/contentmapper/transform.go:isModuleVirtualExtension
fn is_module_virtual_extension(extension: &[u8]) -> bool {
    [
        tsr_tspath::EXTENSION_MTS,
        tsr_tspath::EXTENSION_CTS,
        tsr_tspath::EXTENSION_MJS,
        tsr_tspath::EXTENSION_CJS,
    ]
    .contains(&extension)
}

/// Rejects compiler-assigned virtual file names that name physical files.
/// port: tsc/internal/contentmapper/transform.go:CheckSupplementalFileNameCollisions
pub fn check_supplemental_file_name_collisions(
    files: &SourceFiles,
    file_exists: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<(), Error> {
    for file in &files.supplemental {
        let name = file
            .view()
            .source_file(file.root())
            .map(|source| source.parse_options().file_name.clone())
            .map_err(|error| Error::Message(format!("{error:?}")))?;
        if file_exists(name.as_bytes()) {
            return Err(Error::SupplementalFileCollision(name));
        }
    }
    Ok(())
}
