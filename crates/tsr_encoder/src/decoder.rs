//! Bottom-up protocol decoding. Returned errors, panics and sibling-chain
//! nontermination follow the pinned Go boundary; adapters apply an external watchdog.
use ts_arena::Counters;
use ts_ast::{
    AstBuilder, ExternalModuleIndicatorOptions, FactoryMethods, JsString, NodeId, NodeKind,
    NodeListId, NodeSlice, SourceFileParseOptions, SyntaxKind,
};
use ts_core::TextRange;
use ts_jsstring::SourceText;

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    Baseline(String),
}
impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Baseline(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for DecodeError {}
#[derive(Debug)]
pub struct DecodedTree {
    pub builder: AstBuilder,
    pub root: Option<NodeId>,
}
// port: tsc/internal/api/encoder/decoder.go:DecodeNodes
pub fn decode_nodes(data: &[u8], counters: &Counters) -> Result<DecodedTree, DecodeError> {
    Decoder::new(data, counters)?.decode()
}
// port: tsc/internal/api/encoder/decoder.go:DecodeSourceFile
pub fn decode_source_file(data: &[u8], counters: &Counters) -> Result<DecodedTree, DecodeError> {
    let tree = decode_nodes(data, counters)?;
    let root = tree
        .root
        .expect("nil SourceFile root returned by DecodeNodes");
    let kind = tree
        .builder
        .view()
        .node(root)
        .expect("decoded node owner")
        .kind();
    if kind != SyntaxKind::SourceFile {
        return Err(DecodeError::Baseline(format!(
            "expected SourceFile root, got {kind}"
        )));
    }
    Ok(tree)
}
pub(crate) struct Decoder<'a> {
    raw: &'a [u8],
    str_table: usize,
    ext_data: usize,
    node_offset: usize,
    node_count: usize,
    all_strings: JsString,
    pub(crate) factory: AstBuilder,
    nodes: Vec<Option<NodeId>>,
    lists: Vec<Option<NodeListId>>,
    arena: Vec<Option<NodeId>>,
    arena_used: usize,
}
impl<'a> Decoder<'a> {
    // port: tsc/internal/api/encoder/decoder.go:newASTDecoder
    fn new(raw: &'a [u8], counters: &Counters) -> Result<Self, DecodeError> {
        let error = |message| Err(DecodeError::Baseline(message));
        if raw.len() < 44 {
            return error(format!("data too short for header: {} bytes", raw.len()));
        }
        let version = raw[3];
        if version != 8 {
            return error(format!(
                "unsupported protocol version {version} (expected 8)"
            ));
        }
        let str_table = read_le32(raw, 24) as usize;
        let str_data = read_le32(raw, 28) as usize;
        let ext_data = read_le32(raw, 32) as usize;
        let node_offset = read_le32(raw, 40) as usize;
        let data_length = raw.len() as u32;
        if [str_table, str_data, ext_data, node_offset]
            .iter()
            .any(|offset| *offset > data_length as usize)
        {
            return error(format!(
                "invalid AST header offsets: offsets exceed data length ({})",
                raw.len() as u32
            ));
        }
        if !(str_table <= str_data && str_data <= ext_data && ext_data <= node_offset) {
            return error(format!(
                "invalid AST header offsets: expected strTable <= strData <= extData <= nodeOff (got {str_table}, {str_data}, {ext_data}, {node_offset})"
            ));
        }
        let all_strings = JsString::from_bytes(&raw[str_data..]);
        let factory = AstBuilder::new(SourceText::from_loaded_bytes(&b""[..]), counters);
        Ok(Self {
            raw,
            str_table,
            ext_data,
            node_offset,
            node_count: (raw.len() - node_offset) / 28,
            all_strings,
            factory,
            nodes: Vec::new(),
            lists: Vec::new(),
            arena: Vec::new(),
            arena_used: 0,
        })
    }
    // port: tsc/internal/api/encoder/decoder.go:astDecoder.nodeField
    fn node_field(&self, index: usize, field: usize) -> u32 {
        read_le32(self.raw, self.node_offset + index * 28 + field)
    }
    // port: tsc/internal/api/encoder/decoder.go:astDecoder.getString
    pub(crate) fn get_string(&self, index: u32) -> JsString {
        let offset = self.str_table + index as usize * 4;
        let start = read_le32(self.raw, offset) as usize;
        let end = read_le32(self.raw, offset + 4) as usize;
        let _ = &self.all_strings.as_bytes()[start..end];
        self.all_strings
            .slice(start..end)
            .expect("checked Go string slice bounds")
    }
    // port: tsc/internal/api/encoder/decoder.go:astDecoder.collectChildren
    fn children(&self, index: usize, out: &mut Vec<usize>) {
        out.clear();
        let first = index + 1;
        if first >= self.node_count || self.node_field(first, 16) != index as u32 {
            return;
        }
        out.push(first);
        let mut next = self.node_field(first, 12) as usize;
        while next != 0 {
            out.push(next);
            next = self.node_field(next, 12) as usize;
        }
    }
    // port: tsc/internal/api/encoder/decoder.go:astDecoder.decode
    fn decode(mut self) -> Result<DecodedTree, DecodeError> {
        if self.node_count < 2 {
            return Err(DecodeError::Baseline("no nodes to decode".into()));
        }
        self.nodes = vec![None; self.node_count];
        self.lists = vec![None; self.node_count];
        self.arena = vec![None; self.node_count];
        let mut children = Vec::new();
        for index in (1..self.node_count).rev() {
            let wire_kind = self.node_field(index, 0);
            let pos = self.node_field(index, 4);
            let end = self.node_field(index, 8);
            let data = self.node_field(index, 20);
            self.children(index, &mut children);
            if wire_kind == u32::MAX {
                self.reserve_arena(children.len());
                let nodes = children
                    .iter()
                    .filter_map(|child| self.nodes[*child])
                    .map(Some)
                    .collect();
                let slice = self.factory.node_slice(nodes).expect("decoded list owner");
                self.lists[index] = Some(
                    self.factory
                        .new_list(TextRange::new(i64::from(pos), i64::from(end)), slice)
                        .expect("decoded list owner"),
                );
                continue;
            }
            let kind = NodeKind::from_raw(wire_kind as i16);
            let node = self
                .create_generated(kind, data, &children)
                .map_err(|message| {
                    DecodeError::Baseline(format!("at node {index} (kind {kind}): {message}"))
                })?;
            let flags = self.node_field(index, 24);
            ts_ast::Factory::set_node_range(
                &mut self.factory,
                node,
                TextRange::new(i64::from(pos), i64::from(end)),
            );
            ts_ast::Factory::set_node_flags(&mut self.factory, node, flags);
            self.nodes[index] = Some(node);
        }
        Ok(DecodedTree {
            root: self.nodes[1],
            builder: self.factory,
        })
    }
    // port: tsc/internal/api/encoder/decoder.go:astDecoder.allocNodeSlice
    fn reserve_arena(&mut self, capacity: usize) {
        let end = self.arena_used + capacity;
        let _ = &self.arena[self.arena_used..end];
        self.arena_used = end;
    }
    pub(crate) fn generated_raw_list(&mut self, children: &[usize]) -> NodeSlice {
        self.reserve_arena(children.len());
        // Pinned generated SyntaxList/JSDocTypeLiteral decoders assign to the
        // zero-length result of allocNodeSlice. Retain that observed bounds panic.
        let mut nodes: Vec<Option<NodeId>> = Vec::with_capacity(children.len());
        for (index, child) in children.iter().enumerate() {
            nodes[index] = self.nodes[*child];
        }
        self.factory
            .node_slice(nodes)
            .expect("decoded raw-list owner")
    }
    // port: tsc/internal/api/encoder/decoder.go:astDecoder.nodeAt
    pub(crate) fn node_at(&self, index: usize) -> Option<NodeId> {
        if index == 0 {
            None
        } else {
            self.nodes[index]
        }
    }
    // port: tsc/internal/api/encoder/decoder.go:astDecoder.nodeListAt
    pub(crate) fn list_at(&self, index: usize) -> Option<NodeListId> {
        if index == 0 {
            None
        } else {
            self.lists[index]
        }
    }
    // port: tsc/internal/api/encoder/decoder.go:astDecoder.modifierListAt
    // port: tsc/internal/api/encoder/decoder.go:astDecoder.getModifierList
    pub(crate) fn modifier_at(&mut self, index: usize) -> Option<NodeListId> {
        let list = self.list_at(index)?;
        let list = self
            .factory
            .view()
            .list(list)
            .expect("decoded list owner")
            .to_owned();
        // A modifier list carries the flags of its modifiers, which
        // `NewModifierList` computes.
        let modifiers = ts_ast::RuntimeFactory::new_modifier_list(&mut self.factory, list.nodes());
        self.factory
            .set_list_location(modifiers, list.loc())
            .expect("decoded modifier owner");
        Some(modifiers)
    }
    // port: tsc/internal/api/encoder/decoder.go:newChildIter
    // port: tsc/internal/api/encoder/decoder.go:childIterator.nextIf
    // port: tsc/internal/api/encoder/decoder.go:childIterator.next
    pub(crate) fn next_if(children: &[usize], cursor: &mut usize, mask: u8, bit: u8) -> usize {
        if mask & (1 << bit) == 0 || *cursor >= children.len() {
            return 0;
        }
        let next = children[*cursor];
        *cursor += 1;
        next
    }
    // port: tsc/internal/api/encoder/decoder.go:astDecoder.createNode
    // upstream: tsc/internal/api/encoder/decoder_generated.go:astDecoder.createExtendedNode
    pub(crate) fn create_extended(
        &mut self,
        kind: NodeKind,
        data: u32,
        children: &[usize],
    ) -> Result<NodeId, String> {
        let offset = self.ext_data + (data & 0x00ff_ffff) as usize;
        macro_rules! literal {
            ($constructor:ident) => {{
                let text = self.get_string(read_le32(self.raw, offset));
                let flags = read_le32(self.raw, offset + 4) as i32;
                self.factory.$constructor(text, flags)
            }};
        }
        macro_rules! template {
            ($constructor:ident) => {{
                let text = self.get_string(read_le32(self.raw, offset));
                let raw = self.get_string(read_le32(self.raw, offset + 4));
                let flags = read_le32(self.raw, offset + 8) as i32;
                self.factory.$constructor(text, raw, flags)
            }};
        }
        Ok(match kind.known() {
            // port: tsc/internal/api/encoder/decoder.go:astDecoder.decodeExtendedData_SourceFile
            Some(SyntaxKind::SourceFile) => {
                let text = self.get_string(read_le32(self.raw, offset));
                let file_name = self.get_string(read_le32(self.raw, offset + 4));
                let path = self.get_string(read_le32(self.raw, offset + 8));
                let options = read_le32(self.raw, 20);
                let mut statements = None;
                let mut eof = None;
                for &child in children {
                    if self.node_field(child, 0) == u32::MAX {
                        statements = self.list_at(child);
                    } else if let Some(id) = self.nodes[child] {
                        if self.factory.view().node(id).expect("decoded owner").kind()
                            == SyntaxKind::EndOfFile
                        {
                            eof = Some(id);
                        }
                    }
                }
                let eof =
                    eof.unwrap_or_else(|| self.factory.new_token(SyntaxKind::EndOfFile.into()));
                self.factory.new_source_file(
                    SourceFileParseOptions {
                        file_name,
                        path,
                        external_module_indicator_options: ExternalModuleIndicatorOptions {
                            jsx: options & 1 != 0,
                            force: options & 2 != 0,
                        },
                    },
                    SourceText::from_loaded_bytes(text.as_bytes()),
                    statements,
                    Some(eof),
                )
            }
            // port: tsc/internal/api/encoder/decoder.go:astDecoder.decodeExtendedData_StringLiteral
            Some(SyntaxKind::StringLiteral) => literal!(new_string_literal),
            // port: tsc/internal/api/encoder/decoder.go:astDecoder.decodeExtendedData_NumericLiteral
            Some(SyntaxKind::NumericLiteral) => literal!(new_numeric_literal),
            // port: tsc/internal/api/encoder/decoder.go:astDecoder.decodeExtendedData_BigIntLiteral
            Some(SyntaxKind::BigIntLiteral) => literal!(new_big_int_literal),
            // port: tsc/internal/api/encoder/decoder.go:astDecoder.decodeExtendedData_RegularExpressionLiteral
            Some(SyntaxKind::RegularExpressionLiteral) => literal!(new_regular_expression_literal),
            // port: tsc/internal/api/encoder/decoder.go:astDecoder.decodeExtendedData_NoSubstitutionTemplateLiteral
            Some(SyntaxKind::NoSubstitutionTemplateLiteral) => {
                literal!(new_no_substitution_template_literal)
            }
            // port: tsc/internal/api/encoder/decoder.go:astDecoder.decodeExtendedData_TemplateHead
            Some(SyntaxKind::TemplateHead) => template!(new_template_head),
            // port: tsc/internal/api/encoder/decoder.go:astDecoder.decodeExtendedData_TemplateMiddle
            Some(SyntaxKind::TemplateMiddle) => template!(new_template_middle),
            // port: tsc/internal/api/encoder/decoder.go:astDecoder.decodeExtendedData_TemplateTail
            Some(SyntaxKind::TemplateTail) => template!(new_template_tail),
            _ => return Err(format!("unknown extended data node kind {kind}")),
        })
    }
}
// port: tsc/internal/api/encoder/decoder.go:readLE32
fn read_le32(data: &[u8], offset: usize) -> u32 {
    data.get(offset..offset + 4).map_or(0, |bytes| {
        u32::from_le_bytes(bytes.try_into().expect("four bytes"))
    })
}
