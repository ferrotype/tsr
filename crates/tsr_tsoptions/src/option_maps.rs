use crate::{ConfigValue as V, OptionDeclaration, OptionKind, COMPILER_OPTIONS};
use std::{collections::BTreeMap, sync::LazyLock};
use tsr_core::collections::OrderedMap;
use tsr_jsstring::{helpers::to_lower_go, JsString};

/// Original spellings and lowercase aliases are separate keys. Collisions keep
/// the last declaration, including an exact spelling shadowed by a later alias.
pub struct CommandLineOptionNameMap<'a>(BTreeMap<Vec<u8>, &'a OptionDeclaration>);
impl<'a> CommandLineOptionNameMap<'a> {
    /// port: tsc/internal/tsoptions/tsconfigparsing.go:commandLineOptionsToMap
    pub fn new(declarations: &'a [OptionDeclaration]) -> Self {
        let mut map = BTreeMap::new();
        for declaration in declarations {
            map.insert(declaration.name.as_bytes().to_vec(), declaration);
            map.insert(to_lower_go(declaration.name.as_bytes()), declaration);
        }
        Self(map)
    }
    /// port: tsc/internal/tsoptions/tsconfigparsing.go:CommandLineOptionNameMap.Get
    pub fn get(&self, name: &[u8]) -> Option<&'a OptionDeclaration> {
        self.0
            .get(name)
            .or_else(|| self.0.get(&to_lower_go(name)))
            .copied()
    }
    /// port: tsc/internal/tsoptions/tsconfigparsing.go:CommandLineOptionNameMap.GetSpellingSuggestion
    pub fn spelling_suggestion(&self, name: &[u8]) -> Option<&'a OptionDeclaration> {
        tsr_scanner::get_spelling_suggestion(
            name,
            self.0.values().copied(),
            |option| option.name.as_bytes(),
            |a, b| a.name.cmp(b.name),
            0,
        )
    }
    pub fn keys(&self) -> impl Iterator<Item = &[u8]> {
        self.0.keys().map(Vec::as_slice)
    }
}
pub fn compiler_option_name_map() -> &'static CommandLineOptionNameMap<'static> {
    static MAP: LazyLock<CommandLineOptionNameMap<'static>> =
        LazyLock::new(|| CommandLineOptionNameMap::new(COMPILER_OPTIONS));
    &MAP
}
/// port: tsc/internal/tsoptions/parsinghelpers.go:ConvertOptionToAbsolutePath
pub fn convert_option_to_absolute_path(
    key: &[u8],
    value: &V,
    map: &CommandLineOptionNameMap<'_>,
    cwd: &[u8],
) -> Option<V> {
    let option = map.get(key)?;
    let absolute = |value: &JsString| {
        V::String(JsString::from_bytes(tsr_tspath::absolute(
            value.as_bytes(),
            cwd,
        )))
    };
    if option.kind == OptionKind::List {
        if option.element.is_some_and(|element| element.is_file_path) {
            if let V::Array(values) = value {
                return Some(V::Array(values.as_ref().map(|values| {
                    values
                        .iter()
                        .map(|value| value.as_string().map_or_else(|| value.clone(), absolute))
                        .collect()
                })));
            }
        }
    } else if option.is_file_path {
        if let V::String(value) = value {
            return Some(absolute(value));
        }
    }
    None
}
/// port: tsc/internal/tsoptions/parsinghelpers.go:convertToOptionsWithAbsolutePaths
pub fn convert_options_with_absolute_paths(
    options: &mut OrderedMap<JsString, V>,
    map: &CommandLineOptionNameMap<'_>,
    cwd: &[u8],
) {
    options.for_each_value_mut(|name, value| {
        if let Some(converted) = convert_option_to_absolute_path(name.as_bytes(), value, map, cwd) {
            *value = converted;
        }
    });
}
