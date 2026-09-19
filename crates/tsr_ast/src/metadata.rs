//! Source metadata slices use owner-qualified backing, like syntax edge slices.
//! Cloning a SourceFile copies headers; exclusive builder mutation of an existing
//! element remains visible through every copied header. Replacing a header is
//! independent. No growable slice/capacity API is exposed here.

use crate::{
    AstBuilder, AstStorageData, AstView, CommentDirective, FileReference, JsString,
    MappedDiagnosticDirective, NodeId, Pragma,
};
use std::ops::{Deref, Range};
use ts_arena::{AuxId, Error};

#[derive(Debug)]
pub enum SourceMetadataData {
    Nodes(Box<[Option<NodeId>]>),
    Text(Box<[JsString]>),
    Comments(Box<[CommentDirective]>),
    Pragmas(Box<[Pragma]>),
    References(Box<[FileReference]>),
    DiagnosticDirectives(Box<[MappedDiagnosticDirective]>),
}
impl SourceMetadataData {
    pub(crate) fn validate(&self, view: AstView<'_>) -> Result<(), Error> {
        if let Self::Nodes(nodes) = self {
            for &node in nodes.iter().flatten() {
                view.node(node)?;
            }
        }
        Ok(())
    }
}

macro_rules! metadata_slice {
    ($name:ident, $read:ident, $element:ty, $variant:ident, $access:ident, $mutate:ident) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub struct $name {
            backing: Option<AuxId>,
            start: u32,
            len: u32,
        }
        impl $name {
            pub const fn empty() -> Self {
                Self {
                    backing: None,
                    start: 0,
                    len: 0,
                }
            }
            pub fn len(self) -> usize {
                self.len as usize
            }
            pub fn is_empty(self) -> bool {
                self.len == 0
            }
            pub fn is_nil(self) -> bool {
                self.backing.is_none()
            }
            pub fn slice(self, range: Range<usize>) -> Result<Self, Error> {
                if range.start > range.end || range.end > self.len() {
                    return Err(Error::InvalidSlot);
                }
                Ok(Self {
                    backing: self.backing,
                    start: self
                        .start
                        .checked_add(range.start as u32)
                        .ok_or(Error::InvalidSlot)?,
                    len: range.len() as u32,
                })
            }
        }
        pub struct $read<'a> {
            record: Option<crate::auxiliary::AuxRead<'a>>,
            start: usize,
            len: usize,
        }
        impl Deref for $read<'_> {
            type Target = [$element];
            fn deref(&self) -> &Self::Target {
                match &self.record {
                    None => &[],
                    Some(record) => match record.full() {
                        Some(AstStorageData::SourceMetadata(SourceMetadataData::$variant(
                            values,
                        ))) => &values[self.start..self.start + self.len],
                        _ => unreachable!("validated source metadata backing"),
                    },
                }
            }
        }
        impl AstBuilder {
            pub fn $access(&mut self, values: Vec<$element>) -> Result<$name, Error> {
                let len = u32::try_from(values.len()).map_err(|_| Error::InvalidSlot)?;
                let data = SourceMetadataData::$variant(values.into_boxed_slice());
                data.validate(self.view())?;
                let backing = self.push_auxiliary(AstStorageData::SourceMetadata(data));
                Ok($name {
                    backing: Some(backing),
                    start: 0,
                    len,
                })
            }
            /// Mutate existing elements under exclusive construction authority.
            /// Cross-owner storage stays read-only, including retained imports.
            pub fn $mutate(&mut self, slice: $name) -> Result<&mut [$element], Error> {
                let Some(backing) = slice.backing else {
                    return Ok(&mut []);
                };
                match self.auxiliary_mut(backing)? {
                    AstStorageData::SourceMetadata(SourceMetadataData::$variant(values)) => values
                        .get_mut(slice.start as usize..slice.start as usize + slice.len as usize)
                        .ok_or(Error::InvalidSlot),
                    _ => Err(Error::InvalidGraph),
                }
            }
        }
        impl<'a> AstView<'a> {
            pub fn $access(self, slice: $name) -> Result<$read<'a>, Error> {
                let record = slice.backing.map(|id| self.auxiliary(id)).transpose()?;
                if let Some(record) = &record {
                    match record.full() {
                        Some(AstStorageData::SourceMetadata(SourceMetadataData::$variant(
                            values,
                        ))) if (slice.start as usize)
                            .checked_add(slice.len as usize)
                            .is_some_and(|end| end <= values.len()) => {}
                        _ => return Err(Error::InvalidGraph),
                    }
                } else if slice.start != 0 || slice.len != 0 {
                    return Err(Error::InvalidSlot);
                }
                Ok($read {
                    record,
                    start: slice.start as usize,
                    len: slice.len as usize,
                })
            }
        }
    };
}
metadata_slice!(
    SourceNodeSlice,
    SourceNodeSliceRead,
    Option<NodeId>,
    Nodes,
    source_nodes,
    source_nodes_mut
);
metadata_slice!(
    SourceTextSlice,
    SourceTextSliceRead,
    JsString,
    Text,
    source_strings,
    source_strings_mut
);
metadata_slice!(
    CommentSlice,
    CommentSliceRead,
    CommentDirective,
    Comments,
    source_comments,
    source_comments_mut
);
metadata_slice!(
    PragmaSlice,
    PragmaSliceRead,
    Pragma,
    Pragmas,
    source_pragmas,
    source_pragmas_mut
);
metadata_slice!(
    ReferenceSlice,
    ReferenceSliceRead,
    FileReference,
    References,
    source_references,
    source_references_mut
);
metadata_slice!(
    DiagnosticDirectiveSlice,
    DiagnosticDirectiveSliceRead,
    MappedDiagnosticDirective,
    DiagnosticDirectives,
    source_diagnostic_directives,
    source_diagnostic_directives_mut
);
