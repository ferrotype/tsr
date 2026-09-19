//! Short eager parser lists avoid a temporary heap backing before AST storage.
use ts_ast::NodeId;

pub(crate) enum ListBuffer {
    Inline {
        nodes: [Option<NodeId>; 4],
        len: usize,
    },
    Heap(Vec<NodeId>),
}

impl ListBuffer {
    pub(crate) fn inline() -> Self {
        Self::Inline {
            nodes: [None; 4],
            len: 0,
        }
    }
    pub(crate) fn heap() -> Self {
        Self::Heap(Vec::new())
    }
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Inline { len, .. } => *len,
            Self::Heap(nodes) => nodes.len(),
        }
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub(crate) fn inline_nodes(&self) -> Option<&[Option<NodeId>]> {
        match self {
            Self::Inline { nodes, len } => Some(&nodes[..*len]),
            Self::Heap(_) => None,
        }
    }
    pub(crate) fn push(&mut self, node: NodeId) {
        match self {
            Self::Inline { nodes, len } if *len < nodes.len() => {
                nodes[*len] = Some(node);
                *len += 1;
            }
            Self::Inline { nodes, len } => {
                let mut spilled = Vec::with_capacity(8);
                spilled.extend(
                    nodes[..*len]
                        .iter()
                        .map(|node| node.expect("filled parser list slot")),
                );
                spilled.push(node);
                *self = Self::Heap(spilled);
            }
            Self::Heap(nodes) => nodes.push(node),
        }
    }
    pub(crate) fn append(&mut self, suffix: &mut Vec<NodeId>) {
        match self {
            Self::Inline { nodes, len } => {
                let required = len.checked_add(suffix.len()).expect("capacity overflow");
                if required <= nodes.len() {
                    for node in suffix.drain(..) {
                        nodes[*len] = Some(node);
                        *len += 1;
                    }
                } else {
                    // Vec::append reserves for the complete suffix. Preserve that
                    // single reservation when an inline prefix has to spill.
                    let capacity = if *len == 0 { required } else { 8.max(required) };
                    let mut spilled = Vec::with_capacity(capacity);
                    spilled.extend(
                        nodes[..*len]
                            .iter()
                            .map(|node| node.expect("filled parser list slot")),
                    );
                    spilled.append(suffix);
                    *self = Self::Heap(spilled);
                }
            }
            Self::Heap(nodes) => nodes.append(suffix),
        }
    }
    pub(crate) fn into_vec(self) -> Vec<NodeId> {
        match self {
            Self::Heap(nodes) => nodes,
            Self::Inline { nodes, len } => {
                let mut values = Vec::with_capacity(if len == 0 { 0 } else { 4 });
                values.extend(
                    nodes
                        .into_iter()
                        .take(len)
                        .map(|node| node.expect("filled parser list slot")),
                );
                values
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_arena::Counters;
    use ts_ast::AstBuilder;
    use ts_jsstring::SourceText;

    fn ids(count: u32) -> Vec<NodeId> {
        let owner = AstBuilder::new(SourceText::default(), &Counters::new())
            .id()
            .arena();
        (1..=count)
            .map(|slot| NodeId::from_parts(owner, slot).unwrap())
            .collect()
    }
    #[test]
    fn inline_boundary_spills_once_and_keeps_order() {
        let nodes = ids(17);
        let mut buffer = ListBuffer::inline();
        assert!(buffer.is_empty());
        for (index, &node) in nodes.iter().enumerate() {
            buffer.push(node);
            assert_eq!(buffer.len(), index + 1);
            assert_eq!(buffer.inline_nodes().is_some(), index < 4);
            if index == 4 {
                let ListBuffer::Heap(values) = &buffer else {
                    unreachable!()
                };
                assert_eq!(values.capacity(), 8);
            }
        }
        assert_eq!(buffer.into_vec(), nodes);
    }
    #[test]
    fn append_reserves_the_whole_suffix_and_preserves_empty_suffix_storage() {
        let nodes = ids(41);
        for total in [5, 9, 41] {
            for prefix_len in [0, 3, 4, 5] {
                let mut buffer = ListBuffer::inline();
                let mut control = Vec::new();
                for &node in &nodes[..prefix_len] {
                    buffer.push(node);
                    control.push(node);
                }
                let mut suffix = nodes[prefix_len..total].to_vec();
                let suffix_capacity = suffix.capacity();
                control.append(&mut suffix.clone());
                buffer.append(&mut suffix);
                assert!(suffix.is_empty());
                assert_eq!(suffix.capacity(), suffix_capacity);
                let values = buffer.into_vec();
                assert_eq!(values, control);
                assert_eq!(values.capacity(), control.capacity());
            }
        }
        let mut buffer = ListBuffer::inline();
        let mut empty = Vec::with_capacity(7);
        buffer.append(&mut empty);
        assert!(buffer.inline_nodes().unwrap().is_empty());
        assert_eq!(empty.capacity(), 7);
    }
}
