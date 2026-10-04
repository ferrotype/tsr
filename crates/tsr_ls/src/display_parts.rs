//! The printer's classified output, shared by hover and signature help. Symbols
//! are resolved after printing, so the builder keeps its exclusive checker borrow.
use crate::Result;
use tsr_ast::{symbol_flags as sf, SymbolId, SyntaxKind as K};
use tsr_checker::Operation;
use tsr_lsproto::VSClassifiedTextRun;
use tsr_printer::EmitTextWriter;

#[derive(Clone, Default)]
pub(crate) struct DisplayParts {
    text: Vec<u8>,
    parts: Vec<(&'static str, Vec<u8>, Option<SymbolId>)>,
    classified: bool,
    last: usize,
}
impl DisplayParts {
    pub fn new(classified: bool) -> Self {
        Self {
            classified,
            ..Self::default()
        }
    }
    // port: tsc/internal/ls/displaypartswriter.go:displayPartsWriter.addRun
    pub(crate) fn add(
        &mut self,
        classification: &'static str,
        text: &[u8],
        symbol: Option<SymbolId>,
    ) {
        if text.is_empty() {
            return;
        }
        if self.classified {
            self.parts.push((classification, text.to_vec(), symbol));
        }
        self.last = self.text.len();
        self.text.extend_from_slice(text);
    }
    // port: tsc/internal/ls/displaypartswriter.go:displayPartsWriter.WriteFrom
    pub fn append(&mut self, other: Self) {
        if !other.text.is_empty() {
            self.last = self.text.len() + other.last;
        }
        self.text.extend_from_slice(&other.text);
        if self.classified {
            self.parts.extend(other.parts);
        }
    }
    pub fn runs(&self, checker: &Operation<'_>) -> Result<Vec<VSClassifiedTextRun>> {
        self.parts
            .iter()
            .map(|(class, text, symbol)| {
                let class = if let Some(id) = symbol {
                    classification(checker, *id)?
                } else {
                    class
                };
                Ok(VSClassifiedTextRun {
                    classification_type_name: class.to_string(),
                    text: String::from_utf8_lossy(text).into_owned(),
                    ..VSClassifiedTextRun::default()
                })
            })
            .collect()
    }
    pub fn string(&self) -> String {
        String::from_utf8_lossy(&self.text).into_owned()
    }
}
// port: tsc/internal/ls/displaypartswriter.go:classificationForSymbol
fn classification(checker: &Operation<'_>, id: SymbolId) -> Result<&'static str> {
    let symbol = checker.symbol_ref(id)?;
    let flags = checker.symbol(symbol)?.flags();
    if flags & sf::VARIABLE != 0 {
        let first = checker.symbol_declarations(symbol)?.iter().flatten().next();
        return Ok(if let Some(first) = first {
            if checker.node(first)?.kind() == K::Parameter {
                "parameter name"
            } else {
                "local name"
            }
        } else {
            "local name"
        });
    }
    for (mask, class) in [
        (
            sf::PROPERTY | sf::GET_ACCESSOR | sf::SET_ACCESSOR,
            "property name",
        ),
        (sf::ENUM_MEMBER, "field name"),
        (sf::FUNCTION, "method name"),
        (sf::CLASS, "class name"),
        (sf::INTERFACE, "interface name"),
        (sf::ENUM, "enum name"),
        (sf::MODULE, "module name"),
        (sf::METHOD, "method name"),
        (sf::TYPE_PARAMETER, "type parameter name"),
        (sf::TYPE_ALIAS | sf::ALIAS, "identifier"),
    ] {
        if flags & mask != 0 {
            return Ok(class);
        }
    }
    Ok("text")
}
impl EmitTextWriter for DisplayParts {
    fn write(&mut self, s: &[u8]) {
        self.add("text", s, None);
    }
    fn raw_write(&mut self, s: &[u8]) {
        self.write(s);
    }
    fn write_trailing_semicolon(&mut self, s: &[u8]) {
        self.write_punctuation(s);
    }
    fn write_comment(&mut self, s: &[u8]) {
        self.write(s);
    }
    fn write_keyword(&mut self, s: &[u8]) {
        self.add("keyword", s, None);
    }
    fn write_operator(&mut self, s: &[u8]) {
        self.add("operator", s, None);
    }
    fn write_punctuation(&mut self, s: &[u8]) {
        self.add("punctuation", s, None);
    }
    fn write_space(&mut self, s: &[u8]) {
        self.add("whitespace", s, None);
    }
    fn write_string_literal(&mut self, s: &[u8]) {
        self.add("string", s, None);
    }
    fn write_literal(&mut self, s: &[u8]) {
        self.write_string_literal(s);
    }
    fn write_parameter(&mut self, s: &[u8]) {
        self.add("parameter name", s, None);
    }
    fn write_property(&mut self, s: &[u8]) {
        self.add("property name", s, None);
    }
    fn write_symbol(&mut self, s: &[u8], symbol: Option<SymbolId>) {
        self.add("text", s, symbol);
    }
    fn write_line(&mut self) {
        self.write_space(b" ");
    }
    fn write_line_force(&mut self, _: bool) {
        self.write_line();
    }
    fn increase_indent(&mut self) {}
    fn decrease_indent(&mut self) {}
    fn clear(&mut self) {
        self.text.clear();
        self.parts.clear();
        self.last = 0;
    }
    fn text(&self) -> &[u8] {
        &self.text
    }
    fn get_text_pos(&self) -> usize {
        self.text.len()
    }
    fn get_line(&self) -> isize {
        0
    }
    fn get_column(&self) -> isize {
        0
    }
    fn get_indent(&self) -> isize {
        0
    }
    fn is_at_start_of_line(&self) -> bool {
        false
    }
    fn has_trailing_comment(&self) -> bool {
        false
    }
    fn has_trailing_whitespace(&self) -> bool {
        // Go's strict DecodeLastRune treats an invalid byte and U+FFFD alike.
        let bytes = &self.text[self.last..];
        let mut start = bytes.len().saturating_sub(1);
        while start > bytes.len().saturating_sub(4) && bytes[start] & 0xc0 == 0x80 {
            start -= 1;
        }
        std::str::from_utf8(&bytes[start..])
            .ok()
            .and_then(|s| s.chars().last())
            .is_some_and(|ch| ch != '\u{fffd}' && tsr_scanner::is_white_space_like(ch as i32))
    }
}
