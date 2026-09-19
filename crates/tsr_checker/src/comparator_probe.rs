//! Supplemental P0 comparator domain bridge. No expected outcomes live here.
use crate::{mapper::Mapper, object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use serde_json::{json, Value};
use ts_arena::NodeId;

impl CheckerState {
    pub(crate) fn residual_comparators(
        &mut self,
        first: NodeId,
        second: NodeId,
    ) -> Result<Value, Error> {
        let mut out = serde_json::Map::new();
        let a = self.new_intrinsic_type(tf::ANY, b"any")?;
        let b = self.new_intrinsic_type(tf::ANY, b"any")?;
        out.insert(
            "nil-and-intrinsic-creation".into(),
            self.comparator_matrix(&[None, Some(a), Some(b)])?,
        );
        let s1 = self.new_symbol(0, ts_ast::JsString::from_bytes(b"duplicate".as_slice()))?;
        let s2 = self.new_symbol(0, ts_ast::JsString::from_bytes(b"duplicate".as_slice()))?;
        out.insert(
            "duplicate-name-no-declarations".into(),
            json!([
                self.compare_symbols(Some(s2), Some(s1))? as i8,
                self.compare_symbols(Some(s1), Some(s2))? as i8,
                self.compare_symbols(None, Some(s1))? as i8,
                self.compare_symbols(Some(s1), None)? as i8
            ]),
        );
        let r1 = self.new_object_type(of::REVERSE_MAPPED, None)?;
        let r2 = self.new_object_type(of::REVERSE_MAPPED, None)?;
        out.insert(
            "reverse-mapped-no-symbol-or-mapper".into(),
            self.comparator_matrix(&[Some(r1), Some(r2)])?,
        );
        let target = self.new_object_type(of::INTERFACE, None)?;
        let d1 = self.create_deferred_type_reference(target, first, None, None)?;
        let d2 = self.create_deferred_type_reference(target, second, None, None)?;
        out.insert(
            "deferred-source-location".into(),
            self.comparator_matrix(&[Some(d1), Some(d2)])?,
        );
        let i1 = self.new_object_type(of::INSTANTIATION_EXPRESSION_TYPE, None)?;
        let i2 = self.new_object_type(of::INSTANTIATION_EXPRESSION_TYPE, None)?;
        self.types.instantiation_expression_mut(i1)?.node = Some(first);
        self.types.instantiation_expression_mut(i2)?.node = Some(second);
        out.insert(
            "instantiation-expression-location".into(),
            self.comparator_matrix(&[Some(i1), Some(i2)])?,
        );
        let simple1 = self.new_type_mapper(&[a], &[a])?;
        let simple2 = self.new_type_mapper(&[a], &[b])?;
        let array1 = self.new_type_mapper(&[a, b], &[a, a])?;
        let array2 = self.new_type_mapper(&[a, b], &[a, b])?;
        let merged1 = self.alloc_mapper(Mapper::Merged {
            first: simple1,
            second: array1,
        })?;
        let merged2 = self.alloc_mapper(Mapper::Merged {
            first: simple1,
            second: array2,
        })?;
        let mappers = [
            None,
            Some(simple1),
            Some(simple2),
            Some(array1),
            Some(array2),
            Some(merged1),
            Some(merged2),
        ];
        let mut matrix = Vec::new();
        for &a in &mappers {
            let mut row = Vec::new();
            for &b in &mappers {
                row.push(self.compare_type_mappers(a, b)? as i8);
            }
            matrix.push(row);
        }
        out.insert("nested-mapper-comparison".into(), json!(matrix));
        let d3 = self.create_deferred_type_reference(target, first, Some(merged1), None)?;
        let d4 = self.create_deferred_type_reference(target, first, Some(merged2), None)?;
        out.insert(
            "deferred-same-location-mapper".into(),
            self.comparator_matrix(&[Some(d3), Some(d4)])?,
        );
        out.insert(
            "nil-node-order".into(),
            json!([
                self.compare_nodes(None, Some(first))? as i8,
                self.compare_nodes(Some(first), None)? as i8,
                self.compare_nodes(None, None)? as i8
            ]),
        );
        Ok(Value::Object(out))
    }

    fn comparator_matrix(&self, types: &[Option<TypeId>]) -> Result<Value, Error> {
        let mut rows = Vec::new();
        for &a in types {
            let mut row = Vec::new();
            for &b in types {
                row.push(match (a, b) {
                    (None, None) => 0,
                    (None, Some(_)) => -1,
                    (Some(_), None) => 1,
                    (Some(a), Some(b)) => self.compare_types(a, b)? as i8,
                });
            }
            rows.push(row);
        }
        Ok(json!(rows))
    }
}
