//! Pinned protocol-8 serialization, decoding and generated layout metadata.
//! Node indexing uses the source file's owned cache and an explicit JSDoc provider.

use ts_ast::SyntaxKind;

// port: tsc/internal/api/encoder/encoder.go:init#1
const _: () = assert!(
    (SyntaxKind::LastUnaryOperator as u16) <= 0x3f,
    "unary operator exceeds six-bit common data"
);

mod decoder;
mod encode;
mod generated;
mod runtime_generated;
mod string_table;
mod structured;
mod walk;

pub use decoder::{decode_nodes, decode_source_file, DecodeError, DecodedTree};
pub use encode::{
    build_node_index_table, encode_node, encode_optional_node, encode_source_file,
    get_node_index_table, source_file_hash, Encoded,
};

pub use generated::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum DataType {
    Children = NODE_DATA_TYPE_CHILDREN,
    String = NODE_DATA_TYPE_STRING,
    Extended = NODE_DATA_TYPE_EXTENDED_DATA,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildType {
    Node,
    NodeList,
    RawNodeList,
    ModifierList,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChildProperty {
    pub name: &'static str,
    pub child_type: ChildType,
    pub optional: bool,
}

/// An automatically packed field, relative to the start of common data (bit 24).
/// Empty `kind_values` means a boolean. For an optional kind union, encoded zero
/// means absent and the first listed kind has encoded value one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommonDataField {
    pub name: &'static str,
    pub bit_position: u8,
    pub bit_width: u8,
    pub optional: bool,
    pub kind_values: &'static [SyntaxKind],
}

#[derive(Debug)]
pub struct NodeLayout {
    pub name: &'static str,
    pub kinds: &'static [SyntaxKind],
    pub data_type: DataType,
    pub children: &'static [ChildProperty],
    pub common_data: &'static [CommonDataField],
    pub text_member: Option<&'static str>,
    pub hand_written_common_data: bool,
}

impl NodeLayout {
    /// A custom codec must supply extended data or the complete common-data word.
    pub fn requires_custom_codec(&self) -> bool {
        self.data_type == DataType::Extended || self.hand_written_common_data
    }

    /// Only ordinary one-child nodes encode that child's index directly.
    pub fn single_child(&self) -> Option<&ChildProperty> {
        if self.data_type != DataType::Extended && self.children.len() == 1 {
            self.children.first()
        } else {
            None
        }
    }
}
