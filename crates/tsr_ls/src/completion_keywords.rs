//! Keyword inventories and ordering from the pinned completion service.
use tsr_ast::{utilities_middle as ast, SyntaxKind as K};
use tsr_lsproto as lsp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Filter {
    None,
    All,
    FunctionBody,
    Class,
    Interface,
    ConstructorParameter,
    TypeAssertion,
    Type,
    TypeKeyword,
}

// port: tsc/internal/ls/completions.go:CompareCompletionEntries
pub fn compare(a: &lsp::CompletionItem, b: &lsp::CompletionItem) -> std::cmp::Ordering {
    use tsr_jsstring::compare::compare_case_insensitive_then_sensitive as strings;
    strings(
        a.sort_text.as_deref().map_or("", String::as_str).as_bytes(),
        b.sort_text.as_deref().map_or("", String::as_str).as_bytes(),
    )
    .then_with(|| strings(a.label.as_bytes(), b.label.as_bytes()))
}

// port: tsc/internal/ls/completions.go:isTypeScriptOnlyKeyword
fn typescript_only(k: K) -> bool {
    matches!(
        k,
        K::AbstractKeyword
            | K::AnyKeyword
            | K::BigIntKeyword
            | K::BooleanKeyword
            | K::DeclareKeyword
            | K::EnumKeyword
            | K::GlobalKeyword
            | K::ImplementsKeyword
            | K::InferKeyword
            | K::InterfaceKeyword
            | K::IsKeyword
            | K::KeyOfKeyword
            | K::ModuleKeyword
            | K::NamespaceKeyword
            | K::NeverKeyword
            | K::NumberKeyword
            | K::ObjectKeyword
            | K::OverrideKeyword
            | K::PrivateKeyword
            | K::ProtectedKeyword
            | K::PublicKeyword
            | K::ReadonlyKeyword
            | K::StringKeyword
            | K::SymbolKeyword
            | K::TypeKeyword
            | K::UniqueKeyword
            | K::UnknownKeyword
    )
}
// port: tsc/internal/ls/completions.go:isClassMemberCompletionKeyword
pub(crate) fn class_keyword(k: K) -> bool {
    matches!(
        k,
        K::AbstractKeyword
            | K::AccessorKeyword
            | K::ConstructorKeyword
            | K::GetKeyword
            | K::SetKeyword
            | K::AsyncKeyword
            | K::DeclareKeyword
            | K::OverrideKeyword
            | K::PublicKeyword
            | K::PrivateKeyword
            | K::ProtectedKeyword
            | K::ReadonlyKeyword
            | K::StaticKeyword
    )
}
// port: tsc/internal/ls/completions.go:isFunctionLikeBodyKeyword
fn function_keyword(k: K) -> bool {
    matches!(
        k,
        K::AsyncKeyword
            | K::AwaitKeyword
            | K::UsingKeyword
            | K::AsKeyword
            | K::SatisfiesKeyword
            | K::TypeKeyword
    ) || !ast::is_contextual_keyword(k.into()) && !class_keyword(k)
}
pub(crate) fn type_keyword(k: K) -> bool {
    matches!(
        k,
        K::AnyKeyword
            | K::UnknownKeyword
            | K::NumberKeyword
            | K::BigIntKeyword
            | K::ObjectKeyword
            | K::BooleanKeyword
            | K::StringKeyword
            | K::SymbolKeyword
            | K::VoidKeyword
            | K::UndefinedKeyword
            | K::NullKeyword
            | K::NeverKeyword
            | K::TypeOfKeyword
            | K::KeyOfKeyword
            | K::ReadonlyKeyword
            | K::UniqueKeyword
            | K::InferKeyword
            | K::AssertsKeyword
            | K::TrueKeyword
            | K::FalseKeyword
    )
}
// port: tsc/internal/ls/completions.go:isContextualKeywordInAutoImportableExpressionSpace
pub(crate) fn contextual_expression(text: &str) -> bool {
    matches!(
        text,
        "abstract"
            | "async"
            | "await"
            | "declare"
            | "module"
            | "namespace"
            | "type"
            | "satisfies"
            | "as"
    )
}
// port: tsc/internal/ls/completions.go:getKeywordCompletions
pub(crate) fn keywords(filter: Filter, javascript: bool) -> Vec<lsp::CompletionItem> {
    (K::BreakKeyword as u16..=K::OfKeyword as u16)
        .filter_map(K::from_u16)
        .filter(|&k| !javascript || !typescript_only(k))
        .filter(|&k| match filter {
            Filter::None => false,
            Filter::All => {
                function_keyword(k)
                    || matches!(
                        k,
                        K::DeclareKeyword
                            | K::ModuleKeyword
                            | K::TypeKeyword
                            | K::NamespaceKeyword
                            | K::AbstractKeyword
                    )
                    || type_keyword(k) && k != K::UndefinedKeyword
            }
            Filter::FunctionBody => function_keyword(k),
            Filter::Class => class_keyword(k),
            Filter::Interface => k == K::ReadonlyKeyword,
            Filter::ConstructorParameter => ast::is_parameter_property_modifier(k.into()),
            Filter::TypeAssertion => type_keyword(k) || k == K::ConstKeyword,
            Filter::Type => type_keyword(k),
            Filter::TypeKeyword => k == K::TypeKeyword,
        })
        .map(|k| keyword(tsr_scanner::token_to_string(k)))
        .collect()
}
pub(crate) fn keyword(name: &str) -> lsp::CompletionItem {
    lsp::CompletionItem {
        label: name.into(),
        kind: Some(Box::new(lsp::CompletionItemKind::KEYWORD)),
        sort_text: Some(Box::new("15".into())),
        ..Default::default()
    }
}
