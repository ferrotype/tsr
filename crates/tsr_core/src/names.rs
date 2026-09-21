//! Source enum names, including open-value fallbacks. Diagnostic consumers use
//! these same renderers rather than maintaining partial private name tables.
use crate::{
    JsxEmit, LanguageVariant, ModuleKind, ModuleResolutionKind, NewLineKind, ScriptKind,
    ScriptTarget, Tristate,
};
use std::fmt;

impl fmt::Display for Tristate {
    /// Source operation: tsc/internal/core/tristate_stringer_generated.go:Tristate.String
    /// port: tsc/internal/core/tristate_stringer_generated.go
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            0 => f.write_str("TSUnknown"),
            1 => f.write_str("TSFalse"),
            2 => f.write_str("TSTrue"),
            n => write!(f, "Tristate({n})"),
        }
    }
}
impl fmt::Display for ScriptKind {
    /// Source operation: tsc/internal/core/scriptkind_stringer_generated.go:ScriptKind.String
    /// port: tsc/internal/core/scriptkind_stringer_generated.go
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::UNKNOWN => "ScriptKindUnknown",
            Self::JS => "ScriptKindJS",
            Self::JSX => "ScriptKindJSX",
            Self::TS => "ScriptKindTS",
            Self::TSX => "ScriptKindTSX",
            Self::JSON => "ScriptKindJSON",
            _ => return write!(f, "ScriptKind({})", self.0),
        };
        f.write_str(name)
    }
}
impl fmt::Display for LanguageVariant {
    /// Source operation: tsc/internal/core/languagevariant_stringer_generated.go:LanguageVariant.String
    /// port: tsc/internal/core/languagevariant_stringer_generated.go
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::STANDARD => f.write_str("LanguageVariantStandard"),
            Self::JSX => f.write_str("LanguageVariantJSX"),
            _ => write!(f, "LanguageVariant({})", self.0),
        }
    }
}
impl fmt::Display for ScriptTarget {
    /// Source operation: tsc/internal/core/scripttarget_stringer_generated.go:ScriptTarget.String
    /// port: tsc/internal/core/scripttarget_stringer_generated.go
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::NONE => "None",
            Self::ES5 => "ES5",
            Self::ES2015 => "ES2015",
            Self::ES2016 => "ES2016",
            Self::ES2017 => "ES2017",
            Self::ES2018 => "ES2018",
            Self::ES2019 => "ES2019",
            Self::ES2020 => "ES2020",
            Self::ES2021 => "ES2021",
            Self::ES2022 => "ES2022",
            Self::ES2023 => "ES2023",
            Self::ES2024 => "ES2024",
            Self::ES2025 => "ES2025",
            Self::ESNEXT => "ESNext",
            Self::JSON => "JSON",
            _ => return write!(f, "ScriptTarget({})", self.0),
        };
        f.write_str(name)
    }
}
impl fmt::Display for ModuleKind {
    /// Source operation: tsc/internal/core/modulekind_stringer_generated.go:ModuleKind.String
    /// port: tsc/internal/core/modulekind_stringer_generated.go
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::NONE => "None",
            Self::COMMON_JS => "CommonJS",
            Self::AMD => "AMD",
            Self::UMD => "UMD",
            Self::SYSTEM => "System",
            Self::ES2015 => "ES2015",
            Self::ES2020 => "ES2020",
            Self::ES2022 => "ES2022",
            Self::ESNEXT => "ESNext",
            Self::NODE16 => "Node16",
            Self::NODE18 => "Node18",
            Self::NODE20 => "Node20",
            Self::NODE_NEXT => "NodeNext",
            Self::PRESERVE => "Preserve",
            _ => return write!(f, "ModuleKind({})", self.0),
        };
        f.write_str(name)
    }
}
impl JsxEmit {
    /// Panics on zero or unknown values, as the pinned hand-written String does.
    /// port: tsc/internal/core/compileroptions.go:JsxEmit.String
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NONE => panic!("should not use zero value of JsxEmit"),
            Self::PRESERVE => "preserve",
            Self::REACT_NATIVE => "react-native",
            Self::REACT => "react",
            Self::REACT_JSX => "react-jsx",
            Self::REACT_JSX_DEV => "react-jsxdev",
            _ => panic!("unhandled case in JsxEmit.String"),
        }
    }
}
impl ModuleResolutionKind {
    /// port: tsc/internal/core/compileroptions.go:ModuleResolutionKind.String
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UNKNOWN => panic!("should not use zero value of ModuleResolutionKind"),
            Self::CLASSIC => "Classic",
            Self::NODE10 => "Node10",
            Self::NODE16 => "Node16",
            Self::NODE_NEXT => "NodeNext",
            Self::BUNDLER => "Bundler",
            _ => panic!("unhandled case in ModuleResolutionKind.String"),
        }
    }
}
impl NewLineKind {
    /// port: tsc/internal/core/compileroptions.go:GetNewLineKind
    pub fn from_text(text: &[u8]) -> Self {
        match text {
            b"\r\n" => Self::CRLF,
            b"\n" => Self::LF,
            _ => Self::NONE,
        }
    }
    /// port: tsc/internal/core/compileroptions.go:NewLineKind.GetNewLineCharacter
    pub fn as_str(self) -> &'static str {
        if self == Self::CRLF {
            "\r\n"
        } else {
            "\n"
        }
    }
}
