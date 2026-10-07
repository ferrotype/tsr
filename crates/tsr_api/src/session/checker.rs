//! The checker-backed queries: symbols, types and signatures by position,
//! location or handle, their properties, the intrinsic and well-known
//! handles, and the type-to-syntax conversions. Each runs on the project's
//! API checker through `setup_checker`.
//! port: tsc/internal/api/session.go
use super::checker_responses::literal_value_json;
use super::diagnostics::DiagnosticKind;
use super::handles::{resolve_node_handle, touching_property_name, CheckerSetup, Committed};
use super::responses::base64_standard;
use super::{client_error, ApiSession, SessionError, SessionResult};
use crate::proto::{
    DocumentIdentifier, NodeHandle, Params, ProjectId, SnapshotId, SourceFileResponse,
    SymbolResponse, TypeResponse, WellKnownSignaturesResponse, WellKnownSymbolsResponse,
};
use tsr_checker::{Operation, SignatureKind, SymbolRef, TypeRef};
use tsr_ipc::{Context, HandlerError, Response};

use super::checker_error;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

type Setup<'a> = CheckerSetup<'a>;

impl ApiSession {
    /// Runs `query` on the project's API checker.
    fn with_checker<R: tsr_json::Encode>(
        &self,
        snapshot: SnapshotId,
        project: &ProjectId,
        query: impl FnOnce(&Setup<'_>, &mut Operation<'_>) -> SessionResult<R>,
    ) -> SessionResult<Committed> {
        let data = self.snapshot_data(snapshot)?;
        let setup = data.setup_checker(project)?;
        let mut operation = setup.registry.operation()?;
        #[cfg(feature = "fault-injection")]
        self.trip_fault(snapshot);
        let value = query(&setup, &mut operation)?;
        setup.commit(&value)
    }

    /// `with_checker` for a query that encodes its own response (a binary
    /// or base64 node encoding): the bytes are produced outside the gate
    /// and the gate revalidates the generation before they are returned.
    fn with_checker_response(
        &self,
        snapshot: SnapshotId,
        project: &ProjectId,
        query: impl FnOnce(&Setup<'_>, &mut Operation<'_>) -> SessionResult<Response>,
    ) -> SessionResult<Response> {
        let data = self.snapshot_data(snapshot)?;
        let setup = data.setup_checker(project)?;
        let mut operation = setup.registry.operation()?;
        #[cfg(feature = "fault-injection")]
        self.trip_fault(snapshot);
        let response = query(&setup, &mut operation)?;
        let _gate = setup.registry.gate()?;
        Ok(response)
    }

    /// The source file of a request, or the pin's client error.
    fn source_file_node(
        setup: &Setup<'_>,
        file: &DocumentIdentifier,
    ) -> SessionResult<tsr_ast::NodeId> {
        setup
            .program
            .source_file(file.to_file_name().as_bytes())
            .map(tsr_compiler::ProgramFile::source)
            .ok_or_else(|| client_error(format!("source file not found: {}", file.display())))
    }

    /// The node at a UTF-16 position of a file.
    fn node_at_position(
        setup: &Setup<'_>,
        file: &DocumentIdentifier,
        position: u32,
    ) -> SessionResult<Option<tsr_ast::NodeId>> {
        let node = touching_property_name(setup.program, file.to_file_name().as_bytes(), position)?;
        if node.is_none() {
            return Err(client_error(format!(
                "source file not found: {}",
                file.display()
            )));
        }
        Ok(node)
    }

    /// A location given as a node handle or as a file and position.
    /// port: tsc/internal/api/session.go:checkerSetup.resolveLocation
    fn resolve_location(
        setup: &Setup<'_>,
        handle: &NodeHandle,
        file: Option<&DocumentIdentifier>,
        position: Option<u32>,
    ) -> SessionResult<Option<tsr_ast::NodeId>> {
        if !handle.0.is_empty() {
            return resolve_node_handle(setup.program, handle).map(Some);
        }
        if let (Some(file), Some(position)) = (file, position) {
            return Self::node_at_position(setup, file, position);
        }
        Ok(None)
    }

    fn resolve_symbol(
        setup: &Setup<'_>,
        operation: &Operation<'_>,
        handle: crate::proto::SymbolId,
    ) -> SessionResult<SymbolRef> {
        setup.data.registries.resolve_symbol(operation, handle)
    }

    fn resolve_type(
        setup: &Setup<'_>,
        operation: &Operation<'_>,
        handle: crate::proto::TypeId,
    ) -> SessionResult<TypeRef> {
        setup.registry.resolve_type(operation, handle)
    }

    /// A node that may be a checker-synthesized type node, encoded for the
    /// client; raw on msgpack and base64 on JSON-RPC.
    fn encoded_node_response(
        &self,
        view: tsr_ast::AstView<'_>,
        node: Option<tsr_ast::NodeId>,
        what: &str,
    ) -> SessionResult<Response> {
        let Some(node) = node else {
            return Ok(if self.binary() {
                Response::binary(Vec::new())
            } else {
                Response::json(None::<SourceFileResponse>)
            });
        };
        let encoded = tsr_encoder::encode_node(
            view,
            node,
            None,
            &mut tsr_parser::ParserJsDocProvider::default(),
        )
        .map_err(|error| SessionError::Other(format!("failed to encode {what}: {error:?}")))?;
        Ok(if self.binary() {
            Response::binary(encoded.bytes)
        } else {
            Response::json(SourceFileResponse {
                data: base64_standard(&encoded.bytes),
            })
        })
    }

    /// The checker-backed methods; anything else is still unported.
    #[allow(
        clippy::redundant_closure_for_method_calls,
        reason = "the intrinsic getters are methods on Operation<'_>, whose path form is not general over the operation lifetime"
    )]
    pub(super) fn dispatch_checker(
        &self,
        ctx: &Context,
        params: Params,
        method: &str,
        files_named: bool,
    ) -> Result<Option<Response>, HandlerError> {
        let response = match params {
            Params::GetSymbolAtPosition(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let Some(node) = Self::node_at_position(setup, &p.file, p.position)? else {
                        return Ok(None);
                    };
                    match op.get_symbol_at_location(node).map_err(checker_error)? {
                        Some(symbol) => Ok(Some(setup.symbol_response(op, symbol)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetSymbolsAtPositions(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let mut results: Vec<Option<Box<SymbolResponse>>> =
                        Vec::with_capacity(p.positions.len());
                    for position in &p.positions {
                        let Some(node) = Self::node_at_position(setup, &p.file, *position)? else {
                            results.push(None);
                            continue;
                        };
                        results.push(
                            match op.get_symbol_at_location(node).map_err(checker_error)? {
                                Some(symbol) => Some(Box::new(setup.symbol_response(op, symbol)?)),
                                None => None,
                            },
                        );
                    }
                    Ok(results)
                })?)
            }
            Params::GetSymbolAtLocation(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let node = resolve_node_handle(setup.program, &p.location)?;
                    match op.get_symbol_at_location(node).map_err(checker_error)? {
                        Some(symbol) => Ok(Some(setup.symbol_response(op, symbol)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetSymbolsAtLocations(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let mut results: Vec<Option<Box<SymbolResponse>>> =
                        Vec::with_capacity(p.locations.len());
                    for location in &p.locations {
                        let node = resolve_node_handle(setup.program, location)?;
                        results.push(
                            match op.get_symbol_at_location(node).map_err(checker_error)? {
                                Some(symbol) => Some(Box::new(setup.symbol_response(op, symbol)?)),
                                None => None,
                            },
                        );
                    }
                    Ok(results)
                })?)
            }
            Params::GetSymbolOfSourceFile(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let node = Self::source_file_node(setup, &p.file)?;
                    match op.get_symbol_at_location(node).map_err(checker_error)? {
                        Some(symbol) => Ok(Some(setup.symbol_response(op, symbol)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetSymbolsOfSourceFiles(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let mut results: Vec<Option<Box<SymbolResponse>>> =
                        Vec::with_capacity(p.files.len());
                    for file in &p.files {
                        let node = Self::source_file_node(setup, file)?;
                        results.push(
                            match op.get_symbol_at_location(node).map_err(checker_error)? {
                                Some(symbol) => Some(Box::new(setup.symbol_response(op, symbol)?)),
                                None => None,
                            },
                        );
                    }
                    Ok(results)
                })?)
            }
            Params::GetTypeOfSymbol(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let symbol = Self::resolve_symbol(setup, op, p.symbol)?;
                    let ty = op.get_type_of_symbol(symbol).map_err(checker_error)?;
                    setup.type_response(op, ty)
                })?)
            }
            Params::GetTypesOfSymbols(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let mut results: Vec<Option<Box<TypeResponse>>> =
                        Vec::with_capacity(p.symbols.len());
                    for handle in &p.symbols {
                        let symbol = Self::resolve_symbol(setup, op, *handle)?;
                        let ty = op.get_type_of_symbol(symbol).map_err(checker_error)?;
                        results.push(Some(Box::new(setup.type_response(op, ty)?)));
                    }
                    Ok(results)
                })?)
            }
            Params::GetDeclaredTypeOfSymbol(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let symbol = Self::resolve_symbol(setup, op, p.symbol)?;
                    let ty = op
                        .get_declared_type_of_symbol(symbol)
                        .map_err(checker_error)?;
                    setup.type_response(op, ty)
                })?)
            }
            Params::GetNonMissingTypeOfSymbol(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let symbol = Self::resolve_symbol(setup, op, p.symbol)?;
                    let ty = op
                        .get_non_missing_type_of_symbol(symbol)
                        .map_err(checker_error)?;
                    setup.type_response(op, ty)
                })?)
            }
            Params::ResolveName(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let location = Self::resolve_location(
                        setup,
                        &p.location,
                        p.file.as_deref(),
                        p.position.as_deref().copied(),
                    )?;
                    match op
                        .resolve_name(p.name.as_bytes(), location, p.meaning, p.exclude_globals)
                        .map_err(checker_error)?
                    {
                        Some(symbol) => Ok(Some(setup.symbol_response(op, symbol)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetSymbolsInScope(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let location = Self::resolve_location(
                        setup,
                        &p.location,
                        p.file.as_deref(),
                        p.position.as_deref().copied(),
                    )?
                    .ok_or_else(|| client_error("getSymbolsInScope requires a location"))?;
                    let symbols = op
                        .get_symbols_in_scope(location, p.meaning)
                        .map_err(checker_error)?;
                    setup.symbol_responses(op, &symbols)
                })?)
            }
            Params::GetSignaturesOfType(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let ty = Self::resolve_type(setup, op, p.r#type)?;
                    let kind = if p.kind == 1 {
                        SignatureKind::Construct
                    } else {
                        SignatureKind::Call
                    };
                    let signatures = op.get_signatures_of_type(ty, kind).map_err(checker_error)?;
                    setup.signature_responses(op, &signatures)
                })?)
            }
            Params::GetResolvedSignature(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let node = resolve_node_handle(setup.program, &p.location)?;
                    let signature = op.get_resolved_signature(node).map_err(checker_error)?;
                    setup.signature_response(op, signature)
                })?)
            }
            Params::GetTypeAtLocation(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let node = resolve_node_handle(setup.program, &p.location)?;
                    let ty = op.get_type_at_location(node).map_err(checker_error)?;
                    setup.type_response(op, ty)
                })?)
            }
            Params::GetTypeAtLocations(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let mut results: Vec<Option<Box<TypeResponse>>> =
                        Vec::with_capacity(p.locations.len());
                    for location in &p.locations {
                        let node = resolve_node_handle(setup.program, location)?;
                        let ty = op.get_type_at_location(node).map_err(checker_error)?;
                        results.push(Some(Box::new(setup.type_response(op, ty)?)));
                    }
                    Ok(results)
                })?)
            }
            Params::GetTypeAtPosition(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let Some(node) = Self::node_at_position(setup, &p.file, p.position)? else {
                        return Ok(None);
                    };
                    let ty = op.get_type_at_location(node).map_err(checker_error)?;
                    Ok(Some(setup.type_response(op, ty)?))
                })?)
            }
            Params::GetTypesAtPositions(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let mut results: Vec<Option<Box<TypeResponse>>> =
                        Vec::with_capacity(p.positions.len());
                    for position in &p.positions {
                        let Some(node) = Self::node_at_position(setup, &p.file, *position)? else {
                            results.push(None);
                            continue;
                        };
                        let ty = op.get_type_at_location(node).map_err(checker_error)?;
                        results.push(Some(Box::new(setup.type_response(op, ty)?)));
                    }
                    Ok(results)
                })?)
            }
            Params::GetParentOfSymbol(p) => Response::json(self.symbol_property(
                p.snapshot,
                &p.project,
                p.symbol,
                |op, symbol| Ok(op.symbol(symbol).map_err(checker_error)?.parent()),
            )?),
            // The pin answers the merged export symbol, or the symbol itself
            // when it has none (`GetExportSymbolOfSymbol`), never null.
            Params::GetExportSymbolOfSymbol(p) => Response::json(self.symbol_property(
                p.snapshot,
                &p.project,
                p.symbol,
                |op, symbol| {
                    Ok(Some(
                        op.get_export_symbol_of_symbol(symbol)
                            .map_err(checker_error)?
                            .id(),
                    ))
                },
            )?),
            Params::GetMembersOfSymbol(p) => Response::json(self.symbol_table_property(
                p.snapshot,
                &p.project,
                p.symbol,
                |op, symbol| Ok(op.symbol(symbol).map_err(checker_error)?.members()),
            )?),
            Params::GetExportsOfSymbol(p) => Response::json(self.symbol_table_property(
                p.snapshot,
                &p.project,
                p.symbol,
                |op, symbol| Ok(op.symbol(symbol).map_err(checker_error)?.exports()),
            )?),
            Params::GetSymbolOfType(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let ty = Self::resolve_type(setup, op, p.r#type)?;
                    match op.type_symbol(ty).map_err(checker_error)? {
                        Some(symbol) => {
                            let symbol = op.symbol_ref(symbol).map_err(checker_error)?;
                            Ok(Some(setup.symbol_response(op, symbol)?))
                        }
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetTargetOfType(p) => Response::json(self.type_property(
                p.snapshot,
                &p.project,
                p.r#type,
                |op, ty| op.type_target(ty).map_err(checker_error),
            )?),
            Params::GetFreshTypeOfType(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        op.fresh_type_of_literal_type(ty)
                            .map(Some)
                            .map_err(checker_error)
                    })?,
                )
            }
            Params::GetRegularTypeOfType(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        op.regular_type_of_literal_type(ty)
                            .map(Some)
                            .map_err(checker_error)
                    })?,
                )
            }
            Params::GetTypesOfType(p) => Response::json(self.type_array_property(
                p.snapshot,
                &p.project,
                p.r#type,
                |op, ty| op.type_parts(ty).map_err(checker_error),
            )?),
            Params::GetTypeParametersOfType(p) => Response::json(self.type_array_property(
                p.snapshot,
                &p.project,
                p.r#type,
                |op, ty| {
                    Ok(op
                        .interface_type_parameters(ty)
                        .map_err(checker_error)?
                        .map(|parameters| parameters.type_parameters().to_vec())
                        .unwrap_or_default())
                },
            )?),
            Params::GetOuterTypeParametersOfType(p) => Response::json(self.type_array_property(
                p.snapshot,
                &p.project,
                p.r#type,
                |op, ty| {
                    Ok(op
                        .interface_type_parameters(ty)
                        .map_err(checker_error)?
                        .map(|parameters| parameters.outer().to_vec())
                        .unwrap_or_default())
                },
            )?),
            Params::GetLocalTypeParametersOfType(p) => Response::json(self.type_array_property(
                p.snapshot,
                &p.project,
                p.r#type,
                |op, ty| {
                    Ok(op
                        .interface_type_parameters(ty)
                        .map_err(checker_error)?
                        .map(|parameters| parameters.local().to_vec())
                        .unwrap_or_default())
                },
            )?),
            Params::GetAliasTypeArgumentsOfType(p) => Response::json(self.type_array_property(
                p.snapshot,
                &p.project,
                p.r#type,
                |op, ty| {
                    Ok(op
                        .type_alias(ty)
                        .map_err(checker_error)?
                        .map(|(_, arguments)| arguments)
                        .unwrap_or_default())
                },
            )?),
            Params::GetAliasSymbolOfType(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let ty = Self::resolve_type(setup, op, p.r#type)?;
                    match op.type_alias(ty).map_err(checker_error)? {
                        Some((symbol, _)) => {
                            let symbol = op.symbol_ref(symbol).map_err(checker_error)?;
                            Ok(Some(setup.symbol_response(op, symbol)?))
                        }
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetObjectTypeOfType(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        Ok(op
                            .indexed_access_parts(ty)
                            .map_err(checker_error)?
                            .map(|(object, _)| object))
                    })?,
                )
            }
            Params::GetIndexTypeOfType(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        Ok(op
                            .indexed_access_parts(ty)
                            .map_err(checker_error)?
                            .map(|(_, index)| index))
                    })?,
                )
            }
            Params::GetCheckTypeOfType(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        Ok(op
                            .conditional_parts(ty)
                            .map_err(checker_error)?
                            .map(|(check, _)| check))
                    })?,
                )
            }
            Params::GetExtendsTypeOfType(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        Ok(op
                            .conditional_parts(ty)
                            .map_err(checker_error)?
                            .map(|(_, extends)| extends))
                    })?,
                )
            }
            Params::GetBaseTypeOfType(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        Ok(op
                            .substitution_parts(ty)
                            .map_err(checker_error)?
                            .map(|(base, _)| base))
                    })?,
                )
            }
            Params::GetConstraintOfType(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        Ok(op
                            .substitution_parts(ty)
                            .map_err(checker_error)?
                            .map(|(_, constraint)| constraint))
                    })?,
                )
            }
            Params::GetTrueTypeOfConditionalType(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        op.get_true_type_of_conditional_type(ty)
                            .map(Some)
                            .map_err(checker_error)
                    })?,
                )
            }
            Params::GetFalseTypeOfConditionalType(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        op.get_false_type_of_conditional_type(ty)
                            .map(Some)
                            .map_err(checker_error)
                    })?,
                )
            }
            Params::GetTypeParametersOfSignature(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let signature = setup.registry.resolve_signature(op, p.signature)?;
                    let types = op
                        .signature_type_parameters(signature)
                        .map_err(checker_error)?;
                    if types.is_empty() {
                        return Ok(None);
                    }
                    Ok(Some(setup.type_responses(op, &types)?))
                })?)
            }
            Params::GetParametersOfSignature(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let signature = setup.registry.resolve_signature(op, p.signature)?;
                    let symbols = op.signature_parameters(signature).map_err(checker_error)?;
                    if symbols.is_empty() {
                        return Ok(None);
                    }
                    Ok(Some(setup.symbol_responses(op, &symbols)?))
                })?)
            }
            Params::GetThisParameterOfSignature(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let signature = setup.registry.resolve_signature(op, p.signature)?;
                    match op
                        .signature_this_parameter(signature)
                        .map_err(checker_error)?
                    {
                        Some(symbol) => Ok(Some(setup.symbol_response(op, symbol)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetTargetOfSignature(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let signature = setup.registry.resolve_signature(op, p.signature)?;
                    match op.signature_target(signature).map_err(checker_error)? {
                        Some(target) => Ok(Some(setup.signature_response(op, target)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetContextualType(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let node = resolve_node_handle(setup.program, &p.location)?;
                    match op.get_contextual_type(node, 0).map_err(checker_error)? {
                        Some(ty) => Ok(Some(setup.type_response(op, ty)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetBaseTypeOfLiteralType(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        op.get_base_type_of_literal_type(ty)
                            .map(Some)
                            .map_err(checker_error)
                    })?,
                )
            }
            Params::GetNonNullableType(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        op.get_non_nullable_type(ty)
                            .map(Some)
                            .map_err(checker_error)
                    })?,
                )
            }
            Params::GetTypeFromTypeNode(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let node = resolve_node_handle(setup.program, &p.location)?;
                    let ty = op.get_type_from_type_node(node).map_err(checker_error)?;
                    setup.type_response(op, ty)
                })?)
            }
            Params::GetWidenedType(p) => Response::json(self.type_property(
                p.snapshot,
                &p.project,
                p.r#type,
                |op, ty| op.get_widened_type(ty).map(Some).map_err(checker_error),
            )?),
            Params::GetParameterType(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let signature = setup.registry.resolve_signature(op, p.signature)?;
                    let index = usize::try_from(p.index)
                        .map_err(|_| client_error("invalid parameter index"))?;
                    let ty = op
                        .get_type_at_position(signature, index)
                        .map_err(checker_error)?;
                    setup.type_response(op, ty)
                })?)
            }
            Params::GetTypeParameterAtPosition(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let signature = setup.registry.resolve_signature(op, p.signature)?;
                    let index = usize::try_from(p.index)
                        .map_err(|_| client_error("invalid parameter index"))?;
                    let ty = op
                        .get_type_parameter_at_position(signature, index)
                        .map_err(checker_error)?;
                    setup.type_response(op, ty)
                })?)
            }
            Params::IsArrayLikeType(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let ty = Self::resolve_type(setup, op, p.r#type)?;
                    op.is_array_like_type(ty).map_err(checker_error)
                })?)
            }
            Params::IsArrayType(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let ty = Self::resolve_type(setup, op, p.r#type)?;
                    op.is_array_type(ty).map_err(checker_error)
                })?)
            }
            Params::IsTypeAssignableTo(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let source = Self::resolve_type(setup, op, p.source)?;
                    let target = Self::resolve_type(setup, op, p.target)?;
                    op.is_type_assignable_to(source, target)
                        .map_err(checker_error)
                })?)
            }
            Params::IsContextSensitive(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let node = resolve_node_handle(setup.program, &p.location)?;
                    op.is_context_sensitive(node).map_err(checker_error)
                })?)
            }
            Params::IsReadonlySymbol(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let symbol = Self::resolve_symbol(setup, op, p.symbol)?;
                    op.is_readonly_symbol(symbol).map_err(checker_error)
                })?)
            }
            Params::GetShorthandAssignmentValueSymbol(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let node = resolve_node_handle(setup.program, &p.location)?;
                    match op
                        .get_shorthand_assignment_value_symbol(Some(node))
                        .map_err(checker_error)?
                    {
                        Some(symbol) => Ok(Some(setup.symbol_response(op, symbol)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetTypeOfSymbolAtLocation(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let symbol = Self::resolve_symbol(setup, op, p.symbol)?;
                    let node = resolve_node_handle(setup.program, &p.location)?;
                    let ty = op
                        .get_type_of_symbol_at_location(symbol, Some(node))
                        .map_err(checker_error)?;
                    setup.type_response(op, ty)
                })?)
            }
            Params::TypeToTypeNode(p) => {
                return self
                    .with_checker_response(p.snapshot, &p.project, |setup, op| {
                        let ty = Self::resolve_type(setup, op, p.r#type)?;
                        let enclosing = if p.location.0.is_empty() {
                            None
                        } else {
                            Some(resolve_node_handle(setup.program, &p.location)?)
                        };
                        let mut builder = op.node_builder();
                        let node = builder
                            .type_to_type_node(ty, enclosing, p.flags as u32, 0)
                            .map_err(checker_error)?;
                        self.encoded_node_response(builder.view(), node, "type node")
                    })
                    .map(Some)
                    .map_err(Into::into)
            }
            Params::SignatureToSignatureDeclaration(p) => {
                return self
                    .with_checker_response(p.snapshot, &p.project, |setup, op| {
                        let signature = setup.registry.resolve_signature(op, p.signature)?;
                        let enclosing = if p.location.0.is_empty() {
                            None
                        } else {
                            Some(resolve_node_handle(setup.program, &p.location)?)
                        };
                        let kind = u16::try_from(p.kind)
                            .ok()
                            .and_then(tsr_ast::SyntaxKind::from_u16)
                            .ok_or_else(|| client_error("invalid syntax kind"))?;
                        let mut builder = op.node_builder();
                        let node = builder
                            .signature_to_signature_declaration(
                                signature,
                                kind,
                                tsr_checker::BuilderRequest {
                                    enclosing,
                                    flags: p.flags as u32,
                                    internal_flags: 0,
                                },
                            )
                            .map_err(checker_error)?;
                        self.encoded_node_response(builder.view(), node, "signature declaration")
                    })
                    .map(Some)
                    .map_err(Into::into)
            }
            Params::TypeToString(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let ty = Self::resolve_type(setup, op, p.r#type)?;
                    let enclosing = if p.location.0.is_empty() {
                        None
                    } else {
                        Some(resolve_node_handle(setup.program, &p.location)?)
                    };
                    let flags = if p.flags == 0 {
                        tsr_checker::type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                        | tsr_checker::type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE
                    } else {
                        p.flags as u32
                    };
                    Ok(text(
                        op.type_to_string_ex(ty, enclosing, flags, None)
                            .map_err(checker_error)?
                            .as_bytes(),
                    ))
                })?)
            }
            Params::GetReturnTypeOfSignature(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let signature = setup.registry.resolve_signature(op, p.signature)?;
                    let ty = op
                        .get_return_type_of_signature(signature)
                        .map_err(checker_error)?;
                    setup.type_response(op, ty)
                })?)
            }
            Params::GetRestTypeOfSignature(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let signature = setup.registry.resolve_signature(op, p.signature)?;
                    let ty = op
                        .get_rest_type_of_signature(signature)
                        .map_err(checker_error)?;
                    setup.type_response(op, ty)
                })?)
            }
            Params::GetTypePredicateOfSignature(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let signature = setup.registry.resolve_signature(op, p.signature)?;
                    match op
                        .get_type_predicate_of_signature(signature)
                        .map_err(checker_error)?
                    {
                        Some(predicate) => Ok(Some(setup.type_predicate_response(op, predicate)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetBaseTypes(p) => Response::json(self.type_array_property(
                p.snapshot,
                &p.project,
                p.r#type,
                |op, ty| op.get_base_types(ty).map_err(checker_error),
            )?),
            Params::GetPropertiesOfType(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let ty = Self::resolve_type(setup, op, p.r#type)?;
                    let properties = op.get_properties_of_type(ty).map_err(checker_error)?;
                    if properties.is_empty() {
                        return Ok(None);
                    }
                    Ok(Some(setup.symbol_responses(op, &properties)?))
                })?)
            }
            Params::GetApparentPropertiesOfType(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let ty = Self::resolve_type(setup, op, p.r#type)?;
                    let properties = op.get_apparent_properties(ty).map_err(checker_error)?;
                    setup.symbol_responses(op, &properties)
                })?)
            }
            Params::GetApparentType(p) => Response::json(self.type_property(
                p.snapshot,
                &p.project,
                p.r#type,
                |op, ty| op.get_apparent_type(ty).map(Some).map_err(checker_error),
            )?),
            Params::GetReducedType(p) => Response::json(self.type_property(
                p.snapshot,
                &p.project,
                p.r#type,
                |op, ty| op.get_reduced_type(ty).map(Some).map_err(checker_error),
            )?),
            Params::GetIndexInfosOfType(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let ty = Self::resolve_type(setup, op, p.r#type)?;
                    let infos = op.get_index_infos_of_type(ty).map_err(checker_error)?;
                    if infos.is_empty() {
                        return Ok(None);
                    }
                    let mut results = Vec::with_capacity(infos.len());
                    for info in infos {
                        results.push(Some(Box::new(setup.index_info_response(op, info)?)));
                    }
                    Ok(Some(results))
                })?)
            }
            Params::GetConstraintOfTypeParameter(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        op.get_constraint_of_type_parameter(ty)
                            .map_err(checker_error)
                    })?,
                )
            }
            Params::GetDefaultFromTypeParameter(p) => {
                Response::json(
                    self.type_property(p.snapshot, &p.project, p.r#type, |op, ty| {
                        op.get_default_from_type_parameter(ty)
                            .map_err(checker_error)
                    })?,
                )
            }
            Params::GetBaseConstraintOfType(p) => Response::json(self.type_property(
                p.snapshot,
                &p.project,
                p.r#type,
                |op, ty| op.get_base_constraint_of_type(ty).map_err(checker_error),
            )?),
            Params::GetPropertyOfType(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let ty = Self::resolve_type(setup, op, p.r#type)?;
                    match op
                        .get_property_of_type(ty, p.name.as_bytes())
                        .map_err(checker_error)?
                    {
                        Some(symbol) => Ok(Some(setup.symbol_response(op, symbol)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetTypeArguments(p) => Response::json(self.type_array_property(
                p.snapshot,
                &p.project,
                p.r#type,
                |op, ty| op.get_type_arguments(ty).map_err(checker_error),
            )?),
            Params::GetSignatureFromDeclaration(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let node = resolve_node_handle(setup.program, &p.location)?;
                    let signature = op
                        .get_signature_from_declaration(node)
                        .map_err(checker_error)?;
                    setup.signature_response(op, signature)
                })?)
            }
            Params::GetExportSpecifierLocalTarget(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let node = resolve_node_handle(setup.program, &p.location)?;
                    match op
                        .get_export_specifier_local_target_symbol(node)
                        .map_err(checker_error)?
                    {
                        Some(symbol) => Ok(Some(setup.symbol_response(op, symbol)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetAliasedSymbol(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let symbol = Self::resolve_symbol(setup, op, p.symbol)?;
                    let aliased = op.get_aliased_symbol(symbol).map_err(checker_error)?;
                    setup.symbol_response(op, aliased)
                })?)
            }
            Params::GetImmediateAliasedSymbol(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let symbol = Self::resolve_symbol(setup, op, p.symbol)?;
                    match op
                        .get_immediate_aliased_symbol(symbol)
                        .map_err(checker_error)?
                    {
                        Some(aliased) => Ok(Some(setup.symbol_response(op, aliased)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetTargetSymbol(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let symbol = Self::resolve_symbol(setup, op, p.symbol)?;
                    let target = op.get_target_symbol(symbol).map_err(checker_error)?;
                    setup.symbol_response(op, target)
                })?)
            }
            Params::GetFullyQualifiedName(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let symbol = Self::resolve_symbol(setup, op, p.symbol)?;
                    Ok(text(
                        op.get_fully_qualified_name(symbol)
                            .map_err(checker_error)?
                            .as_bytes(),
                    ))
                })?)
            }
            Params::GetExportsOfModule(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let symbol = Self::resolve_symbol(setup, op, p.symbol)?;
                    let mut exports = op.get_exports_of_module(symbol).map_err(checker_error)?;
                    if exports.is_empty() {
                        return Ok(None);
                    }
                    exports.sort_by(|a, b| {
                        op.compare_symbols(Some(*a), Some(*b))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    Ok(Some(setup.symbol_responses(op, &exports)?))
                })?)
            }
            Params::GetMemberInModuleExports(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let symbol = Self::resolve_symbol(setup, op, p.symbol)?;
                    match op
                        .try_get_member_in_module_exports(p.name.as_bytes(), symbol)
                        .map_err(checker_error)?
                    {
                        Some(member) => Ok(Some(setup.symbol_response(op, member)?)),
                        None => Ok(None),
                    }
                })?)
            }
            Params::GetAnyType(p) => {
                Response::json(self.intrinsic(p.snapshot, &p.project, |op| op.get_any_type())?)
            }
            Params::GetStringType(p) => {
                Response::json(self.intrinsic(p.snapshot, &p.project, |op| op.get_string_type())?)
            }
            Params::GetNumberType(p) => {
                Response::json(self.intrinsic(p.snapshot, &p.project, |op| op.get_number_type())?)
            }
            Params::GetBooleanType(p) => {
                Response::json(self.intrinsic(p.snapshot, &p.project, |op| op.get_boolean_type())?)
            }
            Params::GetVoidType(p) => {
                Response::json(self.intrinsic(p.snapshot, &p.project, |op| op.get_void_type())?)
            }
            Params::GetUndefinedType(p) => {
                Response::json(
                    self.intrinsic(p.snapshot, &p.project, |op| op.get_undefined_type())?,
                )
            }
            Params::GetNullType(p) => {
                Response::json(self.intrinsic(p.snapshot, &p.project, |op| op.get_null_type())?)
            }
            Params::GetNeverType(p) => {
                Response::json(self.intrinsic(p.snapshot, &p.project, |op| op.get_never_type())?)
            }
            Params::GetUnknownType(p) => {
                Response::json(self.intrinsic(p.snapshot, &p.project, |op| op.get_unknown_type())?)
            }
            Params::GetBigIntType(p) => {
                Response::json(self.intrinsic(p.snapshot, &p.project, |op| op.get_big_int_type())?)
            }
            Params::GetESSymbolType(p) => {
                Response::json(
                    self.intrinsic(p.snapshot, &p.project, |op| op.get_es_symbol_type())?,
                )
            }
            Params::GetNonPrimitiveType(p) => {
                Response::json(
                    self.intrinsic(p.snapshot, &p.project, |op| op.get_non_primitive_type())?,
                )
            }
            Params::GetWellKnownSymbols(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let registries = &setup.data.registries;
                    let unknown = registries
                        .register_symbol(
                            &setup.registry,
                            op,
                            op.get_unknown_symbol().map_err(checker_error)?,
                            &setup.project,
                        )?
                        .0;
                    let undefined = registries
                        .register_symbol(
                            &setup.registry,
                            op,
                            op.get_undefined_symbol().map_err(checker_error)?,
                            &setup.project,
                        )?
                        .0;
                    let arguments = registries
                        .register_symbol(
                            &setup.registry,
                            op,
                            op.get_arguments_symbol().map_err(checker_error)?,
                            &setup.project,
                        )?
                        .0;
                    Ok(WellKnownSymbolsResponse {
                        unknown,
                        undefined,
                        arguments,
                    })
                })?)
            }
            Params::GetWellKnownSignatures(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    Ok(WellKnownSignaturesResponse {
                        unknown: setup
                            .registry
                            .register_signature(op, op.get_unknown_signature())?,
                    })
                })?)
            }
            Params::PrintNode(p) => Response::json(Self::handle_print_node(&p)?),
            Params::GetSyntacticDiagnostics(p) => Response::json(self.handle_get_diagnostics(
                ctx,
                &p,
                DiagnosticKind::Syntactic,
                files_named,
            )?),
            Params::GetBindDiagnostics(p) => Response::json(self.handle_get_diagnostics(
                ctx,
                &p,
                DiagnosticKind::Bind,
                files_named,
            )?),
            Params::GetSemanticDiagnostics(p) => Response::json(self.handle_get_diagnostics(
                ctx,
                &p,
                DiagnosticKind::Semantic,
                files_named,
            )?),
            Params::GetSuggestionDiagnostics(p) => Response::json(self.handle_get_diagnostics(
                ctx,
                &p,
                DiagnosticKind::Suggestion,
                files_named,
            )?),
            Params::GetDeclarationDiagnostics(p) => Response::json(self.handle_get_diagnostics(
                ctx,
                &p,
                DiagnosticKind::Declaration,
                files_named,
            )?),
            Params::GetConfigFileParsingDiagnostics(p) => {
                Response::json(self.handle_get_config_file_parsing_diagnostics(&p)?)
            }
            Params::GetProgramDiagnostics(p) => {
                Response::json(self.handle_get_program_diagnostics(&p)?)
            }
            Params::GetGlobalDiagnostics(p) => {
                Response::json(self.handle_get_global_diagnostics(ctx, &p)?)
            }
            Params::Emit(p) => Response::json(self.handle_emit(&p)?),
            Params::EmitToString(p) => Response::json(self.handle_emit_to_string(&p)?),
            Params::GetJavaScriptEmit(p) => Response::json(self.handle_selected_files_emit(
                &p,
                tsr_compiler::EmitOnly::Js,
                files_named,
            )?),
            Params::GetDeclarationEmit(p) => Response::json(self.handle_selected_files_emit(
                &p,
                tsr_compiler::EmitOnly::Dts,
                files_named,
            )?),
            Params::FormatNodeForInsertion(p) => {
                Response::json(self.handle_format_node_for_insertion(&p)?)
            }
            Params::GetCompletionsAtPosition(p) => {
                Response::json(self.handle_get_completions_at_position(&p)?)
            }
            Params::GetJSDocTags(p) => Response::json(self.handle_get_jsdoc_tags(&p)?),
            Params::GetDocumentationComment(p) => {
                Response::json(self.handle_get_documentation_comment(&p)?)
            }
            Params::GetSignatureUsages(p) => Response::json(self.handle_get_signature_usages(&p)?),
            Params::GetImportAdderEdits(p) => {
                Response::json(self.handle_get_import_adder_edits(&p)?)
            }
            // port: tsc/internal/api/session.go:Session.handleGetConstantValue
            Params::GetConstantValue(p) => {
                Response::json(self.with_checker(p.snapshot, &p.project, |setup, op| {
                    let node = resolve_node_handle(setup.program, &p.location)?;
                    let value = op.constant_value(node).map_err(checker_error)?;
                    Ok(value.and_then(|value| {
                        literal_value_json(&match value {
                            tsr_printer::emit_resolver::ConstantValue::Number(number) => {
                                tsr_checker::LiteralValue::Number(number)
                            }
                            tsr_printer::emit_resolver::ConstantValue::String(text) => {
                                tsr_checker::LiteralValue::String(text)
                            }
                        })
                    }))
                })?)
            }
            Params::GetReferencedSymbolsForNode(p) => {
                Response::json(self.handle_get_referenced_symbols_for_node(&p)?)
            }
            Params::GetReferencesToSymbolInFile(p) => {
                Response::json(self.handle_get_references_to_symbol_in_file(&p)?)
            }
            // Decision 8: the profiling methods answer the explicit unsupported
            // error the LSP server gives its profiling methods.
            Params::StartCPUProfile(_) | Params::StopCPUProfile | Params::SaveHeapProfile(_) => {
                return Err(SessionError::Other(format!("method not implemented: {method}")).into())
            }
            _ => return Err(crate::server::unsupported(method)),
        };
        Ok(Some(response))
    }

    /// port: tsc/internal/api/session.go:Session.resolveTypePropertyOfType
    fn type_property(
        &self,
        snapshot: SnapshotId,
        project: &ProjectId,
        handle: crate::proto::TypeId,
        getter: impl FnOnce(&mut Operation<'_>, TypeRef) -> SessionResult<Option<TypeRef>>,
    ) -> SessionResult<Committed> {
        self.with_checker(snapshot, project, |setup, op| {
            let ty = Self::resolve_type(setup, op, handle)?;
            match getter(op, ty)? {
                Some(result) => Ok(Some(setup.type_response(op, result)?)),
                None => Ok(None),
            }
        })
    }

    /// port: tsc/internal/api/session.go:Session.resolveTypeArrayPropertyOfType
    fn type_array_property(
        &self,
        snapshot: SnapshotId,
        project: &ProjectId,
        handle: crate::proto::TypeId,
        getter: impl FnOnce(&mut Operation<'_>, TypeRef) -> SessionResult<Vec<TypeRef>>,
    ) -> SessionResult<Committed> {
        self.with_checker(snapshot, project, |setup, op| {
            let ty = Self::resolve_type(setup, op, handle)?;
            let types = getter(op, ty)?;
            if types.is_empty() {
                return Ok(None);
            }
            Ok(Some(setup.type_responses(op, &types)?))
        })
    }

    /// port: tsc/internal/api/session.go:Session.resolveSymbolPropertyOfSymbol
    fn symbol_property(
        &self,
        snapshot: SnapshotId,
        project: &ProjectId,
        handle: crate::proto::SymbolId,
        getter: impl FnOnce(&mut Operation<'_>, SymbolRef) -> SessionResult<Option<tsr_arena::SymbolId>>,
    ) -> SessionResult<Committed> {
        self.with_checker(snapshot, project, |setup, op| {
            let symbol = Self::resolve_symbol(setup, op, handle)?;
            match getter(op, symbol)? {
                Some(result) => {
                    let result = op.symbol_ref(result).map_err(checker_error)?;
                    Ok(Some(setup.symbol_response(op, result)?))
                }
                None => Ok(None),
            }
        })
    }

    /// Members or exports of a symbol in the checker's canonical order.
    /// port: tsc/internal/api/session.go:Session.resolveSymbolTablePropertyOfSymbol
    fn symbol_table_property(
        &self,
        snapshot: SnapshotId,
        project: &ProjectId,
        handle: crate::proto::SymbolId,
        getter: impl FnOnce(
            &mut Operation<'_>,
            SymbolRef,
        ) -> SessionResult<Option<tsr_ast::SymbolTableId>>,
    ) -> SessionResult<Committed> {
        self.with_checker(snapshot, project, |setup, op| {
            let symbol = Self::resolve_symbol(setup, op, handle)?;
            let Some(table) = getter(op, symbol)? else {
                return Ok(None);
            };
            let mut symbols: Vec<SymbolRef> = {
                let table = op.symbol_table(table).map_err(checker_error)?;
                let ids: Vec<tsr_arena::SymbolId> = table.iter().filter_map(|(_, id)| id).collect();
                ids.into_iter()
                    .map(|id| op.symbol_ref(id).map_err(checker_error))
                    .collect::<SessionResult<_>>()?
            };
            if symbols.is_empty() {
                return Ok(None);
            }
            symbols.sort_by(|a, b| {
                op.compare_symbols(Some(*a), Some(*b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            Ok(Some(setup.symbol_responses(op, &symbols)?))
        })
    }

    fn intrinsic(
        &self,
        snapshot: SnapshotId,
        project: &ProjectId,
        getter: impl FnOnce(&Operation<'_>) -> TypeRef,
    ) -> SessionResult<Committed> {
        self.with_checker(snapshot, project, |setup, op| {
            let ty = getter(op);
            setup.type_response(op, ty)
        })
    }

    /// port: tsc/internal/api/session.go:Session.handlePrintNode
    fn handle_print_node(params: &crate::proto::PrintNodeParams) -> SessionResult<String> {
        let data = super::responses::base64_decode(&params.data).map_err(|at| {
            client_error(format!(
                "invalid base64 data: illegal base64 data at input byte {at}"
            ))
        })?;
        crate::print_node(
            &data,
            crate::PrintNodeOptions {
                preserve_source_newlines: params.preserve_source_newlines,
                never_ascii_escape: params.never_ascii_escape,
                terminate_unterminated_literals: params.terminate_unterminated_literals,
            },
            &tsr_arena::Counters::new(),
        )
        .map(|printed| text(&printed))
        .map_err(|error| client_error(format!("failed to decode AST: {error}")))
    }
}

impl DocumentIdentifier {
    /// The pin's `String()`: the file name or the URI.
    pub fn display(&self) -> String {
        if !self.uri.is_empty() {
            return self.uri.clone();
        }
        self.file_name.clone()
    }
}
