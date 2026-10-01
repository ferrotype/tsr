//! The transformer framework (`transformers/transformer.go`, `chain.go`).
//!
//! A [`Transformer`] is upstream's: an emit context, a node visitor attached
//! to it and a `visit` function. The visitor is `tsr_ast::NodeVisitor`, whose
//! callback is a shared `Fn` because visiting reenters it; a transformer keeps
//! its mutable state in `Cell`s and `RefCell`s it never holds across a visit,
//! and receives the visitor (and through it the factory) as an argument where
//! upstream reads `tx.Visitor()` and `tx.Factory()`.
//!
//! Upstream's visit functions cannot fail; here storage reads and resolver
//! queries can. A transformer records its first failure in the [`Failure`]
//! of its options, returns the node unchanged and stops visiting;
//! [`Transformer::transform_source_file`] reports it, so no rewritten root
//! escapes a failed transformation.
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use tsr_ast::{NodeId, NodeVisit, NodeVisitor, RuntimeFactory, SyntaxKind as K};
use tsr_core::{CompilerOptions, ModuleKind};
use tsr_printer::script_resolver::{EmitResolver, EmitResolverError, ReferenceResolver};
use tsr_printer::{EmitContext, EmitVisitorHooks};

/// What a transformation can fail with.
#[derive(Debug)]
pub enum Error {
    Arena(tsr_arena::Error),
    Resolver(EmitResolverError),
    /// A transformer or a construct the port does not transform yet, by
    /// upstream name.
    Unsupported(&'static str),
}

impl From<tsr_arena::Error> for Error {
    fn from(error: tsr_arena::Error) -> Self {
        Self::Arena(error)
    }
}

impl From<EmitResolverError> for Error {
    fn from(error: EmitResolverError) -> Self {
        Self::Resolver(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Arena(error) => write!(f, "{error:?}"),
            Self::Resolver(error) => write!(f, "{error}"),
            Self::Unsupported(name) => write!(f, "unsupported: {name}"),
        }
    }
}

impl std::error::Error for Error {}

/// The first failure of a transformation, shared by the transformers of one
/// chain. Clones share the slot.
#[derive(Clone, Default)]
pub struct Failure(Rc<RefCell<Option<Error>>>);

impl Failure {
    /// Records `error` unless an earlier one is already held.
    pub fn record(&self, error: impl Into<Error>) {
        let mut slot = self.0.borrow_mut();
        if slot.is_none() {
            *slot = Some(error.into());
        }
    }

    /// Whether a failure is held; a transformer stops visiting once it is.
    pub fn is_set(&self) -> bool {
        self.0.borrow().is_some()
    }

    pub fn take(&self) -> Option<Error> {
        self.0.borrow_mut().take()
    }

    /// The value of `result`, or `None` after recording its error.
    pub fn ok<T, E: Into<Error>>(&self, result: Result<T, E>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.record(error);
                None
            }
        }
    }
}

/// The resolver handles of [`TransformOptions`]. A query borrows the resolver
/// for its own duration only; no visit runs inside one.
pub type SharedEmitResolver<'a> = Rc<RefCell<dyn EmitResolver + 'a>>;
pub type SharedReferenceResolver<'a> = Rc<RefCell<dyn ReferenceResolver + 'a>>;

/// `GetEmitModuleFormatOfFile` of the emit host, by file name (upstream's
/// `ast.HasFileName`): a transformed file is a new node with its source's name.
pub type EmitModuleFormatOfFile<'a> = Rc<dyn Fn(&[u8]) -> Result<ModuleKind, Error> + 'a>;

/// `TransformOptions`, with the failure slot the transformers of one file share.
#[derive(Clone)]
pub struct TransformOptions<'a> {
    pub context: EmitContext,
    pub compiler_options: Arc<CompilerOptions>,
    pub resolver: SharedReferenceResolver<'a>,
    pub emit_resolver: SharedEmitResolver<'a>,
    pub get_emit_module_format_of_file: EmitModuleFormatOfFile<'a>,
    pub failure: Failure,
}

/// `TransformerFactory`. `None` is upstream's nil transformer.
pub type TransformerFactory<'a> =
    Box<dyn Fn(&TransformOptions<'a>) -> Option<Transformer<'a>> + 'a>;

/// `Transformer`.
pub struct Transformer<'a> {
    emit_context: EmitContext,
    hooks: EmitVisitorHooks,
    visit: Box<NodeVisit<'a>>,
    failure: Failure,
}

impl<'a> Transformer<'a> {
    /// Upstream initializes an embedded struct in place and panics on a second
    /// call; here construction is the only way to obtain one. A nil context
    /// is `None`, for which a new one is created.
    // port: tsc/internal/transformers/transformer.go:Transformer.NewTransformer
    pub fn new(
        visit: impl for<'v> Fn(&mut NodeVisitor<'v>, Option<NodeId>) -> Option<NodeId> + 'a,
        emit_context: Option<EmitContext>,
        failure: Failure,
    ) -> Self {
        let emit_context = emit_context.unwrap_or_default();
        let hooks = emit_context.visitor_hooks();
        Self {
            emit_context,
            hooks,
            visit: Box::new(visit),
            failure,
        }
    }

    // port: tsc/internal/transformers/transformer.go:Transformer.EmitContext
    pub fn emit_context(&self) -> &EmitContext {
        &self.emit_context
    }

    /// The transformer's visitor over `factory`, which must be the builder
    /// that carries the emit context's factory hooks. Upstream's `Factory()`
    /// is that builder: `visitor.factory_mut()`.
    // port: tsc/internal/transformers/transformer.go:Transformer.Visitor
    // port: tsc/internal/transformers/transformer.go:Transformer.Factory
    pub fn visitor<'v>(&'v self, factory: &'v mut dyn RuntimeFactory) -> NodeVisitor<'v> {
        self.hooks.new_node_visitor(Some(&*self.visit), factory)
    }

    /// The transformed file, or the first failure any transformer of the
    /// chain recorded while producing it.
    // port: tsc/internal/transformers/transformer.go:Transformer.TransformSourceFile
    pub fn transform_source_file(
        &self,
        factory: &mut dyn RuntimeFactory,
        file: NodeId,
    ) -> Result<NodeId, Error> {
        let transformed = self.visitor(factory).visit_source_file(file);
        match self.failure.take() {
            Some(error) => Err(error),
            None => Ok(transformed),
        }
    }

    /// `TransformSourceFile` for a component of a chain: the failure stays in
    /// the shared slot for the chain's caller.
    fn transform_component(&self, factory: &mut dyn RuntimeFactory, file: NodeId) -> NodeId {
        self.visitor(factory).visit_source_file(file)
    }
}

/// A transformer that is not ported yet: it fails the file by name and leaves
/// it unchanged.
pub(crate) fn unported<'a>(opts: &TransformOptions<'a>, name: &'static str) -> Transformer<'a> {
    let failure = opts.failure.clone();
    Transformer::new(
        move |_: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            failure.record(Error::Unsupported(name));
            node
        },
        Some(opts.context.clone()),
        opts.failure.clone(),
    )
}

// port: tsc/internal/transformers/chain.go:chainedTransformer.visit
fn chained_visit(
    components: &[Transformer<'_>],
    failure: &Failure,
    visitor: &mut NodeVisitor<'_>,
    node: Option<NodeId>,
) -> NodeId {
    let node = node.expect("runtime error: invalid memory address or nil pointer dereference");
    assert!(
        visitor.factory().node(node).kind() == K::SourceFile,
        "Chained transform passed non-sourcefile initial node"
    );
    let mut result = node;
    for component in components {
        if failure.is_set() {
            break;
        }
        result = component.transform_component(visitor.factory_mut(), result);
    }
    result
}

/// Chains transforms in left-to-right order, running them one at a time in
/// order (as opposed to interleaved at each node). The resulting combined
/// transform only operates on SourceFile nodes.
// port: tsc/internal/transformers/chain.go:Chain
pub fn chain<'a>(mut transforms: Vec<TransformerFactory<'a>>) -> TransformerFactory<'a> {
    if transforms.len() < 2 {
        assert!(
            !transforms.is_empty(),
            "Expected some number of transforms to chain, but got none"
        );
        return transforms.remove(0);
    }
    Box::new(move |opt| {
        let mut constructed: Vec<Transformer<'a>> = Vec::with_capacity(transforms.len());
        for transform in &transforms {
            // TODO: flatten nested chains?
            if let Some(result) = transform(opt) {
                constructed.push(result);
            }
        }
        match constructed.len() {
            0 => return None,
            1 => return constructed.pop(),
            _ => {}
        }
        let failure = opt.failure.clone();
        let shared = failure.clone();
        Some(Transformer::new(
            move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
                Some(chained_visit(&constructed, &shared, visitor, node))
            },
            Some(opt.context.clone()),
            failure,
        ))
    })
}
