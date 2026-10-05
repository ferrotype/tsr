use crate::{completions::CompletionOptions, symbol_display::ScriptElementKind as E};
use tsr_lsproto as lsp;

// port: tsc/internal/ls/completions.go:getCompletionsSymbolKind
pub(crate) fn kind(element: E) -> lsp::CompletionItemKind {
    use lsp::CompletionItemKind as C;
    match element {
        E::PrimitiveType | E::Keyword => C::KEYWORD,
        E::ConstElement
        | E::LetElement
        | E::VariableElement
        | E::LocalVariableElement
        | E::Alias
        | E::ParameterElement => C::VARIABLE,
        E::MemberVariableElement | E::MemberGetAccessorElement | E::MemberSetAccessorElement => {
            C::FIELD
        }
        E::FunctionElement | E::LocalFunctionElement => C::FUNCTION,
        E::MemberFunctionElement
        | E::ConstructSignatureElement
        | E::CallSignatureElement
        | E::IndexSignatureElement => C::METHOD,
        E::EnumElement => C::ENUM,
        E::EnumMemberElement => C::ENUM_MEMBER,
        E::ModuleElement | E::ExternalModuleName => C::MODULE,
        E::ClassElement | E::TypeElement => C::CLASS,
        E::InterfaceElement => C::INTERFACE,
        E::Warning => C::TEXT,
        E::ScriptElement => C::FILE,
        E::Directory => C::FOLDER,
        E::String => C::CONSTANT,
        _ => C::PROPERTY,
    }
}

// port: tsc/internal/ls/completions.go:LanguageService.setItemDefaults
pub(crate) fn defaults(
    list: &mut lsp::CompletionList,
    options: &CompletionOptions,
    position: &lsp::Position,
    replacement: Option<lsp::Range>,
    commit: &[&str],
) {
    if options.commit_characters {
        let characters: Vec<String> = commit.iter().map(|s| (*s).into()).collect();
        if options.default_commit_characters {
            list.item_defaults
                .get_or_insert_with(Default::default)
                .commit_characters = Some(Box::new(characters));
        } else {
            for item in list.items.iter_mut().flatten() {
                item.commit_characters
                    .get_or_insert_with(|| Box::new(characters.clone()));
            }
        }
    }
    if let Some(replace) = replacement {
        let insert = lsp::Range {
            start: replace.start.clone(),
            end: position.clone(),
        };
        if options.default_edit_range {
            list.item_defaults
                .get_or_insert_with(Default::default)
                .edit_range = Some(Box::new(lsp::RangeOrEditRangeWithInsertReplace {
                edit_range_with_insert_replace: Some(Box::new(lsp::EditRangeWithInsertReplace {
                    insert: insert.clone(),
                    replace: replace.clone(),
                })),
                ..Default::default()
            }));
        }
        if options.default_edit_range || options.insert_replace {
            for item in list.items.iter_mut().flatten() {
                if item.text_edit.is_some()
                    || options.default_edit_range && item.insert_text.is_none()
                {
                    continue;
                }
                let text = item.insert_text.as_deref().unwrap_or(&item.label).clone();
                item.text_edit = Some(Box::new(lsp::TextEditOrInsertReplaceEdit {
                    insert_replace_edit: Some(Box::new(lsp::InsertReplaceEdit {
                        new_text: text,
                        insert: insert.clone(),
                        replace: replace.clone(),
                    })),
                    ..Default::default()
                }));
                if options.default_edit_range {
                    item.insert_text = None;
                }
            }
        }
    }
}

// port: tsc/internal/ls/completions.go:getFilterText
pub(crate) fn filter_text(insert: &str, label: &str, word_start: Option<u8>, dot: &str) -> String {
    if let Some(name) = label.strip_prefix('#') {
        if insert.is_empty() {
            return if word_start == Some(b'#') {
                String::new()
            } else {
                name.into()
            };
        }
        if let Some(name) = insert.strip_prefix("this.#") {
            return if word_start == Some(b'#') {
                String::new()
            } else {
                name.into()
            };
        }
    }
    if insert.starts_with("this.") {
        return String::new();
    }
    if insert.starts_with('[') {
        return format!("{dot}{}", trim_element_access(insert));
    }
    if let Some(after) = insert.strip_prefix("?.") {
        return format!(
            "{dot}{}",
            if after.starts_with('[') {
                trim_element_access(after)
            } else {
                after
            }
        );
    }
    insert.into()
}
// port: tsc/internal/ls/completions.go:trimElementAccess
fn trim_element_access(text: &str) -> &str {
    let text = text.strip_prefix('[').unwrap_or(text);
    let text = text.strip_suffix(']').unwrap_or(text);
    for quote in ['\'', '"'] {
        if let Some(text) = text.strip_prefix(quote).and_then(|t| t.strip_suffix(quote)) {
            return text;
        }
    }
    text
}
