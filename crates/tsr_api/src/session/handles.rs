//! The handle registries of a snapshot: symbols snapshot-wide with a dense
//! id per symbol identity and the project each was first seen in, types and
//! signatures per project bound to that project's API checker; node handles
//! `index.kind.path` over the encoder's node index tables; and the checker
//! setup a query starts from. Registry reads validate the owner and the
//! pool generation through the operation that performs them
//! (docs/design/symbols.md section 2.5).
//! port: tsc/internal/api/session.go
use super::{client_error, SessionError, SessionResult, SnapshotData};
use crate::proto::{NodeHandle, ProjectId, SignatureId, SymbolId, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tsr_arena::ArenaId;
use tsr_checker::{
    CheckerOwner, Operation, RetainedSignature, RetainedSymbol, RetainedType, SignatureRef,
    SymbolRef, TypeRef,
};
use tsr_compiler::Program;
use tsr_project::{CheckerSlot, PooledChecker, Project};

use super::checker_error;

/// The registries of one snapshot: the pin's `snapshotData` of
/// tsc/internal/api/session.go.
#[derive(Default)]
pub struct Registries {
    symbols: Mutex<SymbolRegistry>,
    projects: Mutex<HashMap<ProjectId, Arc<ProjectRegistry>>>,
}

#[derive(Default)]
struct SymbolRegistry {
    next: u64,
    /// The packed symbol identity (its arena and slot) to the dense id, so a
    /// symbol seen from two projects has one handle.
    by_identity: HashMap<(ArenaId, u32), u64>,
    entries: HashMap<u64, SymbolEntry>,
}

struct SymbolEntry {
    retained: RetainedSymbol,
    raw: tsr_arena::SymbolId,
    /// The project the symbol was first observed in; first writer wins.
    canonical: ProjectId,
}

/// A project's type and signature registries, bound to the API checker of
/// that project for the snapshot's lifetime: the pin's `projectRegistryData`
/// of tsc/internal/api/session.go.
pub struct ProjectRegistry {
    _lease: PooledChecker,
    owner: Arc<CheckerOwner>,
    types: Mutex<HashMap<u32, RetainedType>>,
    signatures: Mutex<HashMap<u32, RetainedSignature>>,
}

impl ProjectRegistry {
    fn new(project: &Project) -> SessionResult<Self> {
        let lease = project
            .pool()
            .acquire(CheckerSlot::Api)
            .map_err(|error| SessionError::Other(format!("{error}")))?;
        let owner = lease.owner().clone();
        Ok(Self {
            _lease: lease,
            owner,
            types: Mutex::default(),
            signatures: Mutex::default(),
        })
    }

    /// An operation on the project's API checker.
    pub fn operation(&self) -> SessionResult<Operation<'_>> {
        self.owner.operation().map_err(checker_error)
    }

    /// port: tsc/internal/api/session.go:snapshotData.registerType
    pub fn register_type(&self, operation: &Operation<'_>, ty: TypeRef) -> SessionResult<TypeId> {
        let id = ty.id();
        let mut types = self.types.lock().expect("type registry");
        if let std::collections::hash_map::Entry::Vacant(slot) = types.entry(id) {
            slot.insert(operation.retain_type(ty).map_err(checker_error)?);
        }
        Ok(TypeId(id))
    }

    /// port: tsc/internal/api/session.go:snapshotData.resolveTypeHandle
    pub fn resolve_type(
        &self,
        operation: &Operation<'_>,
        handle: TypeId,
    ) -> SessionResult<TypeRef> {
        if handle.0 == 0 {
            return Err(client_error("empty type handle"));
        }
        let retained = self
            .types
            .lock()
            .expect("type registry")
            .get(&handle.0)
            .cloned()
            .ok_or_else(|| {
                client_error(format!(
                    "type handle {} not found in project registry",
                    handle.0
                ))
            })?;
        operation.import_type(&retained).map_err(checker_error)
    }

    /// Signature ids are one-based on the wire: the checker's arena starts
    /// at zero and zero is the empty handle.
    /// port: tsc/internal/api/session.go:snapshotData.registerSignature
    pub fn register_signature(
        &self,
        operation: &Operation<'_>,
        signature: SignatureRef,
    ) -> SessionResult<SignatureId> {
        let id = signature.id();
        let mut signatures = self.signatures.lock().expect("signature registry");
        if let std::collections::hash_map::Entry::Vacant(slot) = signatures.entry(id) {
            slot.insert(
                operation
                    .retain_signature(signature)
                    .map_err(checker_error)?,
            );
        }
        Ok(SignatureId(u64::from(id) + 1))
    }

    /// port: tsc/internal/api/session.go:snapshotData.resolveSignatureHandle
    pub fn resolve_signature(
        &self,
        operation: &Operation<'_>,
        handle: SignatureId,
    ) -> SessionResult<SignatureRef> {
        if handle.0 == 0 {
            return Err(client_error("empty signature handle"));
        }
        let id = u32::try_from(handle.0 - 1).map_err(|_| {
            client_error(format!(
                "signature handle {} not found in project registry",
                handle.0
            ))
        })?;
        let retained = self
            .signatures
            .lock()
            .expect("signature registry")
            .get(&id)
            .cloned()
            .ok_or_else(|| {
                client_error(format!(
                    "signature handle {} not found in project registry",
                    handle.0
                ))
            })?;
        operation.import_signature(&retained).map_err(checker_error)
    }
}

impl Registries {
    /// The registry of `project`, created with the project's API checker on
    /// first use. Creation initializes a checker and runs outside the map's lock.
    /// port: tsc/internal/api/session.go:snapshotData.getOrCreateProjectRegistry
    pub fn project(
        &self,
        handle: &ProjectId,
        project: &Project,
    ) -> SessionResult<Arc<ProjectRegistry>> {
        if handle.0.is_empty() {
            return Err(SessionError::Other(
                "getOrCreateProjectRegistry: empty project ID".into(),
            ));
        }
        if let Some(registry) = self
            .projects
            .lock()
            .expect("project registries")
            .get(handle)
        {
            return Ok(registry.clone());
        }
        let created = Arc::new(ProjectRegistry::new(project)?);
        let mut projects = self.projects.lock().expect("project registries");
        Ok(projects.entry(handle.clone()).or_insert(created).clone())
    }

    /// The registry of a project when it exists; type and signature handles
    /// of a project without one are unknown by definition.
    pub fn existing_project(&self, handle: &ProjectId) -> Option<Arc<ProjectRegistry>> {
        self.projects
            .lock()
            .expect("project registries")
            .get(handle)
            .cloned()
    }

    /// Registers a symbol observed in `canonical` and returns its handle with
    /// the project it was first observed in.
    /// port: tsc/internal/api/session.go:snapshotData.registerSymbol
    pub fn register_symbol(
        &self,
        operation: &Operation<'_>,
        symbol: SymbolRef,
        canonical: &ProjectId,
    ) -> SessionResult<(SymbolId, ProjectId)> {
        assert!(
            !canonical.0.is_empty(),
            "registerSymbol requires a non-empty canonical project"
        );
        let raw = symbol.id();
        let key = (raw.arena(), raw.slot());
        let mut symbols = self.symbols.lock().expect("symbol registry");
        if let Some(id) = symbols.by_identity.get(&key).copied() {
            let entry = symbols.entries.get(&id).expect("registered symbol");
            return Ok((SymbolId(id), entry.canonical.clone()));
        }
        let retained = operation.retain_symbol_ref(symbol).map_err(checker_error)?;
        symbols.next += 1;
        let id = symbols.next;
        symbols.by_identity.insert(key, id);
        symbols.entries.insert(
            id,
            SymbolEntry {
                retained,
                raw,
                canonical: canonical.clone(),
            },
        );
        Ok((SymbolId(id), canonical.clone()))
    }

    /// The symbol of a handle as the operation's checker sees it: the
    /// retained reference when the checker is the one that registered it,
    /// otherwise the same source symbol read through this checker.
    /// port: tsc/internal/api/session.go:snapshotData.resolveSymbolHandle
    pub fn resolve_symbol(
        &self,
        operation: &Operation<'_>,
        handle: SymbolId,
    ) -> SessionResult<SymbolRef> {
        if handle.0 == 0 {
            return Err(client_error("empty symbol handle"));
        }
        let (retained, raw) = {
            let symbols = self.symbols.lock().expect("symbol registry");
            let entry = symbols.entries.get(&handle.0).ok_or_else(|| {
                client_error(format!(
                    "symbol handle {} not found in snapshot registry",
                    handle.0
                ))
            })?;
            (entry.retained.clone(), entry.raw)
        };
        if Arc::ptr_eq(retained.owner(), operation.owner()) {
            return operation
                .import_symbol_ref(&retained)
                .map_err(checker_error);
        }
        operation.symbol_ref(raw).map_err(|_| {
            client_error(format!(
                "symbol handle {} is not visible to the requested project",
                handle.0
            ))
        })
    }
}

/// A query's starting point: the snapshot, the project's program and
/// registry, and the project handle responses are scoped to (the pin's
/// `checkerSetup` of tsc/internal/api/session.go).
pub struct CheckerSetup<'a> {
    pub data: &'a SnapshotData,
    pub program: &'a Arc<Program>,
    pub registry: Arc<ProjectRegistry>,
    pub project: ProjectId,
}

impl SnapshotData {
    /// port: tsc/internal/api/session.go:Session.setupChecker
    pub fn setup_checker(&self, project: &ProjectId) -> SessionResult<CheckerSetup<'_>> {
        let program = self.program(project)?;
        let registry = self.registries.project(project, self.project(project)?)?;
        Ok(CheckerSetup {
            data: self,
            program,
            registry,
            project: project.clone(),
        })
    }
}

/// `index.kind.path`: the node's index in its file's node index table, its
/// kind, and the file's path.
/// port: tsc/internal/api/session.go:snapshotData.nodeHandleFrom
pub fn node_handle(operation: &Operation<'_>, node: tsr_ast::NodeId) -> SessionResult<NodeHandle> {
    let view = operation.ast_view(node).map_err(checker_error)?;
    let read = view.node(node).map_err(checker_error)?;
    let kind = read.kind();
    let mut root = node;
    while let Some(parent) = view.node(root).map_err(checker_error)?.parent() {
        root = parent;
    }
    let file = view.source_file(root).map_err(checker_error)?;
    let path = String::from_utf8_lossy(file.path()).into_owned();
    let table = tsr_encoder::get_node_index_table(
        view,
        root,
        &mut tsr_parser::ParserJsDocProvider::default(),
    )
    .map_err(checker_error)?
    .ok_or_else(|| SessionError::Other("node index table unavailable".into()))?;
    let index = table.get_index(view, Some(&read)).map_err(checker_error)?;
    Ok(NodeHandle(format!("{index}.{}.{path}", kind.raw())))
}

/// port: tsc/internal/api/session.go:snapshotData.resolveNodeHandle
pub fn resolve_node_handle(
    program: &Program,
    handle: &NodeHandle,
) -> SessionResult<tsr_ast::NodeId> {
    let text = handle.0.as_str();
    let invalid = || {
        client_error(format!(
            "invalid node handle {}",
            tsr_jsstring::go_quote(text.as_bytes())
        ))
    };
    let first = text.find('.').ok_or_else(invalid)?;
    let second = text[first + 1..].find('.').ok_or_else(invalid)? + first + 1;
    let index: usize = text[..first].parse().map_err(|error| {
        client_error(format!(
            "invalid node handle {}: {error}",
            tsr_jsstring::go_quote(text.as_bytes())
        ))
    })?;
    let path = &text[second + 1..];
    let stale = || {
        client_error(format!(
            "node handle {} could not be resolved (file may not be loaded or handle may be stale)",
            tsr_jsstring::go_quote(text.as_bytes())
        ))
    };
    let file = program.file(path.as_bytes()).ok_or_else(stale)?;
    let view = file.bound().view().ast();
    let table = tsr_encoder::get_node_index_table(
        view,
        file.source(),
        &mut tsr_parser::ParserJsDocProvider::default(),
    )
    .map_err(checker_error)?
    .ok_or_else(stale)?;
    table
        .nodes()
        .get(index)
        .copied()
        .flatten()
        .ok_or_else(stale)
}

/// The node whose property name touches a UTF-16 position of a file, or the
/// file's root when none does; `None` for a file the program does not have.
/// port: tsc/internal/astnav/tokens.go:GetTouchingPropertyName
pub fn touching_property_name(
    program: &Program,
    file_name: &[u8],
    position: u32,
) -> SessionResult<Option<tsr_ast::NodeId>> {
    let Some(file) = program.source_file(file_name) else {
        return Ok(None);
    };
    let view = file.bound().view().ast();
    let source = view.source_file(file.source()).map_err(checker_error)?;
    let offset = source
        .position_map()
        .utf16_to_utf8(isize::try_from(position).unwrap_or(isize::MAX));
    let mut provider = tsr_parser::ParserJsDocProvider::default();
    let mut navigator = tsr_astnav::Navigator::new(view, file.source(), &mut provider);
    navigator
        .get_touching_property_name(i64::try_from(offset).unwrap_or(i64::MAX))
        .map(Some)
        .map_err(|error| SessionError::Other(format!("{error:?}")))
}
