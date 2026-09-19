//! Node.Text keeps stored bytes borrowed; only Go's concatenation paths allocate.
use crate::{
    AstView, JsString, Node, NodeAccess, NodeDataRead, NodeId, NodeRead, SyntaxKind as K,
    TextSliceRead,
};
use std::ops::Deref;
use ts_arena::Error;

pub struct NodeText<'a>(TextStorage<'a>);
enum TextStorage<'a> {
    Empty,
    Node(NodeRead<'a>),
    Fragment(TextSliceRead<'a>),
    Joined(Vec<u8>),
}
impl NodeText<'_> {
    /// Retain the existing string backing when text is stored in a node. Go's
    /// concatenation paths transfer their newly allocated bytes instead.
    pub fn into_js_string(self) -> JsString {
        match self.0 {
            TextStorage::Empty => JsString::default(),
            TextStorage::Node(node) => stored_owned_text(&node),
            TextStorage::Fragment(parts) => parts[0].clone(),
            TextStorage::Joined(bytes) => JsString::from_bytes(bytes),
        }
    }
    pub fn as_bytes(&self) -> &[u8] {
        match &self.0 {
            TextStorage::Empty => &[],
            TextStorage::Node(node) => stored_text(node),
            TextStorage::Fragment(parts) => parts[0].as_bytes(),
            TextStorage::Joined(bytes) => bytes,
        }
    }
}
impl Deref for NodeText<'_> {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}

// Byte access must not require a physical `&JsString`: compact identifiers
// will borrow their source suffix or exception pool through this same view.
macro_rules! text_field {
    ($node:expr, $variant:ident, $accessor:ident, $getter:ident) => {
        $node
            .$accessor()
            .unwrap_or_else(|| {
                panic!(
                    "interface conversion: ast.nodeData is *ast.{}, not *ast.{}",
                    $node.data().name(),
                    stringify!($variant)
                )
            })
            .$getter()
    };
}
macro_rules! stored_text_field {
    ($node:expr, $getter:ident) => {
        match $node.kind().known() {
            Some(K::Identifier) => text_field!($node, Identifier, as_identifier, $getter),
            Some(K::PrivateIdentifier) => {
                text_field!($node, PrivateIdentifier, as_private_identifier, $getter)
            }
            Some(K::StringLiteral) => text_field!($node, StringLiteral, as_string_literal, $getter),
            Some(K::NumericLiteral) => {
                text_field!($node, NumericLiteral, as_numeric_literal, $getter)
            }
            Some(K::BigIntLiteral) => {
                text_field!($node, BigIntLiteral, as_big_int_literal, $getter)
            }
            Some(K::NoSubstitutionTemplateLiteral) => {
                text_field!(
                    $node,
                    NoSubstitutionTemplateLiteral,
                    as_no_substitution_template_literal,
                    $getter
                )
            }
            Some(K::TemplateHead) => text_field!($node, TemplateHead, as_template_head, $getter),
            Some(K::TemplateMiddle) => {
                text_field!($node, TemplateMiddle, as_template_middle, $getter)
            }
            Some(K::TemplateTail) => text_field!($node, TemplateTail, as_template_tail, $getter),
            Some(K::RegularExpressionLiteral) => {
                text_field!(
                    $node,
                    RegularExpressionLiteral,
                    as_regular_expression_literal,
                    $getter
                )
            }
            _ => panic!("Unhandled case in Node.Text: *ast.{}", $node.data().name()),
        }
    };
}
fn stored_text<'read>(node: &'read NodeRead<'_>) -> &'read [u8] {
    stored_text_field!(node, text)
}
fn stored_owned_text(node: &NodeRead<'_>) -> JsString {
    stored_text_field!(node, text_owned)
}

impl Node {
    pub fn raw_text(&self) -> &[u8] {
        NodeAccess::raw_text(self)
    }
}
impl NodeRead<'_> {
    pub fn raw_text(&self) -> &[u8] {
        NodeAccess::raw_text(self)
    }
}

impl<'a> AstView<'a> {
    /// port: tsc/internal/ast/ast.go:Node.Text
    pub fn node_text(self, id: NodeId) -> Result<NodeText<'a>, Error> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || self.node_text_worker(id))
    }
    fn node_text_worker(self, mut id: NodeId) -> Result<NodeText<'a>, Error> {
        loop {
            let node = self.node(id)?;
            match node.kind().known() {
                Some(K::MetaProperty) => {
                    let NodeDataRead::MetaProperty(data) = node.data() else {
                        panic!(
                            "interface conversion: ast.nodeData is *ast.{}, not *ast.MetaProperty",
                            node.data().name()
                        );
                    };
                    id = data
                        .name()
                        .expect("runtime error: invalid memory address or nil pointer dereference");
                }
                Some(K::JsxNamespacedName) => {
                    let NodeDataRead::JsxNamespacedName(data) = node.data() else {
                        panic!(
                            "interface conversion: ast.nodeData is *ast.{}, not *ast.JsxNamespacedName",
                            node.data().name()
                        );
                    };
                    let namespace = self.node_text(data.namespace().expect(
                        "runtime error: invalid memory address or nil pointer dereference",
                    ))?;
                    let name = self.node_text(data.name().expect(
                        "runtime error: invalid memory address or nil pointer dereference",
                    ))?;
                    let mut bytes = Vec::with_capacity(namespace.len() + 1 + name.len());
                    bytes.extend_from_slice(&namespace);
                    bytes.push(b':');
                    bytes.extend_from_slice(&name);
                    return Ok(NodeText(TextStorage::Joined(bytes)));
                }
                Some(
                    kind @ (K::JSDocText | K::JSDocLink | K::JSDocLinkCode | K::JSDocLinkPlain),
                ) => {
                    let text = match (kind, node.data()) {
                        (K::JSDocText, NodeDataRead::JSDocText(data)) => data.text(),
                        (K::JSDocLink, NodeDataRead::JSDocLink(data)) => data.text(),
                        (K::JSDocLinkCode, NodeDataRead::JSDocLinkCode(data)) => data.text(),
                        (K::JSDocLinkPlain, NodeDataRead::JSDocLinkPlain(data)) => data.text(),
                        _ => panic!(
                            "interface conversion: ast.nodeData is *ast.{}, not *ast.{kind:?}",
                            node.data().name()
                        ),
                    };
                    let parts = self.text_slice(text)?;
                    return Ok(NodeText(match parts.len() {
                        0 => TextStorage::Empty,
                        1 => TextStorage::Fragment(parts),
                        _ => {
                            let len = parts
                                .iter()
                                .try_fold(0_usize, |total, part| total.checked_add(part.len()))
                                .expect("strings: Join output length overflow");
                            if len == 0 {
                                TextStorage::Empty
                            } else {
                                let mut bytes = Vec::with_capacity(len);
                                for part in parts.iter() {
                                    bytes.extend_from_slice(part.as_bytes());
                                }
                                TextStorage::Joined(bytes)
                            }
                        }
                    }));
                }
                _ => {
                    // Validate the selected payload now, even if the returned
                    // borrowed bytes are never subsequently inspected.
                    stored_text(&node);
                    return Ok(NodeText(TextStorage::Node(node)));
                }
            }
        }
    }
}
