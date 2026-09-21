use crate::{format, locales_generated::TABLE_JSON, Message};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tsr_locale::Locale;

pub type TranslationTable = HashMap<String, String>;
static TABLES: [OnceLock<TranslationTable>; 13] = [const { OnceLock::new() }; 13];
static REQUESTS: OnceLock<Mutex<HashMap<Locale, Option<usize>>>> = OnceLock::new();

/// Source operation: tsc/internal/diagnostics/loc_generated.go:loadLocaleData
/// port: tsc/internal/diagnostics/loc_generated.go
fn load_table(index: usize) -> TranslationTable {
    let mut values = tsr_core::collections::OrderedMap::<String, String>::default();
    tsr_json::unmarshal(
        TABLE_JSON[index].as_bytes(),
        &mut values,
        tsr_json::Options::default(),
    )
    .expect("invalid generated localization table");
    values
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
/// The process cache is keyed by the requested tag, including untranslated
/// tags. Each selected table is decoded once. Undefined bypasses both caches.
/// port: tsc/internal/diagnostics/diagnostics.go:getLocalizedMessages
pub fn localized_messages(locale: &Locale) -> Option<&'static TranslationTable> {
    if locale.is_default() {
        return None;
    }
    let cache = REQUESTS.get_or_init(Mutex::default);
    let index = *cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(locale.clone())
        .or_insert_with(|| locale.diagnostic_match().filter(|&index| index > 0));
    index.map(|index| TABLES[index - 1].get_or_init(|| load_table(index - 1)))
}
/// port: tsc/internal/diagnostics/diagnostics.go:Localize
pub fn localize(locale: &Locale, message: Option<&Message>, key: &[u8], args: &[&[u8]]) -> Vec<u8> {
    let message = message
        .or_else(|| crate::by_key_bytes(key))
        .unwrap_or_else(|| {
            panic!(
                "Unknown diagnostic message: {}",
                String::from_utf8_lossy(key)
            )
        });
    let template = localized_messages(locale)
        .and_then(|table| table.get(message.key))
        .map_or(message.text.as_bytes(), |text| text.as_bytes());
    format(template, args)
}
impl Message {
    /// port: tsc/internal/diagnostics/diagnostics.go:Message.Localize
    pub fn localize(&self, locale: &Locale, args: &[crate::Argument]) -> Vec<u8> {
        let args = crate::stringify_args(args).unwrap_or_default();
        let refs: Vec<_> = args.iter().map(Vec::as_slice).collect();
        localize(locale, Some(self), b"", &refs)
    }
}
/// Runtime messages carry owned bytes without leaking them into the static
/// generated message roster. Their code, category and key are fixed by Go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdHocMessage {
    text: Vec<u8>,
}
impl AdHocMessage {
    /// port: tsc/internal/diagnostics/diagnostics.go:NewAdHocMessage
    pub fn new(text: impl Into<Vec<u8>>) -> Self {
        Self { text: text.into() }
    }
    pub fn code(&self) -> i32 {
        -1
    }
    pub fn category(&self) -> crate::Category {
        crate::Category::Error
    }
    pub fn key(&self) -> &'static str {
        "-1"
    }
    pub fn text(&self) -> &[u8] {
        &self.text
    }
    pub fn localize(&self, locale: &Locale, args: &[&[u8]]) -> Vec<u8> {
        let template = localized_messages(locale)
            .and_then(|table| table.get(self.key()))
            .map_or(self.text(), |value| value.as_bytes());
        format(template, args)
    }
}
