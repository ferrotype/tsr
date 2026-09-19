use super::*;
use crate::{
    flow_flags, AstBuilder, FlowNodes, FlowReduceLabelData, ParsedFile, SourceFileParseOptions,
};
use ts_arena::{Counters, SymbolArena};
use ts_jsstring::SourceText;

fn parsed(counters: &Counters) -> ParsedFile {
    let text = SourceText::from_loaded_bytes(b"".as_slice());
    let mut builder = AstBuilder::new(text.clone(), counters);
    let source = builder.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/scoped-state.ts".as_slice()),
            ..Default::default()
        },
        text,
        None,
        None,
    );
    builder.complete(source).unwrap()
}

#[test]
fn scoped_state_survives_growth_and_narrow_mutation() {
    parsed(&Counters::new())
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|mut local| {
                    let symbol = local.new_symbol(1, JsString::from_bytes(b"first".as_slice()));
                    let parent = local.new_symbol(2, JsString::from_bytes(b"parent".as_slice()));
                    let table = local.new_table();
                    let flow = local.new_flow(flow_flags::START, None, None);
                    let next = local.new_flow(flow_flags::ASSIGNMENT, None, Some(flow));
                    let list = local.new_flow_list(Some(next), None);
                    let tail = local.new_flow_list(None, None);
                    for _ in 0..600 {
                        local.new_symbol(0, JsString::default());
                        local.new_table();
                        local.new_flow(0, None, None);
                        local.new_flow_list(None, None);
                    }
                    *local.local_symbol_flags_mut(symbol) |= 4;
                    *local.local_symbol_check_flags_mut(symbol) = 8;
                    local.local_set_symbol_parent(symbol, Some(parent));
                    local.local_set_symbol_export_symbol(symbol, Some(parent));
                    local.local_set_symbol_members(symbol, Some(table));
                    local.local_set_symbol_exports(symbol, Some(table));
                    local.local_set_symbol_value_declaration(symbol, Some(local.source()));
                    let source = local.node_id(local.source());
                    let declarations = local.declarations_mut().alloc_one(Some(source))?;
                    local.local_set_symbol_declarations(symbol, declarations)?;
                    let symbol_id = local.symbol_id(symbol);
                    {
                        let mut entries = local.local_table_mut(table);
                        entries.insert(JsString::from_bytes(b"symbol".as_slice()), Some(symbol_id));
                        entries.insert(JsString::from_bytes(b"null".as_slice()), None);
                    }
                    *local.local_flow_flags_mut(flow) |= flow_flags::REFERENCED;
                    local.local_set_flow_data(
                        next,
                        Some(FlowData::Ast(local.node_id(local.source()))),
                    );
                    local.local_set_flow_antecedents(flow, Some(list));
                    local.local_set_flow_list_next(list, Some(tail));
                    local.local_set_flow_list_flow(tail, Some(flow));
                    assert_eq!(local.symbol(symbol).flags(), 5);
                    assert_eq!(local.symbol(symbol).check_flags(), 8);
                    assert_eq!(local.symbol(symbol).name_bytes(), b"first");
                    assert_eq!(local.symbol(symbol).parent(), Some(local.symbol_id(parent)));
                    assert_eq!(
                        local.symbol(symbol).export_symbol(),
                        Some(local.symbol_id(parent))
                    );
                    assert_eq!(local.symbol(symbol).members(), Some(local.table_id(table)));
                    assert_eq!(local.symbol(symbol).exports(), Some(local.table_id(table)));
                    assert_eq!(
                        local.symbol(symbol).value_declaration(),
                        Some(local.node_id(local.source()))
                    );
                    assert!(local.symbol(symbol).declarations().same(declarations));
                    assert_eq!(local.table(table).get(b"symbol"), Some(Some(symbol_id)));
                    assert_eq!(local.table(table).get(b"null"), Some(None));
                    assert_eq!(local.table(table).get(b"missing"), None);
                    assert_eq!(local.flow_antecedent(next)?, Some(flow));
                    assert_eq!(local.flow_antecedents(flow)?, Some(list));
                    assert_eq!(local.flow_list_flow(list)?, Some(next));
                    assert_eq!(local.flow_list_next(list)?, Some(tail));
                    assert_eq!(local.flow_list_flow(tail)?, Some(flow));
                    assert_eq!(
                        local.flow(flow).flags(),
                        flow_flags::START | flow_flags::REFERENCED
                    );
                    Ok(())
                })
                .expect("eligible local core")
        })
        .unwrap();
}

#[test]
fn state_imports_check_owner_before_slot_without_mutation() {
    let counters = Counters::new();
    let foreign_symbols = SymbolArena::<()>::new(&counters);
    let foreign_flows = FlowNodes::new(&counters);
    let foreign_lists = crate::FlowLists::new(&counters);
    let foreign_tables = crate::SymbolTables::new(&counters);
    parsed(&counters)
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|mut local| {
                    let symbol = local.new_symbol(0, JsString::default());
                    let table = local.new_table();
                    let flow = local.new_flow(0, None, None);
                    let list = local.new_flow_list(None, None);
                    let wrong_symbol = SymbolId::from_parts(foreign_symbols.id(), u32::MAX)?;
                    let wrong_table = SymbolTableId::from_parts(foreign_tables.id(), u32::MAX)?;
                    let wrong_flow = crate::FlowId::from_parts(foreign_flows.id(), u32::MAX)?;
                    let wrong_list = FlowListId::from_parts(foreign_lists.id(), u32::MAX)?;
                    assert_eq!(local.import_symbol(wrong_symbol), Err(Error::WrongOwner));
                    assert_eq!(local.import_table(wrong_table), Err(Error::WrongOwner));
                    assert_eq!(local.import_flow(wrong_flow), Err(Error::WrongOwner));
                    assert_eq!(local.import_flow_list(wrong_list), Err(Error::WrongOwner));
                    assert_eq!(
                        local.import_symbol(SymbolId::from_parts(
                            local.symbol_id(symbol).arena(),
                            u32::MAX
                        )?),
                        Err(Error::InvalidSlot)
                    );
                    assert_eq!(
                        local.import_table(SymbolTableId::from_parts(
                            local.table_id(table).arena(),
                            u32::MAX
                        )?),
                        Err(Error::InvalidSlot)
                    );
                    assert_eq!(
                        local.import_flow(crate::FlowId::from_parts(
                            local.flow_id(flow).arena(),
                            u32::MAX
                        )?),
                        Err(Error::InvalidSlot)
                    );
                    assert_eq!(
                        local.import_flow_list(FlowListId::from_parts(
                            local.flow_list_id(list).arena(),
                            u32::MAX
                        )?),
                        Err(Error::InvalidSlot)
                    );
                    assert_eq!(local.import_symbol(local.symbol_id(symbol))?, symbol);
                    assert_eq!(local.import_table(local.table_id(table))?, table);
                    assert_eq!(local.import_flow(local.flow_id(flow))?, flow);
                    assert_eq!(local.import_flow_list(local.flow_list_id(list))?, list);
                    assert_eq!(local.symbol(symbol).flags(), 0);
                    assert!(local.table(table).is_empty());
                    assert_eq!(local.flow_antecedents(flow)?, None);
                    assert_eq!(local.flow_list_next(list)?, None);
                    Ok(())
                })
                .expect("eligible local core")
        })
        .unwrap();
}

#[test]
fn compatibility_escapes_remain_raw_until_import_and_narrow_writes_clear_them() {
    let counters = Counters::new();
    let foreign_symbols = SymbolArena::<()>::new(&counters);
    let foreign_lists = crate::FlowLists::new(&counters);
    parsed(&counters)
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|mut local| {
                    let symbol = local.new_symbol(0, JsString::default());
                    let flow = local.new_flow(0, None, None);
                    let list = local.new_flow_list(None, None);
                    let foreign_symbol = SymbolId::from_parts(foreign_symbols.id(), u32::MAX)?;
                    let foreign_list = FlowListId::from_parts(foreign_lists.id(), u32::MAX)?;
                    let wide_flow =
                        crate::FlowId::from_parts(local.flow_id(flow).arena(), u32::MAX)?;
                    let symbol_id = local.symbol_id(symbol);
                    let flow_id = local.flow_id(flow);
                    let list_id = local.flow_list_id(list);
                    local
                        .symbols_mut()
                        .set_parent(symbol_id, Some(foreign_symbol))?;
                    local.set_flow_antecedents(flow_id, Some(foreign_list))?;
                    local
                        .result
                        .flows
                        .set_antecedent(flow_id, Some(wide_flow))?;
                    local.set_flow_list_next(list_id, Some(foreign_list))?;
                    assert_eq!(local.symbol(symbol).parent(), Some(foreign_symbol));
                    assert_eq!(local.flow(flow).antecedents(), Some(foreign_list));
                    assert_eq!(local.flow(flow).antecedent(), Some(wide_flow));
                    assert_eq!(local.flow_list(list).next(), Some(foreign_list));
                    assert_eq!(local.flow_antecedent(flow), Err(Error::InvalidSlot));
                    assert_eq!(local.flow_antecedents(flow), Err(Error::WrongOwner));
                    assert_eq!(local.flow_list_next(list), Err(Error::WrongOwner));
                    let synthetic = Some(FlowData::ReduceLabel(FlowReduceLabelData::new(
                        Some(wide_flow),
                        Some(foreign_list),
                    )));
                    local.local_set_flow_data(flow, synthetic);
                    assert_eq!(local.flow(flow).node(), synthetic);
                    local.local_set_symbol_parent(symbol, None);
                    local.local_set_flow_antecedent(flow, None);
                    local.local_set_flow_antecedents(flow, Some(list));
                    local.local_set_flow_list_next(list, None);
                    local.local_set_flow_data(flow, None);
                    assert_eq!(local.symbol(symbol).parent(), None);
                    assert_eq!(local.flow_antecedent(flow)?, None);
                    assert_eq!(local.flow_antecedents(flow)?, Some(list));
                    assert_eq!(local.flow_list_next(list)?, None);
                    assert_eq!(local.flow(flow).node(), None);
                    Ok(())
                })
                .expect("eligible local core")
        })
        .unwrap();
}

#[test]
fn future_raw_flow_is_importable_only_after_its_slot_is_allocated() {
    parsed(&Counters::new())
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|mut local| {
                    let first = local.new_flow(0, None, None);
                    let first_id = local.flow_id(first);
                    let future = crate::FlowId::from_parts(first_id.arena(), first_id.slot() + 1)?;
                    assert_eq!(local.import_flow(future), Err(Error::InvalidSlot));
                    // A raw compatibility edge may name the future slot without
                    // changing failure timing or fabricating a local capability.
                    local.result.flows.set_antecedent(first_id, Some(future))?;
                    assert_eq!(local.flow(first).antecedent(), Some(future));
                    assert_eq!(local.flow_antecedent(first), Err(Error::InvalidSlot));
                    let allocated = local.new_flow(0, None, None);
                    assert_eq!(local.flow_id(allocated), future);
                    assert_eq!(local.import_flow(future)?, allocated);
                    assert_eq!(local.flow_antecedent(first)?, Some(allocated));
                    Ok(())
                })
                .expect("eligible source")
        })
        .unwrap();
}
