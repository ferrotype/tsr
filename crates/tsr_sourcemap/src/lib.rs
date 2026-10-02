//! The pinned `internal/sourcemap` package: the source map generator, the
//! `mappings` decoder, ECMAScript line info and the document position mapper.
//!
//! Text stays bytes ([`tsr_jsstring::JsString`] and `&[u8]`). Generated and source columns
//! are UTF-16 code units, as the pinned package counts them; lines, columns and
//! indices are Go `int`s and stay signed 64-bit values here.
mod base64;
mod decoder;
mod generator;
mod lineinfo;
mod source;
mod source_mapper;
mod util;

pub use decoder::{
    decode_mappings, Mapping, MappingsDecoder, MISSING_LINE_OR_COLUMN, MISSING_NAME,
    MISSING_SOURCE, MISSING_UTF16_COLUMN,
};
pub use generator::{new_generator, Generator, NameIndex, RawSourceMap, SourceIndex};
pub use lineinfo::{create_ecma_line_info, EcmaLineInfo};
pub use source::Source;
pub use source_mapper::{
    get_document_position_mapper, DocumentPosition, DocumentPositionMapper, Host,
};
pub use util::try_get_source_mapping_url;

/// The pinned `core.UTF16Offset`: a Go `int` counting UTF-16 code units.
pub type Utf16Offset = isize;

/// An error the pinned package creates with `errors.New`; its message is the
/// exact upstream text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    message: &'static str,
}

impl Error {
    pub(crate) const fn new(message: &'static str) -> Self {
        Self { message }
    }
    /// The upstream error text.
    pub fn message(&self) -> &'static str {
        self.message
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod decoder_tests;
#[cfg(test)]
mod generator_tests;
#[cfg(test)]
mod source_mapper_tests;
