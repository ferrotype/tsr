//! Static source option declarations shared by CLI and config interpretation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionKind {
    String,
    Number,
    Boolean,
    Object,
    List,
    ListOrElement,
    Enum,
}
impl OptionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Object => "object",
            Self::List => "list",
            Self::ListOrElement => "listOrElement",
            Self::Enum => "enum",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnumValue {
    String(&'static str),
    Number(i32),
}
#[derive(Clone, Copy, Debug)]
pub struct OptionDeclaration {
    pub name: &'static str,
    pub short_name: &'static str,
    pub kind: OptionKind,
    pub is_file_path: bool,
    pub is_tsconfig_only: bool,
    pub is_command_line_only: bool,
    pub enum_values: &'static [(&'static str, EnumValue)],
    pub deprecated_keys: &'static [&'static str],
    pub element: Option<&'static Self>,
    pub extra_validation: &'static str,
    pub min_value: i64,
    pub allow_config_dir_template: bool,
    pub preserve_falsy: bool,
}
impl OptionDeclaration {
    pub fn disallow_null(&self) -> bool {
        self.name == "extends"
    }
    pub fn value_type_name(&self) -> String {
        match self.kind {
            OptionKind::List => "Array".into(),
            OptionKind::ListOrElement => format!(
                "{} or Array",
                self.element
                    .expect("list element declaration")
                    .value_type_name()
            ),
            kind => kind.as_str().into(),
        }
    }
    pub fn enum_names(&self) -> String {
        let names: Vec<_> = self
            .enum_values
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| !self.deprecated_keys.contains(name))
            .collect();
        format!("'{}'", names.join("', '"))
    }
}
#[path = "option_declarations_generated.rs"]
mod generated;
pub use generated::{
    BUILD_OPTIONS, COMPILER_OPTIONS, ROOT_OPTIONS, TYPE_ACQUISITION_OPTIONS, WATCH_OPTIONS,
};

/// port: tsc/internal/tsoptions/namemap.go:NameMap.GetOptionDeclarationFromName
pub fn option_declaration(name: &[u8], allow_short: bool) -> Option<&'static OptionDeclaration> {
    find_declaration(COMPILER_OPTIONS, name, allow_short)
}
pub fn find_declaration(
    options: &'static [OptionDeclaration],
    name: &[u8],
    allow_short: bool,
) -> Option<&'static OptionDeclaration> {
    let lower: std::borrow::Cow<'_, [u8]> = if name.is_ascii() {
        std::borrow::Cow::Borrowed(name)
    } else {
        std::borrow::Cow::Owned(ts_jsstring::helpers::to_lower_go(name))
    };
    if allow_short {
        if let Some(found) = options.iter().find(|option| {
            !option.short_name.is_empty()
                && option.short_name.as_bytes().eq_ignore_ascii_case(&lower)
        }) {
            return find_declaration(options, found.name.as_bytes(), false);
        }
    }
    options
        .iter()
        .rev()
        .find(|option| option.name.as_bytes().eq_ignore_ascii_case(&lower))
}
