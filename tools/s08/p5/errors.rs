//! Native `.errors.txt` annotation, over production diagnostic formatting.
//! This does not produce diagnostics or infer file order from Program.files().
use crate::paths::remove_prefixes;
use serde_json::{json, Value};
use ts_ast::Diagnostic;
use ts_compiler::{
    diagnostic_writer::{category, flattened, DiagnosticWriter, FormattingOptions},
    Program,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const NL: &[u8] = b"\r\n";
pub struct InputFile<'a> {
    pub name: &'a [u8],
    pub content: &'a [u8],
}
fn library(name: &[u8]) -> bool {
    let name = ts_tspath::base_name(name);
    name.starts_with(b"lib.") && name.ends_with(b".d.ts")
}
fn config(name: &[u8]) -> bool {
    name.windows(8).any(|s| s == b"tsconfig") && name.windows(4).any(|s| s == b"json")
}
fn built(name: &[u8]) -> bool {
    name.starts_with(b"built/local/") || name.starts_with(b"/.ts/")
}
fn new_line(out: &mut Vec<u8>, first: &mut bool) {
    if !*first {
        out.extend_from_slice(NL);
    }
    *first = false;
}
fn rune_count(mut bytes: &[u8]) -> usize {
    let mut n = 0;
    while !bytes.is_empty() {
        let (_, w) = ts_jsstring::wtf8::decode_utf8(bytes);
        bytes = &bytes[w..];
        n += 1;
    }
    n
}
fn whitespace_prefix(out: &mut Vec<u8>, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let (r, w) = ts_jsstring::wtf8::decode_utf8(bytes);
        if matches!(r, 9 | 10 | 12 | 13 | 32) {
            out.extend_from_slice(&bytes[..w]);
        } else {
            out.push(b' ');
        }
        bytes = &bytes[w..];
    }
}
fn folded_literal_width(bytes: &[u8], literal: &[u8]) -> Option<usize> {
    let mut offset = 0;
    for &byte in literal {
        let (_, width) = ts_jsstring::wtf8::decode_utf8(&bytes[offset..]);
        if width == 0 || !ts_jsstring::equal_fold(&bytes[offset..offset + width], &[byte]) {
            return None;
        }
        offset += width;
    }
    Some(offset)
}
// The two pinned regexp substitutions differ: line-start lib(...), versus
// anywhere lib:... for related locations. Match greedily within a line.
fn library_locations(bytes: &[u8], related: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let allowed = related || i == 0 || bytes[i - 1] == b'\n';
        if allowed && folded_literal_width(&bytes[i..], b"lib").is_some() {
            let limit = bytes[i..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(bytes.len(), |j| i + j);
            let mut selected = None;
            for marker in i + 3..limit {
                let Some(width) = folded_literal_width(&bytes[marker..], b".d.ts") else {
                    continue;
                };
                let end = marker + width;
                let open = if related { b':' } else { b'(' };
                let separator = if related { b':' } else { b',' };
                if bytes.get(end) != Some(&open) {
                    continue;
                }
                let mut n = end + 1;
                let start = n;
                while bytes.get(n).is_some_and(u8::is_ascii_digit) {
                    n += 1;
                }
                if n == start || bytes.get(n) != Some(&separator) {
                    continue;
                }
                n += 1;
                let start = n;
                while bytes.get(n).is_some_and(u8::is_ascii_digit) {
                    n += 1;
                }
                if n == start || (!related && bytes.get(n) != Some(&b')')) {
                    continue;
                }
                selected = Some((end, n + usize::from(!related)));
            }
            if let Some((end, after)) = selected {
                out.extend_from_slice(&bytes[i..end]);
                out.extend_from_slice(if related { b":--:--" } else { b"(--,--)" });
                i = after;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}
struct Annotation<'a, 'p> {
    writer: &'a mut DiagnosticWriter<'p>,
    out: Vec<u8>,
    first: bool,
    non_library: usize,
}
impl Annotation<'_, '_> {
    fn error(&mut self, d: &Diagnostic) -> Result<()> {
        let message = remove_prefixes(&flattened(d, NL)?);
        for line in message.split(|&b| b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.is_empty() {
                continue;
            }
            new_line(&mut self.out, &mut self.first);
            self.out.extend_from_slice(b"!!! ");
            self.out.extend_from_slice(category(d.category)?);
            self.out
                .extend_from_slice(format!(" TS{}: ", d.code).as_bytes());
            self.out.extend_from_slice(line);
        }
        for info in &d.related_information {
            let mut location = Vec::new();
            if let Some(file) = self.writer.file(info)? {
                location.push(b' ');
                location.extend_from_slice(&self.writer.location(&file, info.loc.pos(), false)?);
                location = remove_prefixes(&location);
                if library(file.name()) {
                    location = library_locations(&location, true);
                }
            }
            new_line(&mut self.out, &mut self.first);
            self.out
                .extend_from_slice(format!("!!! related TS{}", info.code).as_bytes());
            self.out.extend_from_slice(&location);
            self.out.extend_from_slice(b": ");
            self.out.extend_from_slice(&flattened(info, NL)?);
        }
        if self
            .writer
            .file(d)?
            .is_none_or(|f| !library(f.name()) && !config(f.name()))
        {
            self.non_library += 1;
        }
        Ok(())
    }
}
/// Harness port: tsc/internal/testutil/tsbaseline/error_baseline.go:GetErrorBaseline.
pub fn render(
    program: &Program,
    inputs: &[InputFile<'_>],
    diagnostics: &[Diagnostic],
    pretty: bool,
) -> Result<Value> {
    if diagnostics.is_empty() {
        return Ok(json!({"state":"no_content"}));
    }
    let mut writer = DiagnosticWriter::new(
        program,
        FormattingOptions {
            new_line: NL.to_vec(),
            ..Default::default()
        },
    );
    let sorted = writer.sorted(diagnostics)?;
    let top = writer.format(&sorted, pretty)?;
    let mut output = library_locations(&remove_prefixes(&top), false);
    output.extend_from_slice(NL);
    output.extend_from_slice(NL);
    let mut state = Annotation {
        writer: &mut writer,
        out: Vec::new(),
        first: true,
        non_library: 0,
    };
    for &d in &sorted {
        if d.file.is_none() {
            state.error(d)?;
        }
    }
    output.append(&mut state.out);
    for input in inputs {
        let mut file_errors = Vec::new();
        for &d in &sorted {
            if let Some(file) = state.writer.file(d)? {
                if ts_tspath::compare_paths(
                    &remove_prefixes(file.name()),
                    &remove_prefixes(input.name),
                    b"",
                    false,
                )
                .is_eq()
                {
                    file_errors.push(d);
                }
            }
        }
        new_line(&mut state.out, &mut state.first);
        state.out.extend_from_slice(b"==== ");
        state.out.extend_from_slice(&remove_prefixes(input.name));
        state
            .out
            .extend_from_slice(format!(" ({} errors) ====", file_errors.len()).as_bytes());
        let starts = ts_jsstring::line_map::compute_ecma_line_starts(input.content);
        // Native lineDelimiter is CR?LF, unlike ComputeECMALineStarts. Keep that
        // distinction even for CR-only and Unicode line separators.
        let lines: Vec<_> = input
            .content
            .split(|&b| b == b'\n')
            .map(|s| s.strip_suffix(b"\r").unwrap_or(s))
            .collect();
        let mut marked = 0;
        for (index, &line) in lines.iter().enumerate() {
            let start = i64::from(
                *starts
                    .get(index)
                    .ok_or("native baseline line index outside ECMA map")?,
            );
            let last = index == lines.len() - 1;
            let next = if last {
                input.content.len() as i64
            } else {
                i64::from(
                    *starts
                        .get(index + 1)
                        .ok_or("native baseline next line missing")?,
                )
            };
            new_line(&mut state.out, &mut state.first);
            state.out.extend_from_slice(b"    ");
            state.out.extend_from_slice(line);
            for &d in &file_errors {
                let err_start = d.loc.pos();
                let end = d.loc.end();
                if end >= start && (err_start < next || last) {
                    let offset = err_start - start;
                    let length = (end - err_start) - 0.max(start - err_start);
                    let from = 0.max(offset) as usize;
                    let to =
                        (from as i64).max((from as i64 + length).min(line.len() as i64)) as usize;
                    let prefix = line
                        .get(..from)
                        .ok_or("native baseline squiggle starts beyond source line")?;
                    let span = line.get(from..to).ok_or("native baseline squiggle range")?;
                    new_line(&mut state.out, &mut state.first);
                    state.out.extend_from_slice(b"    ");
                    whitespace_prefix(&mut state.out, prefix);
                    state
                        .out
                        .extend(std::iter::repeat_n(b'~', rune_count(span)));
                    if last || next > end {
                        state.error(d)?;
                        marked += 1;
                    }
                }
            }
        }
        if marked != file_errors.len() {
            return Err("native baseline per-file diagnostic coverage assertion".into());
        }
        // dupeCase at the pin is never populated. Do not silently repair its
        // duplicate-input counting behaviour in the Rust comparison.
        output.append(&mut state.out);
    }
    let mut libraries = 0;
    let mut configs = 0;
    let mut supplemental = 0;
    for &d in &sorted {
        if let Some(file) = state.writer.file(d)? {
            libraries += usize::from(library(file.name()) || built(file.name()));
            configs += usize::from(config(file.name()));
            supplemental += usize::from(file.is_supplemental());
        }
    }
    if state.non_library + libraries + configs + supplemental != sorted.len() {
        return Err("native baseline total diagnostic coverage assertion".into());
    }
    if pretty {
        output.extend_from_slice(&remove_prefixes(&state.writer.error_summary(&sorted)?));
    }
    if sorted.iter().any(|d| d.code == -1) {
        return Err("native baseline critical assertion diagnostic -1".into());
    }
    let hex = hex(&output);
    Ok(json!({"state":"content","text_hex":hex}))
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|&b| {
            [
                char::from(DIGITS[usize::from(b >> 4)]),
                char::from(DIGITS[usize::from(b & 15)]),
            ]
        })
        .collect()
}
