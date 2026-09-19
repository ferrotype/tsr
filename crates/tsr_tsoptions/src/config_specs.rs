//! Validated configuration patterns retain the user's pre-substitution spelling.
use crate::{
    glob::{SpecMatcher, Usage},
    ConfigValue, ParsedCommandLine,
};
use ts_jsstring::JsString;

#[derive(Clone, Debug, Default)]
pub struct ConfigFileSpecs {
    pub files_specs: ConfigValue,
    pub include_specs: ConfigValue,
    pub exclude_specs: ConfigValue,
    pub validated_files: Vec<JsString>,
    pub validated_includes: Vec<JsString>,
    pub validated_excludes: Vec<JsString>,
    pub files_before_substitution: Vec<JsString>,
    pub includes_before_substitution: Vec<JsString>,
    pub is_default_include: bool,
}
impl ConfigFileSpecs {
    /// port: tsc/internal/tsoptions/tsconfigparsing.go:configFileSpecs.matchesExclude
    pub fn matches_exclude(&self, file: &[u8], cwd: &[u8], case_sensitive: bool) -> bool {
        let Some(matcher) = SpecMatcher::new(
            &self.validated_excludes,
            cwd,
            Usage::Exclude,
            case_sensitive,
        ) else {
            return false;
        };
        if matcher.matches(file) {
            return true;
        }
        if !ts_tspath::has_extension(file) {
            let mut trailing = file.to_vec();
            if !trailing.ends_with(b"/") {
                trailing.push(b'/');
            }
            return matcher.matches(&trailing);
        }
        false
    }
    /// port: tsc/internal/tsoptions/tsconfigparsing.go:configFileSpecs.getMatchedFileSpec
    pub fn matched_file_spec(&self, file: &[u8], cwd: &[u8], case_sensitive: bool) -> &[u8] {
        let file = ts_tspath::to_path(file, cwd, case_sensitive);
        for (index, spec) in self.validated_files.iter().enumerate() {
            if ts_tspath::to_path(spec.as_bytes(), cwd, case_sensitive) == file {
                return self.files_before_substitution[index].as_bytes();
            }
        }
        b""
    }
    /// port: tsc/internal/tsoptions/tsconfigparsing.go:configFileSpecs.getMatchedIncludeSpec
    pub fn matched_include_spec(&self, file: &[u8], cwd: &[u8], case_sensitive: bool) -> &[u8] {
        for (index, spec) in self.validated_includes.iter().enumerate() {
            if SpecMatcher::new(
                std::slice::from_ref(spec),
                cwd,
                Usage::Files,
                case_sensitive,
            )
            .is_some_and(|m| m.matches(file))
            {
                return self.includes_before_substitution[index].as_bytes();
            }
        }
        b""
    }
}
impl ParsedCommandLine {
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.ConfigName
    pub fn config_name(&self) -> JsString {
        self.config_file
            .as_ref()
            .map_or_else(JsString::default, |config| {
                JsString::from_bytes(
                    config
                        .file
                        .view()
                        .source_file(config.root)
                        .expect("config source")
                        .file_name(),
                )
            })
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.GetMatchedFileSpec
    pub fn matched_file_spec(&self, file: &[u8]) -> &[u8] {
        self.config_specs
            .as_ref()
            .expect("file spec matching requires a parsed config")
            .matched_file_spec(
                file,
                self.config_base_path.as_bytes(),
                self.config_case_sensitive,
            )
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.GetMatchedIncludeSpec
    pub fn matched_include_spec(&self, file: &[u8]) -> (&[u8], bool) {
        let specs = self
            .config_specs
            .as_ref()
            .expect("include spec matching requires a parsed config");
        if specs.validated_includes.is_empty() {
            return (b"", false);
        }
        if specs.is_default_include {
            return (specs.validated_includes[0].as_bytes(), true);
        }
        (
            specs.matched_include_spec(
                file,
                self.config_base_path.as_bytes(),
                self.config_case_sensitive,
            ),
            false,
        )
    }
}
