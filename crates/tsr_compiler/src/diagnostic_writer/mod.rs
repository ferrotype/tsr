//! Byte-preserving diagnostic display from `tsc/internal/diagnosticwriter`.
//! This formats existing diagnostics; it never runs checker queries. Content-map
//! spans resolve against the original text, preserving synthesized-code notes.
use crate::{Error, Program};
use std::{collections::HashMap, sync::Arc};
use tsr_ast::{Diagnostic, NodeId, SourceFileRead};
use tsr_jsstring::{JsString, SourceText};
mod pretty;
mod resolved;
pub use resolved::{
    to_diagnostics, try_clear_screen, wrap_diagnostics, AstDiagnostic, FileKind, ResolvedLocation,
};

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
    pub locale: tsr_locale::Locale,
}
impl Default for FormattingOptions {
    fn default() -> Self {
        Self {
            new_line: b"\n".to_vec(),
            current_directory: Vec::new(),
            case_sensitive: false,
            locale: tsr_locale::Locale::default(),
        }
    }
}

/// One retained source snapshot shared by this formatting request's diagnostics.
pub struct File {
    name: JsString,
    text: SourceText,
    supplemental: bool,
    stable_identity: bool,
    kind: FileKind,
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
            tsr_jsstring::line_map::utf16_len(&self.text.as_bytes()[self.lines[line] as usize..pos])
                as usize,
        ))
    }
}

pub struct DiagnosticWriter<'a> {
    sources: &'a dyn DiagnosticSources,
    pub options: FormattingOptions,
    files: HashMap<(NodeId, bool), Arc<File>>,
}
impl<'a> DiagnosticWriter<'a> {
    pub fn new(program: &'a Program, options: FormattingOptions) -> Self {
        Self {
            sources: program,
            options,
            files: HashMap::new(),
        }
    }
    pub fn source(&self, id: NodeId) -> Result<SourceFileRead<'_>> {
        self.sources.diagnostic_source(id)
    }
    /// Formats configuration diagnostics without constructing a program or
    /// loading/binding its input files. The provider retains every source owner.
    pub fn from_sources(sources: &'a dyn DiagnosticSources, options: FormattingOptions) -> Self {
        Self {
            sources,
            options,
            files: HashMap::new(),
        }
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:ASTDiagnostic.File
    /// Select original or virtual text after resolving the diagnostic span.
    pub fn file(&mut self, diagnostic: &Diagnostic) -> Result<Option<Arc<File>>> {
        let Some(id) = diagnostic.file else {
            return Ok(None);
        };
        let original = self.resolved_location(diagnostic)?.use_original;
        if let Some(file) = self.files.get(&(id, original)) {
            return Ok(Some(file.clone()));
        }
        let source = self.source(id)?;
        let name = if let Some(canonical) = source.canonical_source_file() {
            self.source(canonical)?.parse_options().file_name.clone()
        } else {
            source.parse_options().file_name.clone()
        };
        let file = Arc::new(if original {
            File::original(&source, name)
        } else if name.as_bytes() != source.file_name() {
            File::renamed(&source, name)
        } else {
            File::from_source(&source)
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
                .ok_or(tsr_arena::Error::WrongOwner)
        };
        let mut sorted: Vec<_> = diagnostics.iter().collect();
        tsr_core::sort_like_go(&mut sorted, &mut |a, b| {
            tsr_ast::compare_diagnostics(a, b, &name).expect("validated diagnostic files")
        });
        Ok(sorted)
    }
    fn relative_name(&self, name: &[u8]) -> Vec<u8> {
        if tsr_tspath::encoded_root_length(name) > 0 {
            tsr_tspath::relative_to_directory_or_url(
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
            let (line, ch) = file.line_and_character(self.resolved_location(d)?.loc.pos())?;
            out.extend_from_slice(&self.relative_name(file.name.as_bytes()));
            out.extend_from_slice(format!("({},{}): ", line + 1, ch + 1).as_bytes());
        }
        out.extend_from_slice(category(d.category)?);
        out.push(b' ');
        out.extend_from_slice(prefix(d));
        out.extend_from_slice(format!("{}: ", d.code).as_bytes());
        out.extend_from_slice(&self.flatten(d, &self.options.new_line)?);
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
pub fn color(value: i32) -> Result<&'static [u8]> {
    match value {
        0 => Ok(YELLOW),
        1 => Ok(b"\x1b[91m"),
        2 => Ok(GREY),
        3 => Ok(b"\x1b[94m"),
        _ => Err(Error::Unsupported("Unhandled diagnostic category")),
    }
}
pub fn prefix(d: &Diagnostic) -> &[u8] {
    if d.source.is_empty() {
        b"TS"
    } else {
        d.source.as_bytes()
    }
}
pub fn styled(out: &mut Vec<u8>, bytes: &[u8], style: &[u8], pretty: bool) {
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
/// Uses the shared diagnostics::Format port for the default locale.
pub fn localized(d: &Diagnostic) -> Result<Vec<u8>> {
    localized_with_locale(d, &tsr_locale::DEFAULT)
}

/// Localize an existing message using the same request locale as its writer.
/// External message text remains byte-exact and is never translated.
pub fn localized_with_locale(d: &Diagnostic, locale: &tsr_locale::Locale) -> Result<Vec<u8>> {
    if d.message.is_none() && !d.message_text.is_empty() {
        return Ok(d.message_text.as_bytes().to_vec());
    }
    let message = d
        .message
        .or_else(|| {
            std::str::from_utf8(d.message_key.as_bytes())
                .ok()
                .and_then(tsr_diagnostics::by_key)
        })
        .ok_or(Error::Unsupported("unknown diagnostic localization key"))?;
    let args: Vec<_> = d
        .message_args
        .iter()
        .map(tsr_jsstring::JsString::as_bytes)
        .collect();
    let template = tsr_diagnostics::localized_messages(locale)
        .and_then(|table| table.get(message.key))
        .map_or(message.text.as_bytes(), |text| text.as_bytes());
    tsr_diagnostics::try_format(template, &args).map_err(Error::Unsupported)
}
/// Flatten already-resolved default-locale messages. This leaf operation does
/// not resolve files; use DiagnosticWriter::flatten for mapped diagnostics.
/// Iteration keeps message nesting off the native call stack.
/// Raw-message helper; the complete file-aware port is DiagnosticWriter::flatten.
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

/// A retained source owner for diagnostic display. Resolving a foreign node
/// must fail; implementations never guess a file from a path or slot number.
pub trait DiagnosticSources {
    fn diagnostic_source(&self, id: NodeId) -> Result<SourceFileRead<'_>>;
}
impl DiagnosticSources for Program {
    fn diagnostic_source(&self, id: NodeId) -> Result<SourceFileRead<'_>> {
        if let Some(config) = self.config_source(id) {
            return Ok(config.file.view().source_file(id)?);
        }
        let index = self
            .owners
            .node_file_index(id)
            .ok_or(tsr_arena::Error::WrongOwner)?;
        Ok(self.files()[index].bound().view().ast().source_file(id)?)
    }
}
impl DiagnosticSources for tsr_tsoptions::ParsedCommandLine {
    fn diagnostic_source(&self, id: NodeId) -> Result<SourceFileRead<'_>> {
        let config = self
            .config_file
            .iter()
            .chain(&self.config_dependencies)
            .find(|config| config.root == id)
            .ok_or(tsr_arena::Error::WrongOwner)?;
        Ok(config.file.view().source_file(id)?)
    }
}
