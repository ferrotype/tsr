//! File inclusion explanations retain source reason identity. Published program
//! edges own the reasons; diagnostics escape explicitly through `Arc` chains.
use crate::{Error, Program};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock, RwLock},
};
use ts_arena::Error as AstError;
use ts_ast::{Diagnostic, NodeId};
use ts_ast::{NodeDataRead, SyntaxKind as K};
use ts_core::TextRange;
use ts_core::{ModuleKind, ScriptTarget};
use ts_diagnostics::{self as d, Message};
use ts_jsstring::JsString;
use ts_module::PackageId;
use ts_tspath as path;

#[derive(Clone, Debug)]
pub(crate) struct SyntheticImport {
    pub name: JsString,
    pub helpers: bool,
}

/// source: tsc/internal/compiler/fileInclude.go:FileIncludeReason
#[derive(Clone, Debug)]
pub(crate) enum IncludeReasonData {
    Import {
        file: JsString,
        index: isize,
        synthetic: Option<SyntheticImport>,
        package_id: PackageId,
    },
    ReferenceFile {
        file: JsString,
        index: usize,
    },
    TypeReference {
        file: JsString,
        index: usize,
    },
    LibReference {
        file: JsString,
        index: usize,
    },
    Root {
        index: usize,
    },
    Lib {
        index: Option<usize>,
    },
    AutomaticType {
        name: JsString,
        package_id: PackageId,
    },
    #[allow(dead_code)] // The source kind is consumed when content mapping is available.
    ContentMapperSupplemental {
        file: JsString,
    },
}

type Cached<T> = OnceLock<Result<T, ts_arena::Error>>;

#[derive(Debug)]
pub(crate) struct IncludeReason {
    data: IncludeReasonData,
    diagnostic: Cached<Arc<Diagnostic>>,
    relative_diagnostic: Cached<Arc<Diagnostic>>,
    location: Cached<ReferenceLocation>,
    related: Cached<Option<Arc<Diagnostic>>>,
}
impl IncludeReason {
    pub(crate) fn new(data: IncludeReasonData) -> Self {
        Self {
            data,
            diagnostic: OnceLock::new(),
            relative_diagnostic: OnceLock::new(),
            location: OnceLock::new(),
            related: OnceLock::new(),
        }
    }

    fn is_referenced(&self) -> bool {
        matches!(
            self.data,
            IncludeReasonData::Import { .. }
                | IncludeReasonData::ReferenceFile { .. }
                | IncludeReasonData::TypeReference { .. }
                | IncludeReasonData::LibReference { .. }
        )
    }

    /// port: tsc/internal/compiler/includeprocessor.go:includeProcessor.getReferenceLocation
    fn reference_location(&self, program: &Program) -> Result<&ReferenceLocation, AstError> {
        self.location
            .get_or_init(|| self.compute_location(program))
            .as_ref()
            .map_err(|e| *e)
    }

    /// port: tsc/internal/compiler/fileInclude.go:FileIncludeReason.getReferencedLocation
    fn compute_location(&self, program: &Program) -> Result<ReferenceLocation, AstError> {
        let (IncludeReasonData::Import {
            file: file_path, ..
        }
        | IncludeReasonData::ReferenceFile {
            file: file_path, ..
        }
        | IncludeReasonData::TypeReference {
            file: file_path, ..
        }
        | IncludeReasonData::LibReference {
            file: file_path, ..
        }) = &self.data
        else {
            panic!("reason is not a referenced file");
        };
        let file = program
            .file(file_path.as_bytes())
            .expect("included reference source is loaded");
        let view = file.bound().view().ast();
        let source = view.source_file(file.source())?;
        let (loc, text, synthetic) = match &self.data {
            IncludeReasonData::Import {
                index, synthetic, ..
            } => {
                if let Some(synthetic) = synthetic {
                    let mut text = Vec::with_capacity(synthetic.name.len() + 2);
                    text.push(b'"');
                    text.extend_from_slice(synthetic.name.as_bytes());
                    text.push(b'"');
                    (TextRange::new(-1, -1), JsString::from_bytes(text), true)
                } else {
                    let imports = source.imports()?;
                    let node =
                        if *index < isize::try_from(imports.len()).expect("source import length") {
                            // A negative, non-synthetic source index must fail at the
                            // same indexing boundary, rather than select an augmentation.
                            imports[usize::try_from(*index).expect("negative import index")]
                                .expect("import node")
                        } else {
                            let mut augmented_index =
                                isize::try_from(imports.len()).expect("source import length");
                            let mut selected = None;
                            for id in source.module_augmentations()?.iter() {
                                let id = id.expect("module augmentation node");
                                if view.node(id)?.kind() == K::StringLiteral {
                                    if augmented_index == *index {
                                        selected = Some(id);
                                        break;
                                    }
                                    augmented_index += 1;
                                }
                            }
                            selected.expect("import or string-literal module augmentation")
                        };
                    let read = view.node(node)?;
                    let start =
                        ts_scanner::skip_trivia(source.text().as_bytes(), i64::from(read.pos()));
                    let loc = TextRange::new(start, i64::from(read.end()));
                    let text = if ts_ast::utilities::node_is_synthesized(&read) {
                        let mut text = vec![b'"'];
                        text.extend_from_slice(view.node_text(node)?.as_bytes());
                        text.push(b'"');
                        JsString::from_bytes(text)
                    } else {
                        source_slice(source.text(), loc)
                    };
                    (loc, text, false)
                }
            }
            IncludeReasonData::ReferenceFile { index, .. } => {
                let reference = &source.referenced_files()?[*index];
                (
                    reference.loc,
                    source_slice(source.text(), reference.loc),
                    false,
                )
            }
            IncludeReasonData::TypeReference { index, .. } => {
                let reference = &source.type_reference_directives()?[*index];
                (
                    reference.loc,
                    source_slice(source.text(), reference.loc),
                    false,
                )
            }
            IncludeReasonData::LibReference { index, .. } => {
                let reference = &source.lib_reference_directives()?[*index];
                (
                    reference.loc,
                    source_slice(source.text(), reference.loc),
                    false,
                )
            }
            _ => unreachable!("referenced kind checked above"),
        };
        Ok(ReferenceLocation {
            source: file.source(),
            loc,
            text,
            file_name: source.parse_options().file_name.clone(),
            synthetic,
        })
    }

    /// port: tsc/internal/compiler/fileInclude.go:FileIncludeReason.toDiagnostic
    fn diagnostic(&self, program: &Program, relative: bool) -> Result<&Arc<Diagnostic>, AstError> {
        let cache = if relative {
            &self.relative_diagnostic
        } else {
            &self.diagnostic
        };
        cache
            .get_or_init(|| self.compute_diagnostic(program, relative).map(Arc::new))
            .as_ref()
            .map_err(|e| *e)
    }

    /// port: tsc/internal/compiler/fileInclude.go:FileIncludeReason.computeDiagnostic
    fn compute_diagnostic(
        &self,
        program: &Program,
        relative: bool,
    ) -> Result<Diagnostic, AstError> {
        if self.is_referenced() {
            return self.reference_diagnostic(program, relative);
        }
        let options = program.options();
        let mut args = Vec::new();
        let message = match &self.data {
            IncludeReasonData::Root { index } => {
                let config = program.config();
                if config.config_file.is_some() {
                    let file_name = path::absolute(
                        config.root_file_names[*index].as_bytes(),
                        program.current_directory(),
                    );
                    let matched = config.matched_file_spec(&file_name);
                    if matched.is_empty() {
                        let (matched, default) = config.matched_include_spec(&file_name);
                        if matched.is_empty() {
                            d::Root_file_specified_for_compilation
                        } else if default {
                            d::Matched_by_default_include_pattern_Asterisk_Asterisk_Slash_Asterisk
                        } else {
                            args.extend([
                                JsString::from_bytes(matched),
                                file_name_for(program, config.config_name().as_bytes(), relative),
                            ]);
                            d::Matched_by_include_pattern_0_in_1
                        }
                    } else {
                        args.extend([
                            JsString::from_bytes(matched),
                            file_name_for(program, &file_name, relative),
                        ]);
                        d::Part_of_files_list_in_tsconfig_json
                    }
                } else {
                    d::Root_file_specified_for_compilation
                }
            }
            IncludeReasonData::AutomaticType { name, package_id } => {
                args.push(name.clone());
                if !package_id.name.is_empty() {
                    args.push(package_id_text(package_id));
                }
                match (options.uses_wildcard_types(), package_id.name.is_empty()) {
                    (false, true) => d::Entry_point_of_type_library_0_specified_in_compilerOptions,
                    (false, false) => d::Entry_point_of_type_library_0_specified_in_compilerOptions_with_packageId_1,
                    (true, true) => d::Entry_point_for_implicit_type_library_0,
                    (true, false) => d::Entry_point_for_implicit_type_library_0_with_packageId_1,
                }
            }
            IncludeReasonData::Lib { index: Some(index) } => {
                args.push(options.lib.as_ref().expect("explicit library list")[*index].clone());
                d::Library_0_specified_in_compilerOptions
            }
            IncludeReasonData::Lib { index: None } => {
                args.push(script_target_text(options.emit_script_target()));
                d::Default_library_for_target_0
            }
            IncludeReasonData::ContentMapperSupplemental { file } => {
                let file = program
                    .file(file.as_bytes())
                    .expect("content mapper canonical file");
                args.push(file_name_for(
                    program,
                    file.bound()
                        .view()
                        .source_file()?
                        .parse_options()
                        .file_name
                        .as_bytes(),
                    relative,
                ));
                d::Supplemental_virtual_file_produced_by_the_content_mapper_for_file_0
            }
            _ => unreachable!("references were handled first"),
        };
        Ok(Diagnostic::compiler(message, args))
    }

    /// port: tsc/internal/compiler/fileInclude.go:FileIncludeReason.computeReferenceFileDiagnostic
    fn reference_diagnostic(
        &self,
        program: &Program,
        relative: bool,
    ) -> Result<Diagnostic, AstError> {
        let location = self.reference_location(program)?;
        let mut args = vec![
            location.text.clone(),
            file_name_for(program, location.file_name.as_bytes(), relative),
        ];
        let message = match &self.data {
            IncludeReasonData::Import {
                synthetic,
                package_id,
                ..
            } => {
                let package = !package_id.name.is_empty();
                if package {
                    args.push(package_id_text(package_id));
                }
                match (synthetic.as_ref().map(|s| s.helpers), package) {
                    (None, false) => d::Imported_via_0_from_file_1,
                    (None, true) => d::Imported_via_0_from_file_1_with_packageId_2,
                    (Some(true), false) => d::Imported_via_0_from_file_1_to_import_importHelpers_as_specified_in_compilerOptions,
                    (Some(true), true) => d::Imported_via_0_from_file_1_with_packageId_2_to_import_importHelpers_as_specified_in_compilerOptions,
                    (Some(false), false) => d::Imported_via_0_from_file_1_to_import_jsx_and_jsxs_factory_functions,
                    (Some(false), true) => d::Imported_via_0_from_file_1_with_packageId_2_to_import_jsx_and_jsxs_factory_functions,
                }
            }
            IncludeReasonData::ReferenceFile { .. } => d::Referenced_via_0_from_file_1,
            // getReferencedLocation leaves PackageId zero for type directives.
            IncludeReasonData::TypeReference { .. } => d::Type_library_referenced_via_0_from_file_1,
            IncludeReasonData::LibReference { .. } => d::Library_referenced_via_0_from_file_1,
            _ => unreachable!("reference diagnostic requires referenced kind"),
        };
        Ok(Diagnostic::compiler(message, args))
    }

    /// port: tsc/internal/compiler/includeprocessor.go:includeProcessor.getRelatedInfo
    fn related_info(&self, program: &Program) -> Result<Option<&Arc<Diagnostic>>, AstError> {
        self.related
            .get_or_init(|| self.compute_related_info(program).map(|d| d.map(Arc::new)))
            .as_ref()
            .map(Option::as_ref)
            .map_err(|e| *e)
    }

    /// port: tsc/internal/compiler/fileInclude.go:FileIncludeReason.toRelatedInfo
    fn compute_related_info(&self, program: &Program) -> Result<Option<Diagnostic>, AstError> {
        if self.is_referenced() {
            let location = self.reference_location(program)?;
            if location.synthetic {
                return Ok(None);
            }
            let message = match self.data {
                IncludeReasonData::Import { .. } => d::File_is_included_via_import_here,
                IncludeReasonData::ReferenceFile { .. } => d::File_is_included_via_reference_here,
                IncludeReasonData::TypeReference { .. } => {
                    d::File_is_included_via_type_library_reference_here
                }
                IncludeReasonData::LibReference { .. } => {
                    d::File_is_included_via_library_reference_here
                }
                _ => unreachable!("reference related information requires referenced kind"),
            };
            return Ok(Some(location.diagnostic_at(message, Vec::new())));
        }
        let config = program.config();
        let Some(syntax) = &config.config_file else {
            return Ok(None);
        };
        let options = program.options();
        let selection = match &self.data {
            IncludeReasonData::Root { index } => {
                let file_name = path::absolute(
                    config.root_file_names[*index].as_bytes(),
                    program.current_directory(),
                );
                let matched = config.matched_file_spec(&file_name);
                if matched.is_empty() {
                    let (matched, default) = config.matched_include_spec(&file_name);
                    (!matched.is_empty() && !default).then_some((
                        syntax.object(),
                        b"include".as_slice(),
                        matched,
                        d::File_is_matched_by_include_pattern_specified_here,
                    ))
                } else {
                    Some((
                        syntax.object(),
                        b"files".as_slice(),
                        matched,
                        d::File_is_matched_by_files_list_specified_here,
                    ))
                }
            }
            IncludeReasonData::AutomaticType { name, .. } if !options.uses_wildcard_types() => {
                Some((
                    program.include_explanations.compiler_options(program),
                    b"types".as_slice(),
                    name.as_bytes(),
                    d::File_is_entry_point_of_type_library_specified_here,
                ))
            }
            IncludeReasonData::Lib { index: Some(index) } => Some((
                program.include_explanations.compiler_options(program),
                b"lib".as_slice(),
                options.lib.as_ref().expect("explicit library list")[*index].as_bytes(),
                d::File_is_library_specified_here,
            )),
            IncludeReasonData::Lib { index: None } => {
                // The pinned callback looks for an array element even for
                // "target". A scalar target value therefore yields no related info.
                let target = script_target_text(options.emit_script_target());
                return Ok(find_array_value(
                    syntax,
                    program.include_explanations.compiler_options(program),
                    b"target",
                    target.as_bytes(),
                )?
                .map(|node| {
                    ts_tsoptions::diagnostic_for_node(
                        syntax,
                        node,
                        d::File_is_default_library_for_target_specified_here,
                        Vec::new(),
                    )
                }));
            }
            _ => None,
        };
        let Some((object, key, value, message)) = selection else {
            return Ok(None);
        };
        Ok(find_array_value(syntax, object, key, value)?
            .map(|node| ts_tsoptions::diagnostic_for_node(syntax, node, message, Vec::new())))
    }
}

#[derive(Debug)]
struct ReferenceLocation {
    source: NodeId,
    loc: TextRange,
    text: JsString,
    file_name: JsString,
    synthetic: bool,
}
impl ReferenceLocation {
    /// port: tsc/internal/compiler/fileInclude.go:referenceFileLocation.diagnosticAt
    fn diagnostic_at(&self, message: &'static Message, args: Vec<JsString>) -> Diagnostic {
        Diagnostic::new(Some(self.source), self.loc, message, args)
    }
}

/// The source cache retains empty results as well as populated explanations.
#[derive(Default)]
pub(crate) struct IncludeExplanations {
    redirects: RwLock<BTreeMap<JsString, Arc<[Arc<Diagnostic>]>>>,
    compiler_options: OnceLock<Option<NodeId>>,
}

impl IncludeExplanations {
    /// port: tsc/internal/compiler/includeprocessor.go:includeProcessor.getCompilerOptionsObjectLiteralSyntax
    fn compiler_options(&self, program: &Program) -> Option<NodeId> {
        *self.compiler_options.get_or_init(|| {
            let syntax = program.config().config_file.as_ref()?;
            let property = ts_tsoptions::find_property(syntax, &[b"compilerOptions"])?;
            let view = syntax.file.view();
            let node = view.node(property).expect("config property owner");
            let NodeDataRead::PropertyAssignment(data) = node.data() else {
                unreachable!("property lookup returns assignment")
            };
            let initializer = data.initializer()?;
            (view
                .node(initializer)
                .expect("config initializer owner")
                .kind()
                == K::ObjectLiteralExpression)
                .then_some(initializer)
        })
    }

    /// port: tsc/internal/compiler/includeprocessor.go:includeProcessor.explainRedirectAndImpliedFormat
    fn redirects(
        &self,
        program: &Program,
        file_path: &[u8],
        relative: bool,
    ) -> Result<Arc<[Arc<Diagnostic>]>, AstError> {
        if let Some(existing) = self
            .redirects
            .read()
            .expect("include cache lock")
            .get(file_path)
        {
            return Ok(Arc::clone(existing));
        }
        let mut result = Vec::new();
        if let Some(target) = program.redirect_paths.get(file_path) {
            let target = program
                .file(target.as_bytes())
                .expect("redirect target file");
            let source = target.bound().view().source_file()?;
            result.push(Arc::new(Diagnostic::compiler(
                d::File_redirects_to_file_0,
                vec![file_name_for(
                    program,
                    source.parse_options().file_name.as_bytes(),
                    relative,
                )],
            )));
        } else {
            let Some(file) = program.file(file_path) else {
                return Ok(Arc::from([]));
            };
            let source = file.bound().view().source_file()?;
            if ts_ast::utilities::is_external_or_common_js_module(&source) {
                let metadata = program.metadata(file_path).expect("loaded source metadata");
                let emit = crate::metadata::implied_for_emit(
                    source.parse_options().file_name.as_bytes(),
                    program.options().emit_module_kind(),
                    metadata,
                );
                let mut args = Vec::new();
                let message = if emit == ModuleKind::ESNEXT
                    && metadata.package_json_type.as_bytes() == b"module"
                {
                    Some(d::File_is_ECMAScript_module_because_0_has_field_type_with_value_module)
                } else if emit == ModuleKind::COMMON_JS {
                    if !metadata.package_json_type.is_empty() {
                        Some(d::File_is_CommonJS_module_because_0_has_field_type_whose_value_is_not_module)
                    } else if !metadata.package_json_directory.is_empty() {
                        Some(d::File_is_CommonJS_module_because_0_does_not_have_field_type)
                    } else {
                        Some(d::File_is_CommonJS_module_because_package_json_was_not_found)
                    }
                } else {
                    None
                };
                if let Some(message) = message {
                    if message.code
                        != d::File_is_CommonJS_module_because_package_json_was_not_found.code
                    {
                        let mut file_name = metadata.package_json_directory.as_bytes().to_vec();
                        file_name.extend_from_slice(b"/package.json");
                        args.push(file_name_for(program, &file_name, relative));
                    }
                    result.push(Arc::new(Diagnostic::compiler(message, args)));
                }
            }
        }
        let mut cache = self.redirects.write().expect("include cache lock");
        Ok(Arc::clone(
            cache
                .entry(JsString::from_bytes(file_path))
                .or_insert_with(|| Arc::from(result)),
        ))
    }
}

impl Program {
    /// Explain a source inclusion diagnostic. A real incoming reference supplies
    /// its location; callers must classify the resulting diagnostic by `file`.
    /// The program owns all raw file identities retained by the result.
    /// port: tsc/internal/compiler/processingDiagnostic.go:processingDiagnostic.createDiagnosticExplainingFile
    pub fn explain_file_include(
        &self,
        file_path: &[u8],
        message: &'static Message,
        args: Vec<JsString>,
    ) -> Result<Diagnostic, Error> {
        self.explain_file_include_with_reason(file_path, None, message, args)
            .map_err(Error::from)
    }

    pub(crate) fn explain_file_include_with_reason<'a>(
        &'a self,
        file_path: &[u8],
        diagnostic_reason: Option<&'a IncludeReason>,
        message: &'static Message,
        args: Vec<JsString>,
    ) -> Result<Diagnostic, AstError> {
        let mut details = Vec::new();
        let mut related = Vec::new();
        let mut preferred = diagnostic_reason.filter(|reason| reason.is_referenced());
        if let Some(reason) = preferred {
            if reason.reference_location(self)?.synthetic {
                preferred = None;
            }
        }
        let mut seen = std::collections::HashSet::new();
        let mut process_reason = |reason: &'a IncludeReason| -> Result<(), AstError> {
            if !seen.insert(std::ptr::from_ref(reason)) {
                return Ok(());
            }
            details.push(Arc::clone(reason.diagnostic(self, false)?));
            if preferred.is_none()
                && reason.is_referenced()
                && !reason.reference_location(self)?.synthetic
            {
                preferred = Some(reason);
            } else if !preferred.is_some_and(|selected| std::ptr::eq(selected, reason)) {
                if let Some(info) = reason.related_info(self)? {
                    related.push(Arc::clone(info));
                }
            }
            Ok(())
        };
        if !file_path.is_empty() {
            if let Some(reasons) = self.include_reasons.get(file_path) {
                for reason in reasons {
                    process_reason(reason)?;
                }
            }
        }
        if let Some(reason) = diagnostic_reason {
            process_reason(reason)?;
        }
        let mut result = if let Some(preferred) = preferred {
            preferred
                .reference_location(self)?
                .diagnostic_at(message, args)
        } else {
            Diagnostic::compiler(message, args)
        };
        if (!file_path.is_empty() || !details.is_empty())
            && (preferred.is_none() || seen.len() != 1)
        {
            let mut reason =
                Diagnostic::compiler(d::The_file_is_in_the_program_because_Colon, Vec::new());
            reason.message_chain = details;
            result.message_chain.push(Arc::new(reason));
        }
        if !file_path.is_empty() {
            result.message_chain.extend(
                self.include_explanations
                    .redirects(self, file_path, false)?
                    .iter()
                    .cloned(),
            );
        }
        result.related_information = related;
        Ok(result)
    }
}

fn source_slice(source: &ts_jsstring::SourceText, loc: TextRange) -> JsString {
    source
        .slice(
            usize::try_from(loc.pos()).expect("negative reference position")
                ..usize::try_from(loc.end()).expect("negative reference end"),
        )
        .expect("reference source range")
}
fn file_name_for(program: &Program, file_name: &[u8], relative: bool) -> JsString {
    if relative {
        JsString::from_bytes(path::relative_from_directory(
            program.current_directory(),
            file_name,
            program.current_directory(),
            program.host().use_case_sensitive_file_names(),
        ))
    } else {
        JsString::from_bytes(file_name)
    }
}
/// port: tsc/internal/module/types.go:PackageId.String
fn package_id_text(package: &PackageId) -> JsString {
    let mut result = package.name.as_bytes().to_vec();
    if !package.sub_module_name.is_empty() {
        result.push(b'/');
        result.extend_from_slice(package.sub_module_name.as_bytes());
    }
    result.push(b'@');
    result.extend_from_slice(package.version.as_bytes());
    result.extend_from_slice(package.peer_dependencies.as_bytes());
    JsString::from_bytes(result)
}
// Stringer always returns a nonempty string, including open enum values.
fn script_target_text(target: ScriptTarget) -> JsString {
    let text = match target.0 {
        0 => "None",
        1 => "ES5",
        2 => "ES2015",
        3 => "ES2016",
        4 => "ES2017",
        5 => "ES2018",
        6 => "ES2019",
        7 => "ES2020",
        8 => "ES2021",
        9 => "ES2022",
        10 => "ES2023",
        11 => "ES2024",
        12 => "ES2025",
        99 => "ESNext",
        100 => "JSON",
        _ => return JsString::from_bytes(format!("ScriptTarget({})", target.0).into_bytes()),
    };
    JsString::from_bytes(text.as_bytes())
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:GetCallbackForFindingPropertyAssignmentByValue
fn find_array_value(
    config: &ts_tsoptions::TsConfigSourceFile,
    object: Option<NodeId>,
    key: &[u8],
    value: &[u8],
) -> Result<Option<NodeId>, AstError> {
    let Some(object) = object else {
        return Ok(None);
    };
    let Some(property) = ts_tsoptions::find_property_in_object(config, object, &[key]) else {
        return Ok(None);
    };
    let view = config.file.view();
    let property = view.node(property)?;
    let NodeDataRead::PropertyAssignment(data) = property.data() else {
        unreachable!("property lookup returns assignment")
    };
    let Some(initializer) = data.initializer() else {
        return Ok(None);
    };
    let node = view.node(initializer)?;
    let NodeDataRead::ArrayLiteralExpression(data) = node.data() else {
        return Ok(None);
    };
    if let Some(elements) = data.elements() {
        for id in view.node_slice(view.list(elements)?.nodes())?.iter() {
            let id = id.expect("config array element");
            if view.node(id)?.kind() == K::StringLiteral && view.node_text(id)?.as_bytes() == value
            {
                return Ok(Some(id));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
#[path = "include_reason_tests.rs"]
mod tests;
