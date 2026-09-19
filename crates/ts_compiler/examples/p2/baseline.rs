//! Bounded plain error-baseline port. Unsupported branches stay named; they do
//! not replace diagnostic production or insert frozen expected output bytes.
use super::{file_name, hex, source, utf8, Error, Result};
use serde_json::{json, Value};
use ts_ast::Diagnostic;
use ts_compiler::Program;
const NL: &str = "\r\n";
fn category(d: &Diagnostic) -> Result<&'static str> {
    match d.category {
        0 => Ok("warning"),
        1 => Ok("error"),
        2 => Ok("suggestion"),
        3 => Ok("message"),
        _ => Err(Error::Unsupported("baseline unknown diagnostic category")),
    }
}
// port: tsc/internal/diagnostics/diagnostics.go:Format
// Default locale formatting.
fn message(d: &Diagnostic) -> Result<String> {
    if d.message.is_none() && !d.message_text.is_empty() {
        return Ok(utf8(d.message_text.as_bytes())?.into());
    }
    let message = d
        .message
        .or_else(|| {
            std::str::from_utf8(d.message_key.as_bytes())
                .ok()
                .and_then(ts_diagnostics::by_key)
        })
        .ok_or(Error::Unsupported("baseline unknown diagnostic key"))?;
    if d.message_args.is_empty() {
        return Ok(message.text.into());
    }
    let text = message.text.as_bytes();
    let mut output = String::new();
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'{' {
            if let Some(end) = text[i + 1..].iter().position(|&b| b == b'}') {
                let end = i + 1 + end;
                let digits = &text[i + 1..end];
                if !digits.is_empty() && digits.iter().all(u8::is_ascii_digit) {
                    let index = utf8(digits)?.parse::<isize>().map_err(|_| {
                        Error::Protocol("invalid diagnostic formatting placeholder".into())
                    })?;
                    let arg = d.message_args.get(index as usize).ok_or_else(|| {
                        Error::Protocol("invalid diagnostic formatting placeholder".into())
                    })?;
                    output.push_str(utf8(arg.as_bytes())?);
                    i = end + 1;
                    continue;
                }
            }
        }
        let ch = message.text[i..].chars().next().expect("nonempty suffix");
        output.push(ch);
        i += ch.len_utf8();
    }
    Ok(output)
}
// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:WriteFlattenedDiagnosticMessage
fn flattened(d: &Diagnostic, level: usize) -> Result<String> {
    let mut out = message(d)?;
    for child in &d.message_chain {
        out.push_str(NL);
        out.push_str(&"  ".repeat(level + 1));
        out.push_str(&flattened(child, level + 1)?);
    }
    Ok(out)
}
fn remove_prefixes(text: &str) -> String {
    let mut value = text.to_owned();
    for (from, to) in [
        ("/.ts/", ""),
        ("/.lib/", ""),
        ("/.src/", ""),
        ("bundled:///libs/", ""),
        ("file:///./ts/", "file:///"),
        ("file:///./lib/", "file:///"),
        ("file:///./src/", "file:///"),
    ] {
        value = value.replace(from, to);
    }
    value
}
fn lines(text: &str) -> Result<(Vec<&str>, Vec<usize>)> {
    if text.chars().any(|ch| matches!(ch, '\u{2028}' | '\u{2029}'))
        || text.bytes().enumerate().any(|(i, b)| {
            (b == b'\r' && text.as_bytes().get(i + 1) != Some(&b'\n'))
                || (b < 32 && !matches!(b, b'\n' | b'\r' | b'\t'))
        })
    {
        return Err(Error::Unsupported("baseline unsupported line delimiter"));
    }
    let mut starts = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    Ok((
        text.split('\n')
            .map(|s| s.strip_suffix('\r').unwrap_or(s))
            .collect(),
        starts,
    ))
}
fn append_line(output: &mut String, first: &mut bool, line: &str) {
    if !*first {
        output.push_str(NL);
    }
    *first = false;
    output.push_str(line);
}
fn error_text(
    program: &Program,
    output: &mut String,
    first: &mut bool,
    d: &Diagnostic,
) -> Result<()> {
    for line in remove_prefixes(&flattened(d, 0)?).split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if !line.is_empty() {
            append_line(
                output,
                first,
                &format!("!!! {} TS{}: {}", category(d)?, d.code, line),
            );
        }
    }
    for related in &d.related_information {
        let location = if let Some(id) = related.file {
            let source = source(program, id)?.view().source_file()?;
            let text = utf8(source.text().as_bytes())?;
            let (_, starts) = lines(text)?;
            let pos = usize::try_from(related.loc.pos())
                .map_err(|_| Error::Unsupported("baseline negative related position"))?;
            if pos > text.len() || !text.is_char_boundary(pos) {
                return Err(Error::Unsupported(
                    "baseline related position outside source boundary",
                ));
            }
            let line = starts.partition_point(|&p| p <= pos) - 1;
            let file = utf8(file_name(program, id)?)?;
            if file.starts_with("bundled:///libs/") {
                format!(" {}:--:--", remove_prefixes(file))
            } else {
                format!(
                    " {}:{}:{}",
                    remove_prefixes(file),
                    line + 1,
                    text[starts[line]..pos].encode_utf16().count() + 1
                )
            }
        } else {
            String::new()
        };
        append_line(
            output,
            first,
            &format!(
                "!!! related TS{}{}: {}",
                related.code,
                location,
                flattened(related, 0)?
            ),
        );
    }
    Ok(())
}
// Harness source (outside the production function ledger):
// tsc/internal/testutil/tsbaseline/error_baseline.go, iterateErrorBaseline.
// WriteFormatDiagnostic supplies the plain heading. Files here are ordinary
// sources; library/config/content-mapper and duplicate-case branches are explicit.
pub fn render(program: &Program, diagnostics: &[Diagnostic]) -> Result<Value> {
    if diagnostics.is_empty() {
        return Ok(json!({"state":"no_content"}));
    }
    let mut top = String::new();
    for d in diagnostics {
        if let Some(id) = d.file {
            let source = source(program, id)?.view().source_file()?;
            let text = utf8(source.text().as_bytes())?;
            let (_, starts) = lines(text)?;
            let pos = usize::try_from(d.loc.pos())
                .map_err(|_| Error::Unsupported("baseline negative source diagnostic position"))?;
            if pos > text.len() {
                return Err(Error::Protocol("diagnostic position outside source".into()));
            }
            let line = starts.partition_point(|&p| p <= pos) - 1;
            if !text.is_char_boundary(pos) {
                return Err(Error::Unsupported(
                    "baseline diagnostic bisects a UTF-8 code point",
                ));
            }
            top.push_str(&format!(
                "{}({},{}): ",
                utf8(file_name(program, id)?)?,
                line + 1,
                text[starts[line]..pos].encode_utf16().count() + 1
            ));
        }
        let prefix = if d.source.is_empty() {
            "TS"
        } else {
            utf8(d.source.as_bytes())?
        };
        top.push_str(&format!(
            "{} {}{}: {}{}",
            category(d)?,
            prefix,
            d.code,
            flattened(d, 0)?,
            NL
        ));
    }
    let mut result = remove_prefixes(&top);
    result.push_str(NL);
    result.push_str(NL);
    let mut decorated = String::new();
    let mut first = true;
    let mut reported = 0;
    for d in diagnostics.iter().filter(|d| d.file.is_none()) {
        error_text(program, &mut decorated, &mut first, d)?;
        reported += 1;
    }
    let mut names = std::collections::HashSet::new();
    for file in program.files() {
        let source = file.bound().view().source_file()?;
        let name = utf8(source.parse_options().file_name.as_bytes())?;
        // The native adapter passes every Program.SourceFile, including loaded
        // libraries, to GetErrorBaseline. Prefix removal happens when printed.
        let lower = name.to_ascii_lowercase();
        if !names.insert(lower.clone())
            || (lower.contains("tsconfig") && lower.contains("json"))
            || !source.content_mapper().is_empty()
        {
            return Err(Error::Unsupported(
                "baseline library/config/duplicate/content-mapper file",
            ));
        }
        let text = utf8(source.text().as_bytes())?;
        let (lines, starts) = lines(text)?;
        let file_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.file == Some(file.source()))
            .collect();
        append_line(
            &mut decorated,
            &mut first,
            &format!(
                "==== {} ({} errors) ====",
                remove_prefixes(name),
                file_errors.len()
            ),
        );
        let mut marked = 0;
        for (index, line) in lines.iter().enumerate() {
            let begin = starts[index];
            let next = starts.get(index + 1).copied().unwrap_or(text.len());
            append_line(&mut decorated, &mut first, &format!("    {line}"));
            for d in &file_errors {
                let start = usize::try_from(d.loc.pos())
                    .map_err(|_| Error::Unsupported("baseline negative source position"))?;
                let end = usize::try_from(d.loc.end())
                    .map_err(|_| Error::Unsupported("baseline negative source end"))?;
                if start > end || end > text.len() {
                    return Err(Error::Protocol("diagnostic source range invalid".into()));
                }
                if end >= begin && (start < next || index == lines.len() - 1) {
                    let squiggle_start = start.saturating_sub(begin);
                    if squiggle_start > line.len() {
                        return Err(Error::Unsupported("baseline squiggle begins outside line"));
                    }
                    let length = (end - start).saturating_sub(begin.saturating_sub(start));
                    let squiggle_end = (squiggle_start + length)
                        .min(line.len())
                        .max(squiggle_start);
                    if !line.is_char_boundary(squiggle_start)
                        || !line.is_char_boundary(squiggle_end)
                    {
                        return Err(Error::Unsupported(
                            "baseline diagnostic bisects a UTF-8 code point",
                        ));
                    }
                    let prefix: String = line[..squiggle_start]
                        .chars()
                        .map(|c| if c == '\t' || c == ' ' { c } else { ' ' })
                        .collect();
                    append_line(
                        &mut decorated,
                        &mut first,
                        &format!(
                            "    {}{}",
                            prefix,
                            "~".repeat(line[squiggle_start..squiggle_end].chars().count())
                        ),
                    );
                    if index == lines.len() - 1 || next > end {
                        error_text(program, &mut decorated, &mut first, d)?;
                        marked += 1;
                        reported += 1;
                    }
                }
            }
        }
        if marked != file_errors.len() {
            return Err(Error::Protocol(
                "baseline omitted a source diagnostic".into(),
            ));
        }
    }
    if reported != diagnostics.len() {
        return Err(Error::Protocol("baseline omitted a diagnostic".into()));
    }
    result.push_str(&decorated);
    Ok(json!({"state":"content","text_hex":hex(result.as_bytes())}))
}
