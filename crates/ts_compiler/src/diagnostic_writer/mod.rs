//! Byte-preserving diagnostic display from `tsc/internal/diagnosticwriter`.
//! This formats existing diagnostics; it never runs checker queries. Content-map
//! span translation remains an explicit boundary until the spanmap port exists.
use crate::{Error, Program};
use std::{collections::HashMap, sync::Arc};
use ts_ast::{Diagnostic, NodeId, SourceFileRead};
use ts_jsstring::{JsString, SourceText};
mod pretty;

type Result<T> = std::result::Result<T, Error>;
const RESET: &[u8] = b"\x1b[0m";
const GREY: &[u8] = b"\x1b[90m";
const CYAN: &[u8] = b"\x1b[96m";
const YELLOW: &[u8] = b"\x1b[93m";

#[derive(Clone, Debug)]
pub struct FormattingOptions {
    pub new_line: Vec<u8>,
    pub current_directory: Vec<u8>,
    pub case_sensitive: bool,
}
impl Default for FormattingOptions {
    fn default() -> Self {
        Self {
            new_line: b"\n".to_vec(),
            current_directory: Vec::new(),
            case_sensitive: false,
        }
    }
}

/// One retained source snapshot shared by this formatting request's diagnostics.
pub struct File {
    name: JsString,
    text: SourceText,
    supplemental: bool,
    stable_identity: bool,
    lines: Vec<i32>,
}
impl File {
    pub fn name(&self) -> &[u8] {
        self.name.as_bytes()
    }
    pub fn text(&self) -> &[u8] {
        self.text.as_bytes()
    }
    pub fn is_supplemental(&self) -> bool {
        self.supplemental
    }

    pub fn line_and_character(&self, pos: i64) -> Result<(usize, usize)> {
        let pos = usize::try_from(pos)
            .map_err(|_| Error::Unsupported("diagnostic negative source position"))?;
        if pos > self.text.as_bytes().len() {
            return Err(Error::Unsupported("diagnostic position outside source"));
        }
        let line = self
            .lines
            .partition_point(|&start| start as usize <= pos)
            .saturating_sub(1);
        Ok((
            line,
            ts_jsstring::line_map::utf16_len(&self.text.as_bytes()[self.lines[line] as usize..pos])
                as usize,
        ))
    }
}

pub struct DiagnosticWriter<'a> {
    program: &'a Program,
    pub options: FormattingOptions,
    files: HashMap<(NodeId, bool), Arc<File>>,
}
impl<'a> DiagnosticWriter<'a> {
    pub fn new(program: &'a Program, options: FormattingOptions) -> Self {
        Self {
            program,
            options,
            files: HashMap::new(),
        }
    }
    pub fn source(&self, id: NodeId) -> Result<SourceFileRead<'_>> {
        if let Some(config) = self
            .program
            .config()
            .config_file
            .iter()
            .chain(&self.program.config().config_dependencies)
            .find(|c| c.root == id)
        {
            return Ok(config.file.view().source_file(id)?);
        }
        let index = self
            .program
            .owners
            .node_file_index(id)
            .ok_or(ts_arena::Error::WrongOwner)?;
        Ok(self.program.files()[index]
            .bound()
            .view()
            .ast()
            .source_file(id)?)
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:ASTDiagnostic.File
    /// Original text for external diagnostics, canonical file names, and explicit
    /// rejection of compiler diagnostics that still require span translation.
    pub fn file(&mut self, diagnostic: &Diagnostic) -> Result<Option<Arc<File>>> {
        let Some(id) = diagnostic.file else {
            return Ok(None);
        };
        let original = !diagnostic.source.is_empty();
        if let Some(file) = self.files.get(&(id, original)) {
            return Ok(Some(file.clone()));
        }
        let source = self.source(id)?;
        if !original && source.span_map().is_some() {
            return Err(Error::Unsupported(
                "ASTDiagnostic.resolve content-map span translation",
            ));
        }
        let name = if let Some(canonical) = source.canonical_source_file() {
            self.source(canonical)?.parse_options().file_name.clone()
        } else {
            source.parse_options().file_name.clone()
        };
        let text = if original {
            SourceText::from_bytes(source.original_text())
        } else {
            source.text().clone()
        };
        let file = Arc::new(File {
            name,
            lines: ts_jsstring::line_map::compute_ecma_line_starts(text.as_bytes()),
            text,
            supplemental: source.is_content_mapper_supplemental(),
            stable_identity: !original && source.canonical_source_file().is_none(),
        });
        self.files.insert((id, original), file.clone());
        Ok(Some(file))
    }
    /// Native sorting uses raw AST identities/locations, before display mapping.
    pub fn sorted<'d>(&self, diagnostics: &'d [Diagnostic]) -> Result<Vec<&'d Diagnostic>> {
        let mut names = HashMap::new();
        let mut pending: Vec<_> = diagnostics.iter().collect();
        while let Some(diagnostic) = pending.pop() {
            if let Some(id) = diagnostic.file {
                names.insert(id, self.source(id)?.parse_options().file_name.clone());
            }
            pending.extend(
                diagnostic
                    .related_information
                    .iter()
                    .chain(diagnostic.message_chain.iter())
                    .map(AsRef::as_ref),
            );
        }
        let name = |id| {
            names
                .get(&id)
                .map(JsString::as_bytes)
                .ok_or(ts_arena::Error::WrongOwner)
        };
        let mut sorted: Vec<_> = diagnostics.iter().collect();
        ts_core::sort_like_go(&mut sorted, &mut |a, b| {
            ts_ast::compare_diagnostics(a, b, &name).expect("validated diagnostic files")
        });
        Ok(sorted)
    }
    fn relative_name(&self, name: &[u8]) -> Vec<u8> {
        if ts_tspath::encoded_root_length(name) > 0 {
            ts_tspath::relative_to_directory_or_url(
                &self.options.current_directory,
                name,
                false,
                &self.options.current_directory,
                self.options.case_sensitive,
            )
        } else {
            name.to_vec()
        }
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:WriteLocation
    pub fn location(&self, file: &File, pos: i64, pretty: bool) -> Result<Vec<u8>> {
        let (line, ch) = file.line_and_character(pos)?;
        let mut out = Vec::new();
        styled(
            &mut out,
            &self.relative_name(file.name.as_bytes()),
            CYAN,
            pretty,
        );
        out.push(b':');
        styled(&mut out, (line + 1).to_string().as_bytes(), YELLOW, pretty);
        out.push(b':');
        styled(&mut out, (ch + 1).to_string().as_bytes(), YELLOW, pretty);
        Ok(out)
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:WriteFormatDiagnostics
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:FormatDiagnosticsWithColorAndContext
    pub fn format(&mut self, diagnostics: &[&Diagnostic], pretty: bool) -> Result<Vec<u8>> {
        let mut pending = diagnostics.to_vec();
        if pretty {
            for diagnostic in diagnostics {
                // Native pretty output only flattens a related message with a
                // location; it does not recursively print related information.
                pending.extend(
                    diagnostic
                        .related_information
                        .iter()
                        .filter(|d| d.file.is_some())
                        .map(AsRef::as_ref),
                );
            }
        }
        while let Some(diagnostic) = pending.pop() {
            self.file(diagnostic)?;
            pending.extend(diagnostic.message_chain.iter().map(AsRef::as_ref));
        }
        let mut out = Vec::new();
        for (index, d) in diagnostics.iter().enumerate() {
            if pretty {
                if index != 0 {
                    out.extend_from_slice(&self.options.new_line);
                }
                self.pretty(&mut out, d)?;
            } else {
                self.plain(&mut out, d)?;
            }
        }
        Ok(out)
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:WriteFormatDiagnostic
    fn plain(&mut self, out: &mut Vec<u8>, d: &Diagnostic) -> Result<()> {
        if let Some(file) = self.file(d)? {
            let (line, ch) = file.line_and_character(d.loc.pos())?;
            out.extend_from_slice(&self.relative_name(file.name.as_bytes()));
            out.extend_from_slice(format!("({},{}): ", line + 1, ch + 1).as_bytes());
        }
        out.extend_from_slice(category(d.category)?);
        out.push(b' ');
        out.extend_from_slice(prefix(d));
        out.extend_from_slice(format!("{}: ", d.code).as_bytes());
        out.extend_from_slice(&flattened(d, &self.options.new_line)?);
        out.extend_from_slice(&self.options.new_line);
        Ok(())
    }
}

pub fn category(value: i32) -> Result<&'static [u8]> {
    match value {
        0 => Ok(b"warning"),
        1 => Ok(b"error"),
        2 => Ok(b"suggestion"),
        3 => Ok(b"message"),
        _ => Err(Error::Unsupported("Unhandled diagnostic category")),
    }
}
fn color(value: i32) -> Result<&'static [u8]> {
    match value {
        0 => Ok(YELLOW),
        1 => Ok(b"\x1b[91m"),
        2 => Ok(GREY),
        3 => Ok(b"\x1b[94m"),
        _ => Err(Error::Unsupported("Unhandled diagnostic category")),
    }
}
fn prefix(d: &Diagnostic) -> &[u8] {
    if d.source.is_empty() {
        b"TS"
    } else {
        d.source.as_bytes()
    }
}
fn styled(out: &mut Vec<u8>, bytes: &[u8], style: &[u8], pretty: bool) {
    if pretty {
        out.extend_from_slice(style);
    }
    out.extend_from_slice(bytes);
    if pretty {
        out.extend_from_slice(RESET);
    }
}

/// Default-locale formatting uses Go ToValidUTF8 on substituted arguments.
/// Stored arguments and unformatted external messages remain byte-exact.
/// port: tsc/internal/diagnostics/diagnostics.go:Format
fn localized(d: &Diagnostic) -> Result<Vec<u8>> {
    if d.message.is_none() && !d.message_text.is_empty() {
        return Ok(d.message_text.as_bytes().to_vec());
    }
    let message = d
        .message
        .or_else(|| {
            std::str::from_utf8(d.message_key.as_bytes())
                .ok()
                .and_then(ts_diagnostics::by_key)
        })
        .ok_or(Error::Unsupported("unknown diagnostic localization key"))?;
    let bytes = message.text.as_bytes();
    if d.message_args.is_empty() {
        return Ok(bytes.to_vec());
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = bytes[i + 1..]
                .iter()
                .position(|&b| b == b'}')
                .map(|j| j + i + 1)
            {
                let digits = &bytes[i + 1..end];
                if !digits.is_empty() && digits.iter().all(u8::is_ascii_digit) {
                    let index = std::str::from_utf8(digits)
                        .unwrap()
                        .parse::<usize>()
                        .map_err(|_| Error::Unsupported("diagnostic placeholder overflow"))?;
                    let arg = d
                        .message_args
                        .get(index)
                        .ok_or(Error::Unsupported("diagnostic placeholder lacks argument"))?;
                    out.extend_from_slice(&valid_argument(arg.as_bytes()));
                    i = end + 1;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Ok(out)
}
/// Flatten already-resolved default-locale messages. This leaf operation does
/// not translate content-mapper aliases; file-aware formatting rejects that
/// unported path. Iteration keeps message nesting off the native call stack.
/// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:WriteFlattenedDiagnosticMessage
pub fn flattened(d: &Diagnostic, new_line: &[u8]) -> Result<Vec<u8>> {
    let mut out = localized(d)?;
    let mut stack: Vec<_> = d
        .message_chain
        .iter()
        .rev()
        .map(|c| (c.as_ref(), 1usize))
        .collect();
    while let Some((child, level)) = stack.pop() {
        out.extend_from_slice(new_line);
        out.extend(std::iter::repeat_n(b' ', level * 2));
        out.extend_from_slice(&localized(child)?);
        stack.extend(
            child
                .message_chain
                .iter()
                .rev()
                .map(|c| (c.as_ref(), level + 1)),
        );
    }
    Ok(out)
}

// strings.ToValidUTF8 replaces each consecutive run of invalid bytes once.
fn valid_argument(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offset = 0;
    let mut invalid = false;
    while offset < bytes.len() {
        let (rune, width) = ts_jsstring::wtf8::decode_utf8(&bytes[offset..]);
        if rune == 0xfffd && width == 1 {
            if !invalid {
                out.extend_from_slice(b"\xef\xbf\xbd");
            }
            invalid = true;
        } else {
            out.extend_from_slice(&bytes[offset..offset + width]);
            invalid = false;
        }
        offset += width;
    }
    out
}
