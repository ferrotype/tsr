use super::{NodeRead, ReadRecord};
use crate::{AstBuilder, FactoryMethods, JsString, SyntaxKind as K};
use tsr_arena::{Counters, Error};
use tsr_jsstring::SourceText;

#[test]
#[cfg(target_pointer_width = "64")]
fn compact_read_layout_records_the_feature_costs() {
    // A layout assertion for the experiment, not a portable ABI promise.
    // The enum uses a bool niche when typed selection is compiled in; that
    // selector fits without growing the ordinary experimental record.
    assert_eq!(
        size_of::<NodeRead<'_>>(),
        32 + 8 * usize::from(cfg!(feature = "access-stats"))
    );
    assert_eq!(align_of::<NodeRead<'_>>(), 8);
}

#[test]
fn cold_reads_keep_transaction_reborrow_and_lazy_publication_guards() {
    let counters = Counters::new();
    let before = counters.snapshot();
    {
        let mut builder = AstBuilder::new(
            SourceText::from_loaded_bytes(b"physical\xff".as_slice()),
            &counters,
        );
        let first = builder.new_identifier(JsString::from_bytes(b"first".as_slice()));
        let second = builder.new_identifier(JsString::from_bytes(b"second".as_slice()));
        let root = builder.new_qualified_name(Some(first), Some(second));
        let owner_id = builder.id();
        let file = builder.complete(root).unwrap().publish_unbound();
        let core = file.view().node(first).unwrap();
        assert!(matches!(core.record, ReadRecord::Core { .. }));
        assert!(matches!(
            core.as_borrowed().unwrap().record,
            ReadRecord::Core { .. }
        ));
        drop(core);
        let roots = file
            .view()
            .jsdoc(first, |transaction| {
                let core = transaction.node(first)?;
                assert!(matches!(&core.record, ReadRecord::TransactionCore { .. }));
                #[cfg(feature = "contextual-payload-experiment")]
                let core = core.with_typed_payload_fields(true);
                let borrowed = core.as_borrowed().unwrap();
                #[cfg(feature = "contextual-payload-experiment")]
                assert!(crate::NodeAccess::typed_payload_shape(&borrowed).is_some());
                assert_eq!(borrowed.id(), first);
                assert_eq!(borrowed.owner_id(), owner_id);
                assert_eq!(borrowed.source().as_bytes(), b"physical\xff");
                assert_eq!(borrowed.as_identifier().unwrap().text(), b"first");
                drop(borrowed);
                drop(core);
                let lazy = transaction.new_identifier(JsString::from_bytes(b"lazy\xff".as_slice()));
                transaction.node_mut(lazy)?.set_parent(Some(first));
                let staged = transaction.node(lazy)?;
                assert!(matches!(
                    &staged.record,
                    ReadRecord::OwnedTransaction { .. }
                ));
                assert_eq!(staged.as_borrowed().unwrap().parent(), Some(first));
                Ok(vec![lazy])
            })
            .unwrap();
        let lazy = roots[0];
        let read = file.view().node(lazy).unwrap();
        assert!(matches!(&read.record, ReadRecord::Lazy { .. }));
        assert!(read.as_borrowed().is_none());
        let ReadRecord::Lazy {
            record: payload, ..
        } = &read.record
        else {
            unreachable!()
        };
        // This node belongs only to this test. The published page and this
        // read each retain it once; an escaped handle adds exactly one more.
        assert_eq!(std::sync::Arc::strong_count(payload), 2);
        let temporary_retained = file.retain_node(lazy).unwrap();
        assert_eq!(std::sync::Arc::strong_count(payload), 3);
        {
            let borrowed = temporary_retained.read();
            let again = borrowed.as_borrowed().unwrap();
            assert_eq!(again.as_identifier().unwrap().text(), b"lazy\xff");
            assert_eq!(std::sync::Arc::strong_count(payload), 3);
        }
        drop(temporary_retained);
        assert_eq!(std::sync::Arc::strong_count(payload), 2);
        // Hold the read and its typed payload while another transaction grows
        // the same owner's lazy directory. The read must keep the payload
        // Arc alive instead of borrowing a directory-relative reference.
        let identifier = read.as_identifier().unwrap();
        file.view()
            .jsdoc(second, |transaction| {
                let mut added = Vec::new();
                for _ in 0..4096 {
                    added
                        .push(transaction.new_identifier(JsString::from_bytes(b"grow".as_slice())));
                }
                Ok(added)
            })
            .unwrap();
        assert_eq!(identifier.text(), b"lazy\xff");
        assert_eq!(read.parent(), Some(first));
        assert_eq!(read.source().as_bytes(), b"physical\xff");
        drop(read);
        drop(roots);
        let retained = file.retain_node(lazy).unwrap();
        drop(file);
        let read = retained.read();
        assert!(matches!(&read.record, ReadRecord::OwnedFile { .. }));
        assert_eq!(read.id(), lazy);
        assert_eq!(read.kind(), K::Identifier);
        assert_eq!(read.owner_id(), owner_id);
        assert_eq!(
            read.as_borrowed().unwrap().as_identifier().unwrap().text(),
            b"lazy\xff"
        );
    }
    assert_eq!(counters.snapshot(), before);
}

#[test]
fn cold_transaction_unwind_does_not_publish_or_retain_staged_nodes() {
    let counters = Counters::new();
    let before = counters.snapshot();
    {
        let mut builder = AstBuilder::new(SourceText::default(), &counters);
        let root = builder.new_identifier(JsString::from_bytes(b"root".as_slice()));
        let file = builder.complete(root).unwrap().publish_unbound();
        let mut failed = None;
        let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = file.view().jsdoc(root, |transaction| {
                let lazy = transaction.new_identifier(JsString::from_bytes(b"failed".as_slice()));
                failed = Some(lazy);
                let read = transaction.node(lazy)?;
                assert_eq!(
                    read.as_borrowed().unwrap().as_identifier().unwrap().text(),
                    b"failed"
                );
                panic!("abort compact-read transaction");
            });
        }));
        assert!(error.is_err());
        assert!(matches!(
            file.view().node(failed.unwrap()),
            Err(Error::InvalidSlot)
        ));
        let roots = file
            .view()
            .jsdoc(root, |transaction| {
                Ok(vec![transaction.new_identifier(JsString::from_bytes(
                    b"success".as_slice(),
                ))])
            })
            .unwrap();
        let read = file.view().node(roots[0]).unwrap();
        assert_eq!(read.as_identifier().unwrap().text(), b"success");
    }
    assert_eq!(counters.snapshot(), before);
}
