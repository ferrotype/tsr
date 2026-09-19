//! S09-1: checker results and checker-created AST nodes retained past a pool
//! lease. Every scenario runs over a program-backed pool, so "dependencies"
//! means the real bound file set, and leases end the way a request ends: the
//! checkout drops, and the idle slot may be replaced.

use super::*;
use std::sync::Weak;
use ts_arena::NodeId;
use ts_checker::{
    RetainedNode, RetainedSignature, RetainedSymbol, RetainedType, RetainedTypeList, TypeRef,
};
use ts_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use ts_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use ts_jsstring::JsString;

const SOURCE: &[u8] = b"interface Shared { field: string }\n";
const WRONG_OWNER: Error = Error::Arena(ts_arena::Error::WrongOwner);

struct Fixture {
    pool: Arc<CheckerPool>,
    program: Weak<Program>,
    /// `interface Shared`, a node of the bound file.
    declaration: NodeId,
    /// Its name, where the symbol query starts.
    name: NodeId,
    source: NodeId,
}

/// The pool's host is the only strong program reference once this returns: the
/// parse cache and the loader's handle are dropped here.
fn fixture(counters: &Counters) -> Fixture {
    let mut files = ts_vfs::MemoryBuilder::new(b"/", true);
    files.insert_loaded(b"/main.ts", SOURCE);
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config: ts_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        target: ScriptTarget::ESNEXT,
                        module: ModuleKind::ESNEXT,
                        strict: Tristate::TRUE,
                        no_lib: Tristate::TRUE,
                        ..Default::default()
                    },
                    vec![JsString::from_bytes(b"/main.ts".as_slice())],
                ),
                host: Arc::new(files.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/no-default-lib".as_slice()),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            counters,
        )
        .unwrap(),
    );
    let (source, declaration, name) = {
        let file = program.file(b"/main.ts").unwrap();
        let source = file.source();
        let view = file.bound().view().ast();
        let declaration = view
            .node_slice(view.node(source).unwrap().statements(view).unwrap())
            .unwrap()
            .iter()
            .flatten()
            .next()
            .unwrap();
        let name = file
            .bound()
            .view()
            .node(declaration)
            .unwrap()
            .name()
            .unwrap();
        (source, declaration, name)
    };
    let pool = CheckerPool::for_program(
        Arc::new(ProgramCheckerHost::new(program.clone())),
        counters,
        1,
    );
    Fixture {
        pool,
        program: Arc::downgrade(&program),
        declaration,
        name,
        source,
    }
}

/// One of each result kind the symbols note names, all from one operation.
struct Results {
    ty: RetainedType,
    signature: RetainedSignature,
    symbol: RetainedSymbol,
    list: RetainedTypeList,
    /// The operation-scoped values the retained handles must import back to.
    expected_type: TypeRef,
    expected_string: TypeRef,
}

fn retain_results(checker: &PooledChecker, name: NodeId) -> Results {
    let mut operation = checker.operation().unwrap();
    let symbol = operation.get_symbol_at_location(name).unwrap().unwrap();
    let ty = operation.get_declared_type_of_symbol(symbol).unwrap();
    let string = operation.builtin_type("stringType").unwrap();
    let signature = operation.call_signature(&[], ty).unwrap();
    // The queried symbol lives in the bound file, not in the checker's arena,
    // so the checker-created-symbol form cannot retain it.
    assert!(operation.retain_symbol(symbol.id()).is_err());
    Results {
        ty: operation.retain_type(ty).unwrap(),
        signature: operation.retain_signature(signature).unwrap(),
        symbol: operation.retain_symbol_ref(symbol).unwrap(),
        list: operation.retain_type_list(&[ty, string]).unwrap(),
        expected_type: ty,
        expected_string: string,
    }
}

/// Every retained kind imports into its own checker and still means what it
/// meant when it was retained.
fn assert_results_resolve(results: &Results) {
    let mut operation = results.ty.owner().operation().unwrap();
    let ty = operation.import_type(&results.ty).unwrap();
    assert_eq!(ty, results.expected_type);
    assert_eq!(
        operation.import_type_list(&results.list).unwrap(),
        vec![results.expected_type, results.expected_string]
    );
    let signature = operation.import_signature(&results.signature).unwrap();
    assert_eq!(
        operation.signature_return_type(signature).unwrap(),
        Some(ty)
    );
    let symbol = operation.import_symbol_ref(&results.symbol).unwrap();
    assert_eq!(
        operation.get_declared_type_of_symbol(symbol).unwrap(),
        ty,
        "the retained symbol still resolves through the retained file"
    );
}

mod results {
    use super::*;

    #[test]
    fn retained_results_outlive_lease_release_with_exact_identity() {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let fixture = fixture(&counters);
        let checkout = fixture.pool.acquire(CheckerSlot::Query(0)).unwrap();
        let identity = checkout.owner().identity().id();
        let results = retain_results(&checkout, fixture.name);
        // The lease ends: the operation is gone and the checkout returns.
        drop(checkout);

        for owner in [
            results.ty.owner(),
            results.signature.owner(),
            results.symbol.owner(),
            results.list.owner(),
        ] {
            assert_eq!(owner.identity().id(), identity, "the exact checker");
        }
        assert_results_resolve(&results);
        // A later checkout of the still-occupied slot is the same checker, and
        // the results import through it as well: nothing was rebound or copied.
        let again = fixture.pool.acquire(CheckerSlot::Query(0)).unwrap();
        assert!(Arc::ptr_eq(again.owner(), results.ty.owner()));
        assert_eq!(
            again.operation().unwrap().import_type(&results.ty).unwrap(),
            results.expected_type
        );
        drop(again);
        drop(results);
        drop(fixture);
        assert_eq!(counters.snapshot(), baseline);
    }

    #[test]
    fn idle_slot_replacement_never_rebinds_retained_results() {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let fixture = fixture(&counters);
        let checkout = fixture.pool.acquire(CheckerSlot::Query(0)).unwrap();
        let old_identity = checkout.owner().identity().id();
        let results = retain_results(&checkout, fixture.name);
        drop(checkout);
        assert!(fixture.pool.evict_idle(CheckerSlot::Query(0)).unwrap());

        let replacement = fixture.pool.acquire(CheckerSlot::Query(0)).unwrap();
        assert_ne!(replacement.owner().identity().id(), old_identity);
        assert!(!Arc::ptr_eq(replacement.owner(), results.ty.owner()));
        {
            // The replacement answers the same query with the same numeric
            // slots, which is exactly why identity, not the number, decides.
            let mut operation = replacement.operation().unwrap();
            let symbol = operation
                .get_symbol_at_location(fixture.name)
                .unwrap()
                .unwrap();
            let ty = operation.get_declared_type_of_symbol(symbol).unwrap();
            assert_eq!(ty.id(), results.expected_type.id(), "same type slot");
            assert_ne!(ty, results.expected_type, "different checker");
            assert_eq!(operation.import_type(&results.ty).err(), Some(WRONG_OWNER));
            assert_eq!(
                operation.import_signature(&results.signature).err(),
                Some(WRONG_OWNER)
            );
            assert_eq!(
                operation.import_symbol_ref(&results.symbol).err(),
                Some(WRONG_OWNER)
            );
            assert_eq!(
                operation.import_type_list(&results.list).err(),
                Some(WRONG_OWNER)
            );
            assert_eq!(
                operation.type_flags(results.expected_type).err(),
                Some(WRONG_OWNER),
                "an operation-scoped value of the old checker is rejected too"
            );
        }
        // The old checker still answers for its own results.
        assert_eq!(results.ty.owner().identity().id(), old_identity);
        assert_results_resolve(&results);
        drop(replacement);
        drop(results);
        drop(fixture);
        assert_eq!(counters.snapshot(), baseline);
    }

    #[test]
    fn retained_access_reacquires_the_exact_owners_permit() {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let fixture = fixture(&counters);
        let checkout = fixture.pool.acquire(CheckerSlot::Query(0)).unwrap();
        let results = retain_results(&checkout, fixture.name);
        drop(checkout);
        assert!(fixture.pool.evict_idle(CheckerSlot::Query(0)).unwrap());
        let replacement = fixture.pool.acquire(CheckerSlot::Query(0)).unwrap();

        let held = results.ty.owner().operation().unwrap();
        // The permit is the old checker's own: a second access on this thread
        // is reentry, while the replacement checker is free to run.
        assert!(matches!(
            results.ty.owner().operation(),
            Err(Error::Reentry)
        ));
        assert!(replacement.operation().is_ok());

        let (attempted, observed) = std::sync::mpsc::channel();
        let retained = results.ty.clone();
        let expected = results.expected_type;
        let reader = std::thread::spawn(move || {
            ts_arena::observe_next_lease_contention(attempted);
            // Blocks on the production permit until the holder releases it.
            let operation = retained.owner().operation().unwrap();
            assert_eq!(operation.import_type(&retained).unwrap(), expected);
        });
        observed
            .recv()
            .expect("the reader contended on the exact owner's permit");
        drop(held);
        reader.join().unwrap();

        drop(replacement);
        drop(results);
        drop(fixture);
        assert_eq!(counters.snapshot(), baseline);
    }

    #[test]
    fn retirement_rejects_retained_access_while_storage_stays_live() {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let fixture = fixture(&counters);
        let checkout = fixture.pool.acquire(CheckerSlot::Query(0)).unwrap();
        let results = retain_results(&checkout, fixture.name);
        let owner = Arc::downgrade(checkout.owner());
        drop(checkout);

        fixture.pool.generation().retire();
        assert!(matches!(
            results.ty.owner().operation(),
            Err(Error::Arena(ts_arena::Error::Retired))
        ));
        let program = fixture.program.clone();
        drop(fixture);
        // Retirement is not disposal: the pool is gone and the generation is
        // closed, yet the results still hold the checker and its files.
        assert!(owner.upgrade().is_some());
        assert!(program.upgrade().is_some());
        drop(results);
        assert!(owner.upgrade().is_none());
        assert!(program.upgrade().is_none());
        assert_eq!(counters.snapshot(), baseline);
    }

    #[test]
    fn final_result_drop_releases_checker_and_program_storage() {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let fixture = fixture(&counters);
        let checkout = fixture.pool.acquire(CheckerSlot::Query(0)).unwrap();
        let owner = Arc::downgrade(checkout.owner());
        let Results {
            ty,
            signature,
            symbol,
            list,
            ..
        } = retain_results(&checkout, fixture.name);
        // A result cache holding a second root for the same type.
        let cache = vec![ty.clone()];
        drop(checkout);
        let program = fixture.program.clone();
        drop(fixture);

        let alive = || owner.upgrade().is_some() && program.upgrade().is_some();
        assert!(alive(), "the pool is gone; the results are the only roots");
        drop(ty);
        assert!(alive());
        drop(signature);
        assert!(alive());
        drop(symbol);
        assert!(alive());
        drop(list);
        assert!(alive(), "the cache entry is a root of its own");
        drop(cache);
        assert!(owner.upgrade().is_none(), "no cycle keeps the checker");
        assert!(program.upgrade().is_none(), "nor its file set");
        assert_eq!(counters.snapshot(), baseline);
    }
}

mod ast {
    use super::*;

    struct Synthetic {
        node: RetainedNode,
        signature: RetainedSignature,
        ty: RetainedType,
        expected_type: TypeRef,
    }

    /// A parented synthetic expression and a signature with a checker-created
    /// declaration, which is what the checker's own AST factory produces.
    fn retain_synthetic(checker: &PooledChecker, fixture: &Fixture) -> Synthetic {
        let mut operation = checker.operation().unwrap();
        let symbol = operation
            .get_symbol_at_location(fixture.name)
            .unwrap()
            .unwrap();
        let ty = operation.get_declared_type_of_symbol(symbol).unwrap();
        let node = operation
            .synthetic_expression_at(fixture.declaration, ty, false)
            .unwrap();
        let signature = operation.call_signature(&[], ty).unwrap();
        Synthetic {
            node: operation.retain_node(node).unwrap(),
            signature: operation.retain_signature(signature).unwrap(),
            ty: operation.retain_type(ty).unwrap(),
            expected_type: ty,
        }
    }

    fn assert_synthetic_resolves(synthetic: &Synthetic, fixture_declaration: NodeId) {
        let operation = synthetic.node.owner().operation().unwrap();
        let node = operation.import_node(&synthetic.node).unwrap();
        assert_eq!(
            operation.node_kind(node).unwrap().known(),
            Some(ts_ast::SyntaxKind::SyntheticExpression)
        );
        assert_eq!(
            operation.synthetic_expression_type(node).unwrap(),
            synthetic.expected_type
        );
        let (parent, kind) = operation.node_parent(node).unwrap().unwrap();
        assert_eq!(parent, fixture_declaration, "the parent is the file's node");
        assert_eq!(kind.known(), Some(ts_ast::SyntaxKind::InterfaceDeclaration));
        let signature = operation.import_signature(&synthetic.signature).unwrap();
        let declaration = operation.signature_declaration(signature).unwrap().unwrap();
        assert_eq!(
            operation.node_kind(declaration).unwrap().known(),
            Some(ts_ast::SyntaxKind::FunctionType)
        );
        assert_eq!(
            operation.signature_return_type(signature).unwrap(),
            Some(operation.import_type(&synthetic.ty).unwrap())
        );
    }

    #[test]
    fn synthetic_expression_resolves_type_and_file_parent_after_lease_release() {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let fixture = fixture(&counters);
        let checkout = fixture.pool.acquire(CheckerSlot::Query(0)).unwrap();
        let synthetic = retain_synthetic(&checkout, &fixture);
        drop(checkout);
        assert_synthetic_resolves(&synthetic, fixture.declaration);
        // Replacing the idle slot changes nothing for the retained node.
        assert!(fixture.pool.evict_idle(CheckerSlot::Query(0)).unwrap());
        assert_synthetic_resolves(&synthetic, fixture.declaration);
        drop(synthetic);
        drop(fixture);
        assert_eq!(counters.snapshot(), baseline);
    }

    #[test]
    fn signature_declaration_survives_builder_release() {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let fixture = fixture(&counters);
        let checkout = fixture.pool.acquire(CheckerSlot::Query(0)).unwrap();
        let synthetic = retain_synthetic(&checkout, &fixture);
        {
            let mut operation = checkout.operation().unwrap();
            let ty = operation.import_type(&synthetic.ty).unwrap();
            // The node builder allocates in its own arena and releases it when
            // the builder goes away; the node it returned is then unusable.
            let generated = operation
                .node_builder()
                .type_to_type_node(ty, Some(fixture.source), 0, 0)
                .unwrap()
                .unwrap();
            assert_eq!(
                operation.type_to_string_at(ty, Some(generated), 0).err(),
                Some(WRONG_OWNER),
                "the builder's arena was released"
            );
            // The checker's own AST is a different arena and is untouched.
            let signature = operation.import_signature(&synthetic.signature).unwrap();
            let declaration = operation.signature_declaration(signature).unwrap().unwrap();
            assert_eq!(
                operation.node_kind(declaration).unwrap().known(),
                Some(ts_ast::SyntaxKind::FunctionType)
            );
        }
        drop(checkout);
        assert_synthetic_resolves(&synthetic, fixture.declaration);
        drop(synthetic);
        drop(fixture);
        assert_eq!(counters.snapshot(), baseline);
    }

    #[test]
    fn another_checker_rejects_typed_ast_links() {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let fixture = fixture(&counters);
        let first = fixture.pool.acquire(CheckerSlot::Query(0)).unwrap();
        let sibling = fixture.pool.acquire(CheckerSlot::Api).unwrap();
        let synthetic = retain_synthetic(&first, &fixture);
        let (node, signature) = {
            let operation = first.operation().unwrap();
            (
                operation.import_node(&synthetic.node).unwrap(),
                operation.import_signature(&synthetic.signature).unwrap(),
            )
        };
        {
            // Same pool generation, same program, same construction order.
            let own = retain_synthetic(&sibling, &fixture);
            let operation = sibling.operation().unwrap();
            let own_signature = operation.import_signature(&own.signature).unwrap();
            assert_eq!(own_signature.id(), signature.id(), "same signature slot");
            assert_eq!(
                operation.import_node(&synthetic.node).err(),
                Some(WRONG_OWNER)
            );
            assert_eq!(
                operation.import_signature(&synthetic.signature).err(),
                Some(WRONG_OWNER)
            );
            assert_eq!(operation.node_kind(node).err(), Some(WRONG_OWNER));
            assert_eq!(operation.node_parent(node).err(), Some(WRONG_OWNER));
            assert_eq!(
                operation.synthetic_expression_type(node).err(),
                Some(WRONG_OWNER)
            );
            assert_eq!(
                operation.signature_declaration(signature).err(),
                Some(WRONG_OWNER)
            );
            assert_eq!(
                operation.signature_return_type(signature).err(),
                Some(WRONG_OWNER)
            );
        }
        assert_synthetic_resolves(&synthetic, fixture.declaration);
        drop(first);
        drop(sibling);
        drop(synthetic);
        drop(fixture);
        assert_eq!(counters.snapshot(), baseline);
    }

    #[test]
    fn checker_context_drop_cannot_free_retained_ast_storage() {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let fixture = fixture(&counters);
        let checkout = fixture.pool.acquire(CheckerSlot::Query(0)).unwrap();
        let owner = Arc::downgrade(checkout.owner());
        let synthetic = retain_synthetic(&checkout, &fixture);
        drop(checkout);
        assert!(fixture.pool.evict_idle(CheckerSlot::Query(0)).unwrap());
        let declaration = fixture.declaration;
        let program = fixture.program.clone();
        drop(fixture);

        // The slot, the pool and every caller handle are gone.
        assert!(owner.upgrade().is_some());
        assert!(program.upgrade().is_some());
        assert_synthetic_resolves(&synthetic, declaration);
        let Synthetic {
            node,
            signature,
            ty,
            ..
        } = synthetic;
        drop(ty);
        drop(node);
        assert!(owner.upgrade().is_some(), "the signature still needs it");
        {
            let operation = signature.owner().operation().unwrap();
            let imported = operation.import_signature(&signature).unwrap();
            assert!(operation.signature_declaration(imported).unwrap().is_some());
        }
        drop(signature);
        assert!(owner.upgrade().is_none());
        assert!(program.upgrade().is_none());
        assert_eq!(counters.snapshot(), baseline);
    }
}
