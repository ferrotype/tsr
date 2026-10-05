//! Content-mapped files in the program loader: the mapper failure budget and
//! the diagnostics that explain a transform the program could not use
//! (`fileloader.go`'s content-mapper paths).
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tsr_ast::span_map::{MappingError, MappingErrorKind};
use tsr_ast::{Diagnostic, NodeId};
use tsr_contentmapper::{
    DiagnosticDirectiveErrorKind, Error, InitializeErrorKind, Project, ProjectErrorKind,
    TransformErrorKind,
};
use tsr_core::TextRange;
use tsr_diagnostics as d;
use tsr_diagnostics::Message;
use tsr_jsstring::JsString;

/// The transform failures one mapper may accumulate before it is disabled for
/// the rest of the program.
const MAX_CONTENT_MAPPER_FAILURES: usize = 5;

fn text(value: impl std::fmt::Display) -> JsString {
    JsString::from_bytes(value.to_string().into_bytes())
}

/// The loader's content-mapper state. A mapper is its index in the config's
/// mapper list, the pin's pointer identity.
#[derive(Default)]
pub(crate) struct ContentMapperState {
    /// The host's project, when the host runs mappers.
    pub project: Option<Arc<dyn Project>>,
    /// The extensions the configured mappers register.
    pub extensions: Vec<JsString>,
    failures: BTreeMap<usize, usize>,
    initialization_failed: BTreeSet<usize>,
    /// Program diagnostics: failed initializations and disabled mappers.
    pub diagnostics: Vec<Diagnostic>,
    /// Supplemental files parsed with their canonical file, by path, until
    /// their own tasks load them.
    pub supplementals: BTreeMap<JsString, Arc<crate::ProgramFile>>,
}

impl ContentMapperState {
    pub fn new(project: Option<Arc<dyn Project>>, extensions: Vec<JsString>) -> Self {
        Self {
            project,
            extensions,
            ..Self::default()
        }
    }

    /// The mapper failed initialization or exhausted its failure budget.
    /// port: tsc/internal/compiler/fileloader.go:fileLoader.contentMapperUnavailable
    pub fn unavailable(&self, mapper: usize) -> bool {
        self.initialization_failed.contains(&mapper)
            || self.failures.get(&mapper).copied().unwrap_or(0) >= MAX_CONTENT_MAPPER_FAILURES
    }

    /// Reports a mapper's failed initialization once.
    /// port: tsc/internal/compiler/fileloader.go:fileLoader.recordContentMapperInitializationFailure
    pub fn record_initialization_failure(
        &mut self,
        mapper: usize,
        label: &JsString,
        error: &Error,
    ) {
        if self.initialization_failed.insert(mapper) {
            self.diagnostics
                .push(initialization_diagnostic(label, error));
        }
    }

    /// Counts a transform failure. Returns whether this file reports it; the
    /// failure that exhausts the budget also disables the mapper.
    /// port: tsc/internal/compiler/fileloader.go:fileLoader.recordContentMapperFailure
    pub fn record_failure(&mut self, mapper: usize, label: &JsString) -> bool {
        let failures = self.failures.entry(mapper).or_insert(0);
        if *failures >= MAX_CONTENT_MAPPER_FAILURES {
            return false;
        }
        *failures += 1;
        if *failures >= MAX_CONTENT_MAPPER_FAILURES {
            self.diagnostics.push(Diagnostic::compiler(
                d::The_content_mapper_0_failed_1_times_and_will_not_be_used,
                vec![label.clone(), text(MAX_CONTENT_MAPPER_FAILURES)],
            ));
        }
        true
    }
}

/// The diagnostic a file reports for a transform the program could not use.
/// port: tsc/internal/compiler/fileloader.go:contentMapperTransformDiagnostic
pub(crate) fn transform_diagnostic(file: NodeId, label: &JsString, error: &Error) -> Diagnostic {
    if let Error::SupplementalFileCollision(name) = error.cause() {
        return transform_diagnostic_chain(
            file,
            label,
            d::Content_mapper_supplemental_output_file_0_conflicts_with_an_existing_file,
            vec![name.clone()],
        );
    }
    match error.transform_kind() {
        Some(TransformErrorKind::Initialize) => {
            if let Some(initialize) = error.initialize_error() {
                match initialize.kind {
                    InitializeErrorKind::PositionEncoding => {
                        return transform_diagnostic_chain(
                            file,
                            label,
                            d::The_content_mapper_selected_unsupported_position_encoding_0,
                            vec![text(initialize.position_encoding.as_str())],
                        );
                    }
                    InitializeErrorKind::EmptyDiagnosticSource => {
                        return transform_diagnostic_chain(
                            file,
                            label,
                            d::The_content_mapper_diagnostic_source_must_not_be_empty,
                            Vec::new(),
                        );
                    }
                    InitializeErrorKind::ReservedDiagnosticSource => {
                        return transform_diagnostic_chain(
                            file,
                            label,
                            d::The_content_mapper_diagnostic_source_0_is_reserved_by_TypeScript,
                            vec![text(&initialize.diagnostic_source)],
                        );
                    }
                    _ => {}
                }
            }
            transform_diagnostic_chain(
                file,
                label,
                d::The_content_mapper_process_could_not_be_started_or_initialized,
                Vec::new(),
            )
        }
        Some(TransformErrorKind::Project) => {
            transform_diagnostic_chain(file, label, project_error_message(error), Vec::new())
        }
        Some(TransformErrorKind::Request) => transform_diagnostic_chain(
            file,
            label,
            d::The_content_mapper_process_failed_while_handling_the_transform_request,
            Vec::new(),
        ),
        Some(TransformErrorKind::Response) => {
            match error.cause() {
                Error::InvalidVirtualExtension(extension) => return transform_diagnostic_chain(
                    file,
                    label,
                    d::The_content_mapper_returned_an_output_with_unsupported_virtual_extension_0,
                    vec![text(extension)],
                ),
                Error::DiagnosticDirective {
                    kind,
                    index,
                    supplemental_index,
                    policy,
                } => {
                    let (message, args) = match kind {
                        DiagnosticDirectiveErrorKind::InvalidRange => (
                            d::Diagnostic_directive_0_returned_by_the_content_mapper_has_an_invalid_range,
                            vec![text(index)],
                        ),
                        DiagnosticDirectiveErrorKind::InvalidPolicy => (
                            d::The_content_mapper_returned_a_diagnostic_directive_with_invalid_policy_0,
                            vec![text(policy)],
                        ),
                        DiagnosticDirectiveErrorKind::ExpectMissingUnusedDiagnostic => (
                            d::Diagnostic_directive_0_returned_by_the_content_mapper_must_specify_unusedExpectDirectiveIndex_when_there_is_not_exactly_one_unusedExpectDirectiveDiagnostics_entry,
                            vec![text(index)],
                        ),
                        DiagnosticDirectiveErrorKind::InvalidUnusedDiagnosticIndex => (
                            d::Diagnostic_directive_0_returned_by_the_content_mapper_has_an_invalid_unusedExpectDirectiveIndex,
                            vec![text(index)],
                        ),
                        DiagnosticDirectiveErrorKind::Overlap => (
                            d::The_content_mapper_returned_diagnostic_directives_with_overlapping_virtual_ranges,
                            Vec::new(),
                        ),
                    };
                    let mut detail = Diagnostic::compiler(message, args);
                    if let Some(supplemental) = supplemental_index {
                        detail = Diagnostic::chain(
                            Some(Arc::new(detail)),
                            d::The_invalid_diagnostic_directive_is_in_supplemental_output_0_returned_by_the_content_mapper,
                            vec![text(supplemental)],
                        );
                    }
                    return transform_diagnostic_with_detail(file, label, detail);
                }
                _ => {}
            }
            transform_diagnostic_chain(
                file,
                label,
                d::The_content_mapper_returned_an_invalid_transform_response,
                Vec::new(),
            )
        }
        Some(TransformErrorKind::Mappings) => Diagnostic::new(
            Some(file),
            TextRange::new(0, 0),
            d::The_content_mapper_0_did_not_provide_the_required_position_mappings,
            vec![label.clone()],
        ),
        Some(TransformErrorKind::Unknown) | None => Diagnostic::new(
            Some(file),
            TextRange::new(0, 0),
            d::The_content_mapper_0_failed_to_transform_this_file,
            vec![label.clone()],
        ),
    }
}

/// The message of a project setup error.
/// port: tsc/internal/compiler/fileloader.go:ContentMapperProjectErrorDiagnostic
pub fn project_error_message(error: &Error) -> &'static Message {
    match error.project_error() {
        Some(ProjectErrorKind::MalformedResponse) => {
            d::The_content_mapper_returned_a_project_response_that_could_not_be_decoded
        }
        Some(ProjectErrorKind::MissingConfigIdentity) => {
            d::The_content_mapper_did_not_return_configIdentity_which_is_required_when_the_content_mapper_has_dynamicConfig_Colon_true_in_its_package_json
        }
        Some(ProjectErrorKind::NonAbsoluteWatchedFile) => {
            d::The_content_mapper_returned_a_non_absolute_path_in_watchedFiles
        }
        Some(ProjectErrorKind::UnexpectedConfigIdentity) => {
            d::The_content_mapper_returned_configIdentity_which_is_only_allowed_when_it_declares_dynamicConfig_Colon_true_in_its_package_json
        }
        Some(ProjectErrorKind::UnexpectedWatchedFiles) => {
            d::The_content_mapper_returned_watchedFiles_which_is_only_allowed_when_it_declares_dynamicConfig_Colon_true_in_its_package_json
        }
        None => d::The_content_mapper_process_failed_while_handling_the_project_request,
    }
}

/// port: tsc/internal/compiler/fileloader.go:contentMapperTransformDiagnosticChain
fn transform_diagnostic_chain(
    file: NodeId,
    label: &JsString,
    message: &'static Message,
    args: Vec<JsString>,
) -> Diagnostic {
    transform_diagnostic_with_detail(file, label, Diagnostic::compiler(message, args))
}

/// port: tsc/internal/compiler/fileloader.go:contentMapperTransformDiagnosticWithDetail
fn transform_diagnostic_with_detail(
    file: NodeId,
    label: &JsString,
    detail: Diagnostic,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::new(
        Some(file),
        TextRange::new(0, 0),
        d::The_content_mapper_0_failed_to_transform_this_file,
        vec![label.clone()],
    );
    diagnostic.message_chain.push(Arc::new(detail));
    diagnostic
}

/// The diagnostic of a mapper whose span map is invalid, with the offsets
/// that locate the problem.
/// port: tsc/internal/compiler/fileloader.go:contentMapperMappingDiagnostic
pub(crate) fn mapping_diagnostic(
    file: NodeId,
    label: &JsString,
    problem: &MappingError,
) -> Diagnostic {
    let (message, args) = match problem.kind {
        MappingErrorKind::Overlap => (
            d::The_content_mapper_0_produced_overlapping_or_out_of_order_position_mappings_near_virtual_offset_1,
            vec![label.clone(), text(problem.virtual_pos)],
        ),
        MappingErrorKind::OutOfBounds => (
            d::The_content_mapper_0_produced_a_position_mapping_that_points_outside_the_original_content_original_offset_1,
            vec![label.clone(), text(problem.original_pos)],
        ),
        MappingErrorKind::VerbatimMismatch => (
            d::The_content_mapper_0_produced_a_verbatim_mapping_that_does_not_match_the_original_content_virtual_offset_1_original_offset_2,
            vec![label.clone(), text(problem.virtual_pos), text(problem.original_pos)],
        ),
        MappingErrorKind::Kind => (
            d::The_content_mapper_0_produced_a_position_mapping_with_an_invalid_kind_near_virtual_offset_1,
            vec![label.clone(), text(problem.virtual_pos)],
        ),
        MappingErrorKind::Feature => (
            d::The_content_mapper_0_produced_invalid_mapping_features_near_original_offset_1,
            vec![label.clone(), text(problem.original_pos)],
        ),
    };
    Diagnostic::new(Some(file), TextRange::new(0, 0), message, args)
}

/// A fileless diagnostic for a mapper whose initialization failed.
/// port: tsc/internal/compiler/fileloader.go:ContentMapperInitializationDiagnostic
pub fn initialization_diagnostic(label: &JsString, error: &Error) -> Diagnostic {
    let initialize = error.initialize_error();
    let label = match initialize {
        Some(initialize) if label.is_empty() => initialize.mapper_name.clone(),
        _ => label.clone(),
    };
    let mut diagnostic = Diagnostic::compiler(
        d::The_content_mapper_0_could_not_be_initialized,
        vec![label],
    );
    let (message, args) = match initialize {
        Some(initialize) => match initialize.kind {
            InitializeErrorKind::ProcessStart => (
                d::The_content_mapper_command_0_could_not_be_started_Colon_1,
                vec![initialize.command.clone(), text(&initialize.detail)],
            ),
            InitializeErrorKind::ProcessExit => (
                d::The_content_mapper_process_exited_before_responding_to_the_initialize_request_exit_code_0,
                vec![text(initialize.exit_code)],
            ),
            InitializeErrorKind::NoResponse => (
                d::The_content_mapper_did_not_respond_to_the_initialize_request_within_0_seconds,
                vec![text(initialize.timeout_seconds)],
            ),
            InitializeErrorKind::InvalidResponse => (
                d::The_content_mapper_returned_an_initialize_response_that_could_not_be_decoded_Colon_0,
                vec![text(&initialize.detail)],
            ),
            InitializeErrorKind::Request => (
                d::The_content_mapper_s_initialize_request_failed_Colon_0,
                vec![text(&initialize.detail)],
            ),
            InitializeErrorKind::PositionEncoding => (
                d::The_content_mapper_selected_unsupported_position_encoding_0,
                vec![text(initialize.position_encoding.as_str())],
            ),
            InitializeErrorKind::EmptyDiagnosticSource => {
                (d::The_content_mapper_diagnostic_source_must_not_be_empty, Vec::new())
            }
            InitializeErrorKind::ReservedDiagnosticSource => (
                d::The_content_mapper_diagnostic_source_0_is_reserved_by_TypeScript,
                vec![text(&initialize.diagnostic_source)],
            ),
        },
        None => (d::The_content_mapper_process_could_not_be_started_or_initialized, Vec::new()),
    };
    diagnostic
        .message_chain
        .push(Arc::new(Diagnostic::compiler(message, args)));
    diagnostic
}

/// A fileless diagnostic for a project setup or mapper initialization error.
// port: tsc/internal/compiler/fileloader.go:ContentMapperProjectDiagnostic
pub fn content_mapper_project_diagnostic(error: &Error) -> Diagnostic {
    if error.initialize_error().is_some() {
        return initialization_diagnostic(&JsString::default(), error);
    }
    Diagnostic::compiler(project_error_message(error), Vec::new())
}
