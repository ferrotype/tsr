//! Go's open signed kind domain at AST factory and codec boundaries.

use std::fmt;

use crate::SyntaxKind;

/// Unlike the scanner's known-token enum, factories accept every Go int16 kind.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct NodeKind(i16);

impl NodeKind {
    pub const fn from_raw(value: i16) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> i16 {
        self.0
    }

    pub fn known(self) -> Option<SyntaxKind> {
        // Deliberate Go reinterpretation; negative values are outside ALL.
        SyntaxKind::from_u16(self.0 as u16)
    }
}

impl From<SyntaxKind> for NodeKind {
    fn from(value: SyntaxKind) -> Self {
        Self(value as i16)
    }
}

impl PartialEq<SyntaxKind> for NodeKind {
    fn eq(&self, other: &SyntaxKind) -> bool {
        self.0 == *other as i16
    }
}

impl PartialEq<NodeKind> for SyntaxKind {
    fn eq(&self, other: &NodeKind) -> bool {
        other == self
    }
}

impl fmt::Display for NodeKind {
    /// upstream: tsc/internal/ast/kind_stringer_generated.go:Kind.String
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(kind) = self.known() {
            write!(formatter, "Kind{}", kind.as_str())
        } else if self.0 == SyntaxKind::ALL.len() as i16 {
            formatter.write_str("KindCount")
        } else {
            write!(formatter, "Kind({})", self.0)
        }
    }
}

impl fmt::Debug for NodeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrowed_wire_kinds_keep_go_stringer_names_and_unknown_values() {
        // Includes decoder-observed aliases and signed-domain boundaries.
        for (wire, expected) in [
            (0x1_0000_u32, "KindUnknown"),
            (0xffff, "Kind(-1)"),
            (351, "KindCount"),
            (352, "Kind(352)"),
            (0x8000, "Kind(-32768)"),
        ] {
            assert_eq!(NodeKind::from_raw(wire as i16).to_string(), expected);
        }
        assert_eq!(
            NodeKind::from(SyntaxKind::SourceFile),
            SyntaxKind::SourceFile
        );
        assert_eq!(NodeKind::from_raw(-1).known(), None);
    }
}
