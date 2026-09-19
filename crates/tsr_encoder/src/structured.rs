//! The pinned encoder's minimal MessagePack writer, including narrowing at the wire.
use ts_ast::{FileReference, MappedDiagnosticDirective, SpanSegment};
use ts_jsstring::PositionMap;
pub(crate) const NONE: u32 = u32::MAX;
// port: tsc/internal/api/encoder/encoder.go:msgpackWriteArrayHeader
pub(crate) fn array(out: &mut Vec<u8>, len: usize) {
    if len <= 15 {
        out.push(0x90 | len as u8);
    } else if len <= 65535 {
        out.push(0xdc);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xdd);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
}
// port: tsc/internal/api/encoder/encoder.go:msgpackWriteUint
pub(crate) fn uint(out: &mut Vec<u8>, value: u32) {
    if value <= 0x7f {
        out.push(value as u8);
    } else if value <= 0xff {
        out.extend([0xcc, value as u8]);
    } else if value <= 0xffff {
        out.push(0xcd);
        out.extend_from_slice(&(value as u16).to_be_bytes());
    } else {
        out.push(0xce);
        out.extend_from_slice(&value.to_be_bytes());
    }
}
// port: tsc/internal/api/encoder/encoder.go:msgpackWriteString
pub(crate) fn string(out: &mut Vec<u8>, value: &[u8]) {
    let len = value.len();
    if len <= 31 {
        out.push(0xa0 | len as u8);
    } else if len <= 255 {
        out.extend([0xd9, len as u8]);
    } else if len <= 65535 {
        out.push(0xda);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xdb);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend_from_slice(value);
}
// port: tsc/internal/api/encoder/encoder.go:msgpackWriteBool
pub(crate) fn boolean(out: &mut Vec<u8>, value: bool) {
    out.push(if value { 0xc3 } else { 0xc2 });
}
// port: tsc/internal/api/encoder/encoder.go:encodeFileReferences
pub(crate) fn references(
    values: &[FileReference],
    positions: &PositionMap,
    out: &mut Vec<u8>,
) -> u32 {
    if values.is_empty() {
        return NONE;
    }
    let offset = out.len() as u32;
    array(out, values.len());
    for value in values {
        array(out, 5);
        uint(
            out,
            positions.utf8_to_utf16(value.loc.pos() as isize) as u32,
        );
        uint(
            out,
            positions.utf8_to_utf16(value.loc.end() as isize) as u32,
        );
        string(out, value.file_name.as_bytes());
        uint(out, value.resolution_mode as u32);
        boolean(out, value.preserve);
    }
    offset
}
// port: tsc/internal/api/encoder/encoder.go:encodeSpanMap
pub(crate) fn spans(
    values: Option<&[SpanSegment]>,
    virtual_map: &PositionMap,
    original_map: &PositionMap,
    out: &mut Vec<u8>,
) -> u32 {
    let Some(values) = values else {
        return NONE;
    };
    let offset = out.len() as u32;
    array(out, values.len());
    for value in values {
        let extended = value.features != SpanSegment::FEATURE_ALL;
        array(out, if extended { 6 } else { 5 });
        let vs = virtual_map.utf8_to_utf16(value.virtual_start as isize);
        let ve = virtual_map.utf8_to_utf16(value.virtual_end as isize);
        let os = original_map.utf8_to_utf16(value.original_start as isize);
        let oe = original_map.utf8_to_utf16(value.original_end as isize);
        for word in [
            vs as u32,
            ve.wrapping_sub(vs) as u32,
            os as u32,
            oe.wrapping_sub(os) as u32,
            value.kind as u32,
        ] {
            uint(out, word);
        }
        if extended {
            uint(out, value.features as u32);
        }
    }
    offset
}
// port: tsc/internal/api/encoder/encoder.go:encodeDiagnosticDirectives
pub(crate) fn directives(
    values: &[MappedDiagnosticDirective],
    virtual_map: &PositionMap,
    original_map: &PositionMap,
    out: &mut Vec<u8>,
) -> u32 {
    if values.is_empty() {
        return NONE;
    }
    let offset = out.len() as u32;
    array(out, values.len());
    for value in values {
        array(out, 6);
        let os = original_map.utf8_to_utf16(value.original_range.pos() as isize);
        let oe = original_map.utf8_to_utf16(value.original_range.end() as isize);
        let vs = virtual_map.utf8_to_utf16(value.virtual_range.pos() as isize);
        let ve = virtual_map.utf8_to_utf16(value.virtual_range.end() as isize);
        for word in [
            os as u32,
            oe.wrapping_sub(os) as u32,
            vs as u32,
            ve.wrapping_sub(vs) as u32,
            u32::from(value.policy),
            value.unused_code as u32,
        ] {
            uint(out, word);
        }
    }
    offset
}
// port: tsc/internal/api/encoder/encoder.go:encodeStringArray
pub(crate) fn strings<'a>(
    values: impl ExactSizeIterator<Item = &'a [u8]>,
    out: &mut Vec<u8>,
) -> u32 {
    if values.len() == 0 {
        return NONE;
    }
    let offset = out.len() as u32;
    array(out, values.len());
    for value in values {
        string(out, value);
    }
    offset
}
