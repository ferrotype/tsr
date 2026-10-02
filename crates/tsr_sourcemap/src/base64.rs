//! The two `encoding/base64` operations the pinned package calls on
//! `base64.StdEncoding` (the standard alphabet with `=` padding, not strict):
//! encoding the data URL and `DecodeString` for an inline source map.

const ENCODE_STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const PAD_CHAR: u8 = b'=';
const INVALID: u8 = 0xff;

/// Go's `base64.StdEncoding.EncodeToString` (the pinned code streams the same
/// bytes through `base64.NewEncoder` and closes it, which pads the tail).
pub(crate) fn std_encode(src: &[u8], out: &mut Vec<u8>) {
    out.reserve(src.len().div_ceil(3) * 4);
    let mut chunks = src.chunks_exact(3);
    for chunk in &mut chunks {
        let val = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        out.extend_from_slice(&[
            ENCODE_STD[(val >> 18 & 0x3f) as usize],
            ENCODE_STD[(val >> 12 & 0x3f) as usize],
            ENCODE_STD[(val >> 6 & 0x3f) as usize],
            ENCODE_STD[(val & 0x3f) as usize],
        ]);
    }
    let rest = chunks.remainder();
    if rest.is_empty() {
        return;
    }
    let mut val = u32::from(rest[0]) << 16;
    if rest.len() == 2 {
        val |= u32::from(rest[1]) << 8;
    }
    out.push(ENCODE_STD[(val >> 18 & 0x3f) as usize]);
    out.push(ENCODE_STD[(val >> 12 & 0x3f) as usize]);
    if rest.len() == 2 {
        out.push(ENCODE_STD[(val >> 6 & 0x3f) as usize]);
    } else {
        out.push(PAD_CHAR);
    }
    out.push(PAD_CHAR);
}

fn decode_map(byte: u8) -> u8 {
    match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => INVALID,
    }
}

/// Decodes with the acceptance rules of Go's `base64.StdEncoding.DecodeString`,
/// returning `None` where Go reports a `CorruptInputError` (the pinned caller
/// then discards the output). Carriage returns and line feeds are ignored;
/// the rest must be whole four-character groups where only the final group
/// may end in `x=` or `==` padding; the encoding is not strict, so unused
/// bits in a padded group are dropped rather than rejected.
pub(crate) fn std_decode_string(src: &[u8]) -> Option<Vec<u8>> {
    let symbols: Vec<u8> = src
        .iter()
        .copied()
        .filter(|&byte| byte != b'\r' && byte != b'\n')
        .collect();
    if !symbols.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(symbols.len() / 4 * 3);
    let groups = symbols.len() / 4;
    for (index, group) in symbols.chunks_exact(4).enumerate() {
        let padding = match group {
            [_, _, PAD_CHAR, PAD_CHAR] => 2,
            [_, _, _, PAD_CHAR] => 1,
            _ => 0,
        };
        if padding > 0 && index + 1 != groups {
            return None;
        }
        let mut val = 0_u32;
        for &symbol in &group[..4 - padding] {
            let digit = decode_map(symbol);
            if digit == INVALID {
                return None;
            }
            val = (val << 6) | u32::from(digit);
        }
        val <<= 6 * padding;
        let bytes = [(val >> 16) as u8, (val >> 8) as u8, val as u8];
        out.extend_from_slice(&bytes[..3 - padding]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expectations derived by reading Go's encoding/base64 (StdEncoding).
    #[test]
    fn encodes_with_padding() {
        for (input, expected) in [
            (&b""[..], &b""[..]),
            (b"f", b"Zg=="),
            (b"fo", b"Zm8="),
            (b"foo", b"Zm9v"),
            (b"foob", b"Zm9vYg=="),
            (b"\xff\xfe\xfd", b"//79"),
        ] {
            let mut out = Vec::new();
            std_encode(input, &mut out);
            assert_eq!(out, expected);
        }
    }

    #[test]
    fn decodes_like_std_encoding() {
        assert_eq!(
            std_decode_string(b"Zm9vYg==").as_deref(),
            Some(&b"foob"[..])
        );
        assert_eq!(std_decode_string(b"Zm8=").as_deref(), Some(&b"fo"[..]));
        assert_eq!(std_decode_string(b"Zm9v").as_deref(), Some(&b"foo"[..]));
        assert_eq!(std_decode_string(b"").as_deref(), Some(&b""[..]));
        // Not strict: nonzero trailing bits are accepted.
        assert_eq!(std_decode_string(b"Zh==").as_deref(), Some(&b"f"[..]));
        // Missing padding, bad padding and trailing data are corrupt.
        assert_eq!(std_decode_string(b"Zg"), None);
        assert_eq!(std_decode_string(b"Zg="), None);
        assert_eq!(std_decode_string(b"Z==="), None);
        assert_eq!(std_decode_string(b"Zg==Zg=="), None);
        assert_eq!(std_decode_string(b"Z!=="), None);
        // Newlines are skipped.
        assert_eq!(std_decode_string(b"Zm\n9v").as_deref(), Some(&b"foo"[..]));
    }
}
