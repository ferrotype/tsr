//! Owner-contained storage for the encoder's per-SourceFile lazy node table.
//! The encoder builds the traversal; this type adds no AST-to-encoder dependency.

use crate::{runtime_node_id, AstView, NodeAccess, NodeId};
use std::{
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    sync::{Mutex, OnceLock},
};
use ts_arena::Error;

#[derive(Debug)]
pub struct NodeIndexCache {
    nodes: Vec<Option<NodeId>>,
    sorted: OnceLock<Vec<u32>>,
    sort_lock: Mutex<()>,
}

impl NodeIndexCache {
    pub fn new(nodes: Vec<Option<NodeId>>) -> Self {
        Self {
            nodes,
            sorted: OnceLock::new(),
            sort_lock: Mutex::new(()),
        }
    }
    pub fn nodes(&self) -> &[Option<NodeId>] {
        &self.nodes
    }

    /// Source lookup initializes its sorted table before dereferencing the query.
    /// Nil is an upstream contract panic; a non-member is the zero sentinel.
    /// port: tsc/internal/api/encoder/encoder.go:NodeIndexTable.GetIndex
    pub fn get_index(
        &self,
        view: AstView<'_>,
        node: Option<&dyn NodeAccess>,
    ) -> Result<u32, Error> {
        let sorted = self.sorted_indexes(view)?;
        let target = node.expect("nil node in NodeIndexTable.GetIndex");
        let target = runtime_node_id(target);
        let mut low = 0_isize;
        let mut high = sorted.len() as isize - 1;
        while low <= high {
            let middle = (low + ((high - low) >> 1)) as usize;
            let member = self.nodes[sorted[middle] as usize].expect("sorted table excludes nil");
            let member = view.node(member)?;
            match runtime_node_id(&member).cmp(&target) {
                std::cmp::Ordering::Less => low = middle as isize + 1,
                std::cmp::Ordering::Greater => high = middle as isize - 1,
                std::cmp::Ordering::Equal => return Ok(sorted[middle]),
            }
        }
        Ok(0)
    }

    fn sorted_indexes(&self, view: AstView<'_>) -> Result<&[u32], Error> {
        if let Some(sorted) = self.sorted.get() {
            return Ok(sorted);
        }
        let guard = self
            .sort_lock
            .lock()
            .expect("node-index sorting unlocks before resuming panic");
        if let Some(sorted) = self.sorted.get() {
            return Ok(sorted);
        }
        // Ownership errors leave the cache retryable. Validate once before the
        // comparator, without assigning any semantic node IDs prematurely.
        for &node in self.nodes.iter().flatten() {
            view.node(node)?;
        }
        let sorted = catch_unwind(AssertUnwindSafe(|| {
            let mut sorted = Vec::with_capacity(self.nodes.len());
            for (index, node) in self.nodes.iter().enumerate() {
                if node.is_some() {
                    sorted.push(index as u32);
                }
            }
            ts_core::sort_like_go(&mut sorted, &mut |&left, &right| {
                let left = self.nodes[left as usize].expect("sorted table excludes nil");
                let right = self.nodes[right as usize].expect("sorted table excludes nil");
                let left =
                    runtime_node_id(&view.node(left).expect("validated immutable index node"));
                let right =
                    runtime_node_id(&view.node(right).expect("validated immutable index node"));
                left.cmp(&right)
            });
            sorted
        }));
        match sorted {
            Ok(sorted) => {
                self.sorted
                    .set(sorted)
                    .expect("only the lock holder initializes sorted indexes");
            }
            Err(panic) => {
                // Go's sync.Once stays completed after a panic in SortFunc.
                self.sorted
                    .set(Vec::new())
                    .expect("only the lock holder initializes sorted indexes");
                drop(guard);
                resume_unwind(panic);
            }
        }
        Ok(self.sorted.get().expect("initialized sorted indexes"))
    }
}
