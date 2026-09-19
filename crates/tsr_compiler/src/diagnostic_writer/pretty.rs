use super::{
    category, color, flattened, styled, Diagnostic, DiagnosticWriter, Error, File, Result, CYAN,
    GREY, RESET,
};
use std::sync::Arc;
const GUTTER: &[u8] = b"\x1b[7m";

fn repeat(out: &mut Vec<u8>, byte: u8, count: usize) {
    out.extend(std::iter::repeat_n(byte, count));
}
fn trim_line(bytes: &[u8]) -> Vec<u8> {
    let (mut offset, mut end) = (0, 0);
    while offset < bytes.len() {
        let (rune, width) = tsr_jsstring::wtf8::decode_utf8(&bytes[offset..]);
        offset += width;
        if !char::from_u32(rune as u32).is_some_and(char::is_whitespace) {
            end = offset;
        }
    }
    bytes[..end]
        .iter()
        .map(|&b| if b == b'\t' { b' ' } else { b })
        .collect()
}
impl DiagnosticWriter<'_> {
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:FormatDiagnosticWithColorAndContext
    pub(super) fn pretty(&mut self, out: &mut Vec<u8>, d: &Diagnostic) -> Result<()> {
        let file = self.file(d)?;
        if let Some(file) = &file {
            out.extend_from_slice(&self.location(file, d.loc.pos(), true)?);
            out.extend_from_slice(b" - ");
        }
        styled(out, category(d.category)?, color(d.category)?, true);
        out.extend_from_slice(GREY);
        out.push(b' ');
        out.extend_from_slice(super::prefix(d));
        out.extend_from_slice(format!("{}: ", d.code).as_bytes());
        out.extend_from_slice(RESET);
        out.extend_from_slice(&flattened(d, &self.options.new_line)?);
        if let Some(file) =
            file.filter(|_| d.code != tsr_diagnostics::File_appears_to_be_binary.code)
        {
            out.extend_from_slice(&self.options.new_line);
            self.snippet(
                out,
                &file,
                d.loc.pos(),
                d.loc.end(),
                color(d.category)?,
                b"",
            )?;
            out.extend_from_slice(&self.options.new_line);
        }
        for related in &d.related_information {
            if let Some(file) = self.file(related)? {
                out.extend_from_slice(&self.options.new_line);
                out.extend_from_slice(b"  ");
                out.extend_from_slice(&self.location(&file, related.loc.pos(), true)?);
                out.extend_from_slice(b" - ");
                out.extend_from_slice(&flattened(related, &self.options.new_line)?);
                self.snippet(
                    out,
                    &file,
                    related.loc.pos(),
                    related.loc.end(),
                    CYAN,
                    b"    ",
                )?;
            }
            out.extend_from_slice(&self.options.new_line);
        }
        Ok(())
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:writeCodeSnippet
    fn snippet(
        &self,
        out: &mut Vec<u8>,
        file: &File,
        start: i64,
        end: i64,
        squiggle_color: &[u8],
        indent: &[u8],
    ) -> Result<()> {
        let (first, first_char) = file.line_and_character(start)?;
        let (last, mut last_char) = file.line_and_character(end)?;
        if start == end {
            last_char += 1;
        }
        if end < start {
            return Err(Error::Unsupported("negative diagnostic span"));
        }
        let many = last - first >= 4;
        let width = (last + 1).to_string().len().max(if many { 3 } else { 0 });
        let mut line = first;
        while line <= last {
            out.extend_from_slice(&self.options.new_line);
            if many && first + 1 < line && line < last - 1 {
                out.extend_from_slice(indent);
                out.extend_from_slice(GUTTER);
                repeat(out, b' ', width - 3);
                out.extend_from_slice(b"...");
                out.extend_from_slice(RESET);
                out.push(b' ');
                out.extend_from_slice(&self.options.new_line);
                line = last - 1;
            }
            let byte_start = file.lines[line] as usize;
            let byte_end = file
                .lines
                .get(line + 1)
                .map_or(file.text.as_bytes().len(), |&n| n as usize);
            let content = trim_line(&file.text.as_bytes()[byte_start..byte_end]);
            out.extend_from_slice(indent);
            out.extend_from_slice(GUTTER);
            out.extend_from_slice(format!("{:>width$}", line + 1).as_bytes());
            out.extend_from_slice(RESET);
            out.push(b' ');
            out.extend_from_slice(&content);
            out.extend_from_slice(&self.options.new_line);
            out.extend_from_slice(indent);
            out.extend_from_slice(GUTTER);
            repeat(out, b' ', width);
            out.extend_from_slice(RESET);
            out.push(b' ');
            out.extend_from_slice(squiggle_color);
            let length = if line == first {
                repeat(out, b' ', first_char);
                let last_for_line = if line == last {
                    last_char
                } else {
                    tsr_jsstring::line_map::utf16_len(&content) as usize
                };
                last_for_line
                    .checked_sub(first_char)
                    .ok_or(Error::Unsupported(
                        "writeCodeSnippet negative squiggle length",
                    ))?
            } else if line == last {
                last_char
            } else {
                tsr_jsstring::line_map::utf16_len(&content) as usize
            };
            repeat(out, b'~', length);
            out.extend_from_slice(RESET);
            line += 1;
        }
        Ok(())
    }
    fn pretty_path(&self, file: &File, first: &Diagnostic) -> Result<Vec<u8>> {
        let (line, _) = file.line_and_character(first.loc.pos())?;
        let mut path = if tsr_tspath::root_length(file.name.as_bytes()) > 0
            && tsr_tspath::root_length(&self.options.current_directory) > 0
        {
            self.relative_name(file.name.as_bytes())
        } else {
            file.name.as_bytes().to_vec()
        };
        path.extend_from_slice(GREY);
        path.extend_from_slice(format!(":{}", line + 1).as_bytes());
        path.extend_from_slice(RESET);
        Ok(path)
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:WriteErrorSummaryText
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:getErrorSummary
    pub fn error_summary(&mut self, diagnostics: &[&Diagnostic]) -> Result<Vec<u8>> {
        let mut total = 0;
        let mut globals = 0;
        // ASTDiagnostic.File constructs a fresh FileLike for original/renamed
        // text. Native summary grouping uses those identities, even when two
        // wrappers have identical names. Preserve that observable distinction.
        let mut groups: Vec<(Arc<File>, Vec<&Diagnostic>)> = Vec::new();
        for &d in diagnostics {
            if d.category != 1 {
                continue;
            }
            total += 1;
            let Some(file) = self.file(d)? else {
                globals += 1;
                continue;
            };
            if let Some((_, values)) = groups
                .iter_mut()
                .find(|(f, _)| file.stable_identity && Arc::ptr_eq(f, &file))
            {
                values.push(d);
            } else {
                groups.push((file, vec![d]));
            }
        }
        let mut out = Vec::new();
        if total == 0 {
            return Ok(out);
        }
        groups.sort_by(|(a, _), (b, _)| a.name.as_bytes().cmp(b.name.as_bytes()));
        let first_path = if let Some((file, diags)) = groups.first() {
            self.pretty_path(file, diags[0])?
        } else {
            Vec::new()
        };
        out.extend_from_slice(&self.options.new_line);
        if total == 1 {
            if globals > 0 || first_path.is_empty() {
                out.extend_from_slice(b"Found 1 error.");
            } else {
                out.extend_from_slice(b"Found 1 error in ");
                out.extend_from_slice(&first_path);
            }
        } else {
            match groups.len() {
                0 => out.extend_from_slice(format!("Found {total} errors.").as_bytes()),
                1 => {
                    out.extend_from_slice(
                        format!("Found {total} errors in the same file, starting at: ").as_bytes(),
                    );
                    out.extend_from_slice(&first_path);
                }
                n => {
                    out.extend_from_slice(format!("Found {total} errors in {n} files.").as_bytes());
                }
            }
        }
        out.extend_from_slice(&self.options.new_line);
        out.extend_from_slice(&self.options.new_line);
        if groups.len() > 1 {
            let digits = groups
                .iter()
                .map(|(_, ds)| ds.len())
                .max()
                .unwrap()
                .to_string()
                .len();
            let width = 6.max(digits);
            repeat(&mut out, b' ', digits.saturating_sub(6));
            out.extend_from_slice(b"Errors  Files");
            out.extend_from_slice(&self.options.new_line);
            for (file, diags) in groups {
                out.extend_from_slice(format!("{:>width$}  ", diags.len()).as_bytes());
                out.extend_from_slice(&self.pretty_path(&file, diags[0])?);
                out.extend_from_slice(&self.options.new_line);
            }
            out.extend_from_slice(&self.options.new_line);
        }
        Ok(out)
    }
}
