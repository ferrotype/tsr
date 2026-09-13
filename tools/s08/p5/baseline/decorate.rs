//! Raw Corsa type/symbol baseline decoration. No legacy Strada fixups.
use super::{Result, Row};
pub const NL: &[u8] = b"\r\n";

fn code_lines(bytes: &[u8]) -> Vec<&[u8]> {
    // Go's regexp alternation matches CR before the optional-CR/LF branch.
    // Consequently CRLF produces two split points in this particular writer.
    let mut result = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        let width = if matches!(bytes[i], b'\r' | b'\n') {
            1
        } else if bytes[i..].starts_with(b"\xe2\x80\xa8") || bytes[i..].starts_with(b"\xe2\x80\xa9")
        {
            3
        } else {
            i += 1;
            continue;
        };
        result.push(&bytes[start..i]);
        i += width;
        start = i;
    }
    result.push(&bytes[start..]);
    result
}

fn blank_or_bracket(mut line: &[u8]) -> bool {
    // regexp \s is ASCII; strings.TrimSpace uses Go's Unicode White_Space set.
    let ascii_space = |b: &u8| matches!(b, b'\t' | b'\n' | 12 | b'\r' | b' ');
    while line.first().is_some_and(ascii_space) {
        line = &line[1..];
    }
    while line.last().is_some_and(ascii_space) {
        line = &line[..line.len() - 1];
    }
    if matches!(line, b"{" | b"|" | b"}") {
        return true;
    }
    const SPACE: &[&[u8]] = &[
        b"\t",
        b"\n",
        b"\x0b",
        b"\x0c",
        b"\r",
        b" ",
        b"\xc2\x85",
        b"\xc2\xa0",
        b"\xe1\x9a\x80",
        b"\xe2\x80\x80",
        b"\xe2\x80\x81",
        b"\xe2\x80\x82",
        b"\xe2\x80\x83",
        b"\xe2\x80\x84",
        b"\xe2\x80\x85",
        b"\xe2\x80\x86",
        b"\xe2\x80\x87",
        b"\xe2\x80\x88",
        b"\xe2\x80\x89",
        b"\xe2\x80\x8a",
        b"\xe2\x80\xa8",
        b"\xe2\x80\xa9",
        b"\xe2\x80\xaf",
        b"\xe2\x81\x9f",
        b"\xe3\x80\x80",
    ];
    while !line.is_empty() {
        let Some(prefix) = SPACE.iter().find(|space| line.starts_with(space)) else {
            return false;
        };
        line = &line[prefix.len()..];
    }
    true
}

fn joined(output: &mut Vec<u8>, lines: &[&[u8]]) {
    for (index, line) in lines.iter().enumerate() {
        if index != 0 {
            output.extend_from_slice(NL);
        }
        output.extend_from_slice(line);
    }
}

fn without_line_delimiters(output: &mut Vec<u8>, bytes: &[u8]) {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"\r\n") {
            i += 2;
        } else if bytes[i] == b'\n' {
            i += 1;
        } else {
            output.push(bytes[i]);
            i += 1;
        }
    }
}

pub use crate::paths::remove_prefixes;

pub fn file(name: &[u8], content: &[u8], rows: &[Row]) -> Result<Vec<u8>> {
    let lines = code_lines(content);
    let mut output = b"=== ".to_vec();
    output.extend_from_slice(name);
    output.extend_from_slice(b" ===\r\n");
    let mut last: Option<usize> = None;
    for row in rows {
        if last != Some(row.line) {
            let start = last.map_or(0, |i| i + 1);
            if last.is_some() && !lines.get(start).is_some_and(|l| blank_or_bracket(l)) {
                output.extend_from_slice(NL);
            }
            joined(
                &mut output,
                lines
                    .get(start..=row.line)
                    .ok_or("baseline line order/range")?,
            );
            output.extend_from_slice(NL);
        }
        last = Some(row.line);
        output.push(b'>');
        without_line_delimiters(&mut output, &row.source_text);
        output.extend_from_slice(b" : ");
        output.extend_from_slice(&row.display);
        output.extend_from_slice(NL);
    }
    let start = last.map_or(0, |i| i + 1);
    if start < lines.len() {
        if !blank_or_bracket(lines[start]) {
            output.extend_from_slice(NL);
        }
        joined(&mut output, &lines[start..]);
    }
    output.extend_from_slice(NL);
    Ok(remove_prefixes(&output))
}
