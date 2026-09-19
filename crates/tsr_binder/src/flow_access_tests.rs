//! Local/checked representations must preserve full flow identity and defer
//! failures in raw links until the algorithm actually dereferences those links.
use crate::{
    backend::Backend,
    flow_access::{BindingFlow, BindingFlowList},
    Binder,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use ts_arena::Counters;
use ts_ast::{flow_flags as F, FlowList, FlowLists, FlowNode, FlowNodes, SourceFileParseOptions};
use ts_core::ScriptKind;
use ts_jsstring::SourceText;

fn parsed() -> ts_ast::ParsedFile {
    ts_parser::parse_source_file(
        SourceText::from_loaded_bytes(b"x;".as_slice()),
        ScriptKind::TS,
        SourceFileParseOptions {
            file_name: ts_ast::JsString::from_bytes(b"/flow-access.ts".as_slice()),
            ..Default::default()
        },
    )
}

#[test]
fn checked_and_local_flow_aliases_compare_equal_and_do_not_duplicate_antecedents() {
    parsed()
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|local| {
                    let mut binder = Binder::from_backend(Backend::Local(local));
                    let flow = binder.new_flow_node(F::START);
                    assert!(matches!(flow, BindingFlow::Local(_)));
                    let checked = BindingFlow::Checked(binder.flow_id(flow));
                    assert!(binder.same_flow(Some(flow), Some(checked)));
                    assert!(binder.same_flow(Some(checked), Some(flow)));
                    assert!(binder.same_flow(None, None));
                    assert!(!binder.same_flow(Some(flow), None));
                    let other = binder.new_flow_node(F::START);
                    assert!(!binder.same_flow(Some(flow), Some(other)));

                    let label = binder.create_branch_label();
                    binder.add_antecedent(label, flow);
                    let original = binder.flow_antecedents(label).unwrap();
                    let flags = binder.flow(flow).flags();
                    binder.add_antecedent(label, checked);
                    assert_eq!(binder.flow_antecedents(label), Some(original));
                    assert!(binder.flow_list(original).next.is_none());
                    assert_eq!(binder.flow(flow).flags(), flags);
                    assert!(binder.same_flow(Some(binder.finish_flow_label(label)), Some(flow)));
                    Ok(())
                })
                .expect("eligible source")
        })
        .unwrap();
}

#[test]
fn escaped_links_survive_reading_their_parent_and_fail_only_on_dereference() {
    let counters = Counters::new();
    let mut foreign_flows = FlowNodes::new(&counters);
    let foreign_flow = foreign_flows.push(FlowNode::new(F::START));
    let mut foreign_lists = FlowLists::new(&counters);
    let foreign_list = foreign_lists.push(FlowList::default());
    parsed()
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|local| {
                    let mut binder = Binder::from_backend(Backend::Local(local));
                    let checked_flow = BindingFlow::Checked(foreign_flow);
                    let checked_list = BindingFlowList::Checked(foreign_list);
                    let list = binder.new_flow_list(Some(checked_flow), Some(checked_list));
                    let label = binder.create_branch_label();
                    binder.set_flow_antecedents(label, Some(checked_list));

                    let links = binder.flow_list(list);
                    assert_eq!(links.flow, Some(checked_flow));
                    assert_eq!(links.next, Some(checked_list));
                    assert_eq!(binder.flow_antecedents(label), Some(checked_list));
                    assert!(catch_unwind(AssertUnwindSafe(|| binder.flow(checked_flow))).is_err());
                    assert!(
                        catch_unwind(AssertUnwindSafe(|| binder.flow_list(checked_list))).is_err()
                    );

                    // Clearing the escaped references restores a valid result.
                    binder.set_flow_antecedents(label, None);
                    binder.set_flow_list_next(list, None);
                    let BindingFlowList::Local(list) = list else {
                        panic!("new list belongs to the local scope")
                    };
                    let Backend::Local(local) = &mut binder.builder else {
                        unreachable!()
                    };
                    local.local_set_flow_list_flow(list, None);
                    Ok(())
                })
                .expect("eligible source")
        })
        .unwrap();
}
