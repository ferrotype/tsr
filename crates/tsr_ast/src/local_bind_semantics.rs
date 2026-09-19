//! The shared Node getter rules over borrowed rows and local graph links.
use super::{BindList, BindNode, BindRead};

macro_rules! field {
    ($node:expr, $variant:ident, $accessor:ident, $field:ident) => {
        match $node.$accessor() {
            Some(data) => data.$field(),
            None => panic!(
                "interface conversion: ast.nodeData is *ast.{}, not *ast.{}",
                $node.payload_name(),
                stringify!($variant)
            ),
        }
    };
}

macro_rules! shape {
    ($node:expr; $($variant:ident $accessor:ident => |$data:ident| $value:expr,)*) => {
        match $node.header.actual_shape() {
            $(crate::local_read_generated::shapes::$variant => {
                let $data = $node.$accessor().expect("selected local payload shape");
                $value
            },)*
            _ => None,
        }
    };
}

macro_rules! locals_shape {
    ($node:expr; $($variant:ident),*) => {
        matches!($node.header.actual_shape(), $(crate::local_read_generated::shapes::$variant)|*)
    };
}

impl<'scope> BindRead<'scope, '_> {
    pub fn is_locals_container(&self) -> bool {
        crate::node_semantics::locals_container_shapes!(locals_shape, self)
    }
    crate::node_semantics::reads!(BindNode<'scope>, BindList<'scope>, field; pub,);
    crate::node_semantics::shape_reads!(BindNode<'scope>, BindList<'scope>, shape; pub,);
}
