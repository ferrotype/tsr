//! Native simultaneous test-path prefix replacement.
pub fn remove_prefixes(bytes: &[u8]) -> Vec<u8> {
    const REPLACE: &[(&[u8], &[u8])] = &[
        (b"/.ts/", b""),
        (b"/.lib/", b""),
        (b"/.src/", b""),
        (b"bundled:///libs/", b""),
        (b"file:///./ts/", b"file:///"),
        (b"file:///./lib/", b"file:///"),
        (b"file:///./src/", b"file:///"),
    ];
    let mut output = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some((from, to)) = REPLACE
            .iter()
            .find(|(from, _)| bytes[i..].starts_with(from))
        {
            output.extend_from_slice(to);
            i += from.len();
        } else {
            output.push(bytes[i]);
            i += 1;
        }
    }
    output
}
