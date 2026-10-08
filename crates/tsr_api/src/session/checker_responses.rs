//! Symbol, type and signature responses: each registers the handles it
//! names in the snapshot's registries so the client can ask about them.
//! port: tsc/internal/api/proto.go
use super::handles::{node_handle, CheckerSetup};
use super::SessionResult;
use crate::proto::{
    IndexInfoResponse, NodeHandle, SignatureResponse, SymbolId, SymbolResponse, TypeId,
    TypePredicateResponse, TypeResponse,
};
use tsr_checker::{
    object_flags, type_flags, LiteralValue, Operation, SignatureRef, SymbolRef, TypeRef,
};

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn checker_error(error: impl std::fmt::Display) -> super::SessionError {
    super::SessionError::Other(format!("{error}"))
}

impl CheckerSetup<'_> {
    /// Registers `symbol` and names its parent and export symbol, which get
    /// handles of their own so the client can resolve them.
    /// port: tsc/internal/api/session.go:snapshotData.newSymbolResponse
    pub fn symbol_response(
        &self,
        operation: &mut Operation<'_>,
        symbol: SymbolRef,
    ) -> SessionResult<SymbolResponse> {
        let (id, project) =
            self.data
                .registries
                .register_symbol(operation, symbol, &self.project)?;
        let (name, flags, check_flags, value_declaration, parent, export_symbol) = {
            let read = operation.symbol(symbol).map_err(checker_error)?;
            (
                tsr_ast::escape_symbol_name(read.name_bytes()).into_owned(),
                read.flags(),
                read.check_flags(),
                read.value_declaration(),
                read.parent(),
                read.export_symbol(),
            )
        };
        let declarations = operation
            .symbol_declarations(symbol)
            .map_err(checker_error)?
            .iter()
            .flatten()
            .collect::<Vec<_>>();
        let mut response = SymbolResponse {
            id,
            project,
            name: text(&name),
            flags,
            check_flags,
            declarations: Vec::new(),
            value_declaration: NodeHandle::default(),
            parent: SymbolId::default(),
            export_symbol: SymbolId::default(),
        };
        for declaration in declarations {
            response
                .declarations
                .push(node_handle(operation, declaration)?);
        }
        if let Some(declaration) = value_declaration {
            response.value_declaration = node_handle(operation, declaration)?;
        }
        if let Some(parent) = parent {
            response.parent = self.symbol_handle_of(operation, parent)?;
        }
        if let Some(export_symbol) = export_symbol {
            response.export_symbol = self.symbol_handle_of(operation, export_symbol)?;
        }
        Ok(response)
    }

    /// The handle of a symbol named by another response, registered so that
    /// the client can follow it.
    /// port: tsc/internal/api/proto.go:SymbolHandle
    pub fn symbol_handle_of(
        &self,
        operation: &Operation<'_>,
        symbol: tsr_arena::SymbolId,
    ) -> SessionResult<SymbolId> {
        let symbol = operation.symbol_ref(symbol).map_err(checker_error)?;
        Ok(self
            .data
            .registries
            .register_symbol(operation, symbol, &self.project)?
            .0)
    }

    /// port: tsc/internal/api/proto.go:symbolHandles
    pub fn symbol_handles(
        &self,
        operation: &Operation<'_>,
        symbols: &[SymbolRef],
    ) -> SessionResult<Vec<SymbolId>> {
        symbols
            .iter()
            .map(|symbol| {
                Ok(self
                    .data
                    .registries
                    .register_symbol(operation, *symbol, &self.project)?
                    .0)
            })
            .collect()
    }

    /// port: tsc/internal/api/proto.go:TypeHandle
    pub fn type_handle(&self, operation: &Operation<'_>, ty: TypeRef) -> SessionResult<TypeId> {
        self.registry.register_type(operation, ty)
    }

    /// port: tsc/internal/api/proto.go:typeHandles
    pub fn type_handles(
        &self,
        operation: &Operation<'_>,
        types: &[TypeRef],
    ) -> SessionResult<Vec<TypeId>> {
        types
            .iter()
            .map(|ty| self.type_handle(operation, *ty))
            .collect()
    }

    /// port: tsc/internal/api/session.go:snapshotData.newTypeResponse
    pub fn type_response(
        &self,
        operation: &mut Operation<'_>,
        ty: TypeRef,
    ) -> SessionResult<TypeResponse> {
        let id = self.type_handle(operation, ty)?;
        let flags = operation.type_flags(ty).map_err(checker_error)?;
        let mut response = TypeResponse {
            id,
            flags,
            ..TypeResponse::default()
        };
        if let Some(symbol) = operation.type_symbol(ty).map_err(checker_error)? {
            response.symbol = self.symbol_handle_of(operation, symbol)?;
        }
        if let Some((alias_symbol, arguments)) = operation.type_alias(ty).map_err(checker_error)? {
            response.alias_type_arguments = self.type_handles(operation, &arguments)?;
            response.alias_symbol = self.symbol_handle_of(operation, alias_symbol)?;
        }
        if flags & type_flags::FRESHABLE != 0 {
            if flags & type_flags::LITERAL != 0 {
                response.value =
                    literal_value_json(&operation.literal_value(ty).map_err(checker_error)?);
            }
            let fresh = operation
                .fresh_type_of_literal_type(ty)
                .map_err(checker_error)?;
            response.fresh_type = self.type_handle(operation, fresh)?;
            let regular = operation
                .regular_type_of_literal_type(ty)
                .map_err(checker_error)?;
            response.regular_type = self.type_handle(operation, regular)?;
        } else if flags & type_flags::OBJECT != 0 {
            let object_flags = operation.type_object_flags(ty).map_err(checker_error)?;
            response.object_flags = object_flags;
            response.is_tuple_type = operation.is_tuple_type(ty).map_err(checker_error)?;
            if object_flags & object_flags::REFERENCE != 0 {
                // Tuple metadata belongs to the tuple target, not to its
                // instantiations.
                let tuple = if operation.is_tuple_type_target(ty).map_err(checker_error)? {
                    operation.tuple_shape(ty).map_err(checker_error)?
                } else {
                    None
                };
                if let Some(tuple) = tuple {
                    response.element_flags = tuple
                        .element_flags()
                        .into_iter()
                        .map(crate::proto::ElementFlags)
                        .collect();
                    response.fixed_length = Some(Box::new(i64::from(tuple.fixed_length)));
                    response.tuple_readonly = Some(Box::new(tuple.readonly));
                    let labeled: Vec<_> = tuple
                        .element_infos
                        .iter()
                        .map(|info| info.labeled_declaration)
                        .collect();
                    if labeled.iter().any(Option::is_some) {
                        for declaration in labeled {
                            response
                                .labeled_element_declarations
                                .push(match declaration {
                                    Some(node) => node_handle(operation, node)?,
                                    None => NodeHandle::default(),
                                });
                        }
                    }
                }
                if let Some(target) = operation.type_target(ty).map_err(checker_error)? {
                    response.target = self.type_handle(operation, target)?;
                }
            }
            if object_flags & object_flags::CLASS_OR_INTERFACE != 0 {
                if let Some(parameters) = operation
                    .interface_type_parameters(ty)
                    .map_err(checker_error)?
                {
                    response.type_parameters =
                        self.type_handles(operation, parameters.type_parameters())?;
                    response.outer_type_parameters =
                        self.type_handles(operation, parameters.outer())?;
                    response.local_type_parameters =
                        self.type_handles(operation, parameters.local())?;
                }
            }
        } else if flags & type_flags::UNION_OR_INTERSECTION != 0 {
            // Constituents are fetched by a separate request.
        } else if flags & type_flags::INDEX != 0 {
            if let Some(target) = operation.type_target(ty).map_err(checker_error)? {
                response.target = self.type_handle(operation, target)?;
            }
        } else if flags & type_flags::INDEXED_ACCESS != 0 {
            if let Some((object, index)) =
                operation.indexed_access_parts(ty).map_err(checker_error)?
            {
                response.object_type = self.type_handle(operation, object)?;
                response.index_type = self.type_handle(operation, index)?;
            }
        } else if flags & type_flags::CONDITIONAL != 0 {
            if let Some((check, extends)) =
                operation.conditional_parts(ty).map_err(checker_error)?
            {
                response.check_type = self.type_handle(operation, check)?;
                response.extends_type = self.type_handle(operation, extends)?;
            }
        } else if flags & type_flags::SUBSTITUTION != 0 {
            if let Some((base, constraint)) =
                operation.substitution_parts(ty).map_err(checker_error)?
            {
                response.base_type = self.type_handle(operation, base)?;
                response.subst_constraint = self.type_handle(operation, constraint)?;
            }
        } else if flags & type_flags::TEMPLATE_LITERAL != 0 {
            if let Some(texts) = operation
                .template_literal_texts(ty)
                .map_err(checker_error)?
            {
                response.texts = texts.iter().map(|part| text(part.as_bytes())).collect();
            }
        } else if flags & type_flags::STRING_MAPPING != 0 {
            if let Some(target) = operation.type_target(ty).map_err(checker_error)? {
                response.target = self.type_handle(operation, target)?;
            }
        } else if flags & type_flags::TYPE_PARAMETER != 0 {
            response.is_this_type = operation
                .type_parameter_is_this(ty)
                .map_err(checker_error)?;
        } else if flags & type_flags::INTRINSIC != 0 {
            response.intrinsic_name = text(
                operation
                    .intrinsic_type_name(ty)
                    .map_err(checker_error)?
                    .as_bytes(),
            );
        }
        Ok(response)
    }

    pub fn type_responses(
        &self,
        operation: &mut Operation<'_>,
        types: &[TypeRef],
    ) -> SessionResult<Vec<Option<Box<TypeResponse>>>> {
        types
            .iter()
            .map(|ty| Ok(Some(Box::new(self.type_response(operation, *ty)?))))
            .collect()
    }

    pub fn symbol_responses(
        &self,
        operation: &mut Operation<'_>,
        symbols: &[SymbolRef],
    ) -> SessionResult<Vec<Option<Box<SymbolResponse>>>> {
        symbols
            .iter()
            .map(|symbol| Ok(Some(Box::new(self.symbol_response(operation, *symbol)?))))
            .collect()
    }

    /// port: tsc/internal/api/session.go:snapshotData.newSignatureResponse
    pub fn signature_response(
        &self,
        operation: &mut Operation<'_>,
        signature: SignatureRef,
    ) -> SessionResult<SignatureResponse> {
        let id = self.registry.register_signature(operation, signature)?;
        let mut response = SignatureResponse {
            id,
            flags: operation
                .signature_flags(signature)
                .map_err(checker_error)?,
            ..SignatureResponse::default()
        };
        if let Some(declaration) = operation
            .signature_declaration(signature)
            .map_err(checker_error)?
        {
            response.declaration = node_handle(operation, declaration.id())?;
        }
        let type_parameters = operation
            .signature_type_parameters(signature)
            .map_err(checker_error)?;
        response.type_parameters = self.type_handles(operation, &type_parameters)?;
        let parameters = operation
            .signature_parameters(signature)
            .map_err(checker_error)?;
        response.parameters = self.symbol_handles(operation, &parameters)?;
        if let Some(this_parameter) = operation
            .signature_this_parameter(signature)
            .map_err(checker_error)?
        {
            response.this_parameter = self
                .data
                .registries
                .register_symbol(operation, this_parameter, &self.project)?
                .0;
        }
        if let Some(target) = operation
            .signature_target(signature)
            .map_err(checker_error)?
        {
            response.target = self.registry.register_signature(operation, target)?;
        }
        Ok(response)
    }

    pub fn signature_responses(
        &self,
        operation: &mut Operation<'_>,
        signatures: &[SignatureRef],
    ) -> SessionResult<Vec<Option<Box<SignatureResponse>>>> {
        signatures
            .iter()
            .map(|signature| {
                Ok(Some(Box::new(
                    self.signature_response(operation, *signature)?,
                )))
            })
            .collect()
    }

    /// port: tsc/internal/api/session.go:Session.handleGetIndexInfosOfType
    pub fn index_info_response(
        &self,
        operation: &mut Operation<'_>,
        info: tsr_checker::IndexInfoRef,
    ) -> SessionResult<IndexInfoResponse> {
        let parts = operation.index_info_parts(info).map_err(checker_error)?;
        Ok(IndexInfoResponse {
            key_type: self.type_response(operation, parts.key_type)?,
            value_type: self.type_response(operation, parts.value_type)?,
            is_readonly: parts.is_readonly,
            declaration: match parts.declaration {
                Some(node) => node_handle(operation, node)?,
                None => NodeHandle::default(),
            },
        })
    }

    /// port: tsc/internal/api/session.go:Session.handleGetTypePredicateOfSignature
    pub fn type_predicate_response(
        &self,
        operation: &mut Operation<'_>,
        predicate: tsr_checker::TypePredicateRef,
    ) -> SessionResult<TypePredicateResponse> {
        let parts = operation
            .type_predicate_parts(predicate)
            .map_err(checker_error)?;
        Ok(TypePredicateResponse {
            kind: parts.kind as i32,
            parameter_index: parts.parameter_index,
            parameter_name: text(parts.parameter_name.as_bytes()),
            r#type: match parts.r#type {
                Some(ty) => Some(Box::new(self.type_response(operation, ty)?)),
                None => None,
            },
        })
    }
}

/// Strings and booleans as themselves, numbers as float64, bigints as their
/// signed decimal text, the rest null.
/// port: tsc/internal/api/proto.go:literalValueToJSON
pub fn literal_value_json(value: &LiteralValue) -> Option<tsr_json::RawValue> {
    let bytes = match value {
        LiteralValue::String(value) => {
            tsr_json::marshal(value, tsr_json::Options::default()).ok()?
        }
        LiteralValue::Number(value) => {
            let number: f64 = value.value();
            tsr_json::marshal(&number, tsr_json::Options::default()).ok()?
        }
        LiteralValue::Boolean(value) => {
            tsr_json::marshal(value, tsr_json::Options::default()).ok()?
        }
        LiteralValue::BigInt(value) => tsr_json::marshal(
            &tsr_jsstring::JsString::from_bytes(value.to_text()),
            tsr_json::Options::default(),
        )
        .ok()?,
        LiteralValue::ComputedEnum => return None,
    };
    Some(tsr_json::RawValue(bytes))
}
