//! Transformed identities live for one file's emit. The checker may cache a
//! reference answer, but must not accumulate IDs of retired transform arenas.
use crate::{emit_visibility::EmitTransient, Operation};

impl Operation<'_> {
    /// Scope resolver bookkeeping to one emit chain, including printing.
    /// Nested scopes restore their caller's mappings. Errors and unwinding
    /// discard the inner mappings as well; normal checker caches remain live.
    pub fn with_emit_scope<R>(&mut self, run: impl FnOnce(&mut Self) -> R) -> R {
        let previous = std::mem::take(&mut self.state_mut().emit.transient);
        let scope = EmitScope {
            operation: self,
            previous,
        };
        run(scope.operation)
    }
}

struct EmitScope<'scope, 'checker> {
    operation: &'scope mut Operation<'checker>,
    previous: EmitTransient,
}
impl Drop for EmitScope<'_, '_> {
    fn drop(&mut self) {
        self.operation.state_mut().emit.transient = std::mem::take(&mut self.previous);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use tsr_arena::{CheckerIdentity, Counters, Generation};
    use tsr_ast::{AstBuilder, FactoryMethods, JsString};
    use tsr_jsstring::SourceText;

    #[test]
    fn temporary_ids_are_scoped_and_identifier_reuse_preserves_name_and_parent() {
        let counters = Counters::new();
        let owner = Arc::new(
            crate::CheckerOwner::new(
                CheckerIdentity::new(Generation::new(&counters), &counters),
                &counters,
                crate::CheckerOptions::default(),
            )
            .unwrap(),
        );
        let mut op = owner.operation().unwrap();
        let parent = op
            .state_mut()
            .factory
            .new_identifier(JsString::from_bytes(b"parent".as_slice()));
        let mut nodes = AstBuilder::new(SourceText::default(), &counters);
        let first = nodes.new_identifier(JsString::default());
        let second = nodes.new_identifier(JsString::default());
        let name = JsString::from_bytes(b"React".as_slice());
        op.with_emit_scope(|op| {
            op.state_mut()
                .emit_parse_tree_stand_in(first, name.clone(), Some(parent))
                .unwrap();
            let outer = op.state().emit.transient.parse_tree_stand_ins[&first];
            let failed: Result<(), ()> = op.with_emit_scope(|op| {
                assert!(op.state().emit.transient.parse_tree_stand_ins.is_empty());
                op.state_mut()
                    .emit_parse_tree_stand_in(second, name.clone(), Some(parent))
                    .unwrap();
                assert_eq!(
                    op.state().emit.transient.parse_tree_stand_ins[&second],
                    outer
                );
                op.state_mut()
                    .emit
                    .transient
                    .import_refs
                    .insert(second, first);
                Err(())
            });
            assert_eq!(failed, Err(()));
            assert_eq!(op.state().emit.transient.parse_tree_stand_ins.len(), 1);
            assert_eq!(
                op.state().emit.transient.parse_tree_stand_ins[&first],
                outer
            );
            assert!(op.state().emit.transient.import_refs.is_empty());
            op.state_mut()
                .emit_parse_tree_stand_in(second, name.clone(), None)
                .unwrap();
            assert_ne!(
                op.state().emit.transient.parse_tree_stand_ins[&second],
                outer
            );
            op.state_mut()
                .emit_parse_tree_stand_in(
                    second,
                    JsString::from_bytes(b"Other".as_slice()),
                    Some(parent),
                )
                .unwrap();
            assert_ne!(
                op.state().emit.transient.parse_tree_stand_ins[&second],
                outer
            );
        });
        assert_eq!(op.state().emit.identifiers.len(), 3);
        assert!(op.state().emit.transient.parse_tree_stand_ins.is_empty());
        assert!(op.state().emit.transient.import_refs.is_empty());

        // Verify the same cleanup during unwinding. Do not resume checker work
        // after the injected panic; production unwinds its operation as well.
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            op.with_emit_scope(|op| {
                op.state_mut()
                    .emit
                    .transient
                    .import_refs
                    .insert(first, second);
                panic!("injected emit failure");
            });
        }));
        assert!(panic.is_err());
        assert!(op.state().emit.transient.import_refs.is_empty());
    }
}
