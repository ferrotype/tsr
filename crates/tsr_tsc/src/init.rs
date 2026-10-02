//! The pinned `--init` config writer. Option insertion order and raw enum
//! identity are retained until the generated JSON is written.
use crate::help::get_header;
use crate::{write_all, System};
use tsr_ast::Diagnostic;
use tsr_core::collections::OrderedMap;
use tsr_core::{JsxEmit, ModuleDetectionKind, ModuleKind, ScriptTarget};
use tsr_diagnostics::{self as d, Message};
use tsr_jsstring::JsString;
use tsr_locale::Locale;
use tsr_tsoptions::{ConfigValue, EnumValue, COMPILER_OPTIONS};

// port: tsc/internal/execute/tsc/init.go:WriteConfigFile
pub fn write_config_file(
    sys: &dyn System,
    locale: &Locale,
    report_diagnostic: &dyn Fn(&Diagnostic),
    options: &OrderedMap<JsString, ConfigValue>,
) {
    let file = tsr_tspath::normalize(&tsr_tspath::combine(
        sys.get_current_directory(),
        &[b"tsconfig.json"],
    ))
    .into_owned();
    let fs = sys.fs();
    if fs.file_exists(&file).unwrap_or(false) {
        report_diagnostic(&Diagnostic::compiler(
            d::A_tsconfig_json_file_is_already_defined_at_Colon_0,
            vec![JsString::from_bytes(file)],
        ));
    } else {
        // The pin deliberately ignores WriteFile's error here.
        let _ = fs.write_file(&file, &generate_tsconfig(options, locale));
        let mut output = b"\n".to_vec();
        output.extend(get_header(sys, b"Created a new tsconfig.json"));
        output.extend(b"You can learn more at https://aka.ms/tsconfig\n");
        write_all(&*sys.writer(), &output);
    }
}

#[derive(Clone, Copy)]
enum Commented {
    Never,
    Optional,
}

struct ConfigWriter<'a> {
    options: &'a OrderedMap<JsString, ConfigValue>,
    locale: &'a Locale,
    remaining: Vec<JsString>,
    lines: Vec<Vec<u8>>,
}

impl ConfigWriter<'_> {
    fn header(&mut self, message: &Message) {
        self.lines
            .push([b"    // ".as_slice(), &message.localize(self.locale, &[])].concat());
    }

    fn newline(&mut self) {
        self.lines.push(Vec::new());
    }

    fn emit_option(&mut self, setting: &[u8], default_value: &ConfigValue, commented: Commented) {
        if let Some(index) = self
            .remaining
            .iter()
            .position(|key| key.as_bytes() == setting)
        {
            self.remaining.remove(index);
        }
        let existing = self.options.get(setting);
        let comment = matches!(commented, Commented::Optional) && existing.is_none();
        let value = existing.unwrap_or(default_value);
        let mut line = b"    ".to_vec();
        if comment {
            line.extend(b"// ");
        }
        line.push(b'"');
        line.extend(setting);
        line.extend(b"\": ");
        line.extend(format_value_or_array(setting, value));
        line.push(b',');
        self.lines.push(line);
    }
}

fn format_single_value(
    value: &ConfigValue,
    enum_map: &[(&str, EnumValue)],
    is_enum: bool,
) -> Vec<u8> {
    let named;
    let value = if is_enum {
        let name = enum_map
            .iter()
            .find(|(_, candidate)| match (value, candidate) {
                (ConfigValue::Enum(value), EnumValue::Number(candidate)) => value == candidate,
                (ConfigValue::String(value), EnumValue::String(candidate)) => {
                    value.as_bytes() == candidate.as_bytes()
                }
                _ => false,
            })
            .map(|(name, _)| *name)
            .unwrap_or_else(|| panic!("No matching value of {}", display_value(value)));
        named = ConfigValue::String(JsString::from_bytes(name.as_bytes()));
        &named
    } else {
        value
    };
    tsr_tsoptions::stringify_json(value)
        .unwrap_or_else(|error| panic!("should not happen: {error}"))
}

fn display_value(value: &ConfigValue) -> String {
    match value {
        ConfigValue::Null => "<nil>".to_owned(),
        ConfigValue::Boolean(value) => value.to_string(),
        ConfigValue::Integer(value) => value.to_string(),
        ConfigValue::Enum(value) => value.to_string(),
        ConfigValue::Number(value) => value.to_string(),
        ConfigValue::String(value) => String::from_utf8_lossy(value.as_bytes()).into_owned(),
        _ => String::from_utf8_lossy(&tsr_tsoptions::stringify_json(value).unwrap_or_default())
            .into_owned(),
    }
}

fn format_value_or_array(setting_name: &[u8], value: &ConfigValue) -> Vec<u8> {
    let option = COMPILER_OPTIONS
        .iter()
        .rev()
        .find(|option| option.name.as_bytes() == setting_name)
        .unwrap_or_else(|| panic!("No option named {}", String::from_utf8_lossy(setting_name)));
    let format_array = |values: &[ConfigValue]| {
        let (map, is_enum) = option.element.map_or((&[][..], false), |element| {
            (
                element.enum_values,
                element.kind == tsr_tsoptions::OptionKind::Enum,
            )
        });
        let values: Vec<_> = values
            .iter()
            .map(|value| format_single_value(value, map, is_enum))
            .collect();
        [b"[".as_slice(), &values.join(&b", "[..]), b"]"].concat()
    };
    match value {
        ConfigValue::Array(values) => format_array(values.as_deref().unwrap_or_default()),
        ConfigValue::StringArray(values) => format_array(
            &values
                .iter()
                .flatten()
                .cloned()
                .map(ConfigValue::String)
                .collect::<Vec<_>>(),
        ),
        _ => format_single_value(
            value,
            option.enum_values,
            option.kind == tsr_tsoptions::OptionKind::Enum,
        ),
    }
}

// port: tsc/internal/execute/tsc/init.go:generateTSConfig
pub fn generate_tsconfig(options: &OrderedMap<JsString, ConfigValue>, locale: &Locale) -> Vec<u8> {
    let mut writer = ConfigWriter {
        options,
        locale,
        remaining: options
            .keys()
            .filter(|key| !matches!(key.as_bytes(), b"init" | b"help" | b"watch"))
            .cloned()
            .collect(),
        lines: vec![
            b"{".to_vec(),
            [
                b"  // ".as_slice(),
                &d::Visit_https_Colon_Slash_Slashaka_ms_Slashtsconfig_to_read_more_about_this_file
                    .localize(locale, &[]),
            ]
            .concat(),
            b"  \"compilerOptions\": {".to_vec(),
        ],
    };
    let string = |text: &[u8]| ConfigValue::String(JsString::from_bytes(text));
    writer.header(d::File_Layout);
    writer.emit_option(b"rootDir", &string(b"./src"), Commented::Optional);
    writer.emit_option(b"outDir", &string(b"./dist"), Commented::Optional);
    writer.newline();
    writer.header(d::Environment_Settings);
    writer.header(d::See_also_https_Colon_Slash_Slashaka_ms_Slashtsconfig_Slashmodule);
    writer.emit_option(
        b"module",
        &ConfigValue::Enum(ModuleKind::NODE_NEXT.0),
        Commented::Never,
    );
    writer.emit_option(
        b"target",
        &ConfigValue::Enum(ScriptTarget::ESNEXT.0),
        Commented::Never,
    );
    writer.emit_option(
        b"types",
        &ConfigValue::Array(Some(Vec::new())),
        Commented::Never,
    );
    if let Some(lib) = options.get(b"lib".as_slice()) {
        writer.emit_option(b"lib", lib, Commented::Never);
    }
    writer.header(d::For_nodejs_Colon);
    writer.lines.push(b"    // \"lib\": [\"esnext\"],".to_vec());
    writer.lines.push(b"    // \"types\": [\"node\"],".to_vec());
    writer.header(d::X_and_npm_install_D_types_Slashnode);
    writer.newline();
    writer.header(d::Other_Outputs);
    for name in [b"sourceMap".as_slice(), b"declaration", b"declarationMap"] {
        writer.emit_option(name, &ConfigValue::Boolean(true), Commented::Never);
    }
    writer.newline();
    writer.header(d::Stricter_Typechecking_Options);
    for name in [
        b"noUncheckedIndexedAccess".as_slice(),
        b"exactOptionalPropertyTypes",
    ] {
        writer.emit_option(name, &ConfigValue::Boolean(true), Commented::Never);
    }
    writer.newline();
    writer.header(d::Style_Options);
    for name in [
        b"noImplicitReturns".as_slice(),
        b"noImplicitOverride",
        b"noUnusedLocals",
        b"noUnusedParameters",
        b"noFallthroughCasesInSwitch",
        b"noPropertyAccessFromIndexSignature",
    ] {
        writer.emit_option(name, &ConfigValue::Boolean(true), Commented::Optional);
    }
    writer.newline();
    writer.header(d::Recommended_Options);
    writer.emit_option(b"strict", &ConfigValue::Boolean(true), Commented::Never);
    writer.emit_option(
        b"jsx",
        &ConfigValue::Enum(JsxEmit::REACT_JSX.0),
        Commented::Never,
    );
    for name in [
        b"verbatimModuleSyntax".as_slice(),
        b"isolatedModules",
        b"noUncheckedSideEffectImports",
    ] {
        writer.emit_option(name, &ConfigValue::Boolean(true), Commented::Never);
    }
    writer.emit_option(
        b"moduleDetection",
        &ConfigValue::Enum(ModuleDetectionKind::FORCE.0),
        Commented::Never,
    );
    writer.emit_option(
        b"skipLibCheck",
        &ConfigValue::Boolean(true),
        Commented::Never,
    );
    if !writer.remaining.is_empty() {
        writer.newline();
        while let Some(key) = writer.remaining.first().cloned() {
            writer.emit_option(
                key.as_bytes(),
                options
                    .get(key.as_bytes())
                    .expect("remaining option exists"),
                Commented::Never,
            );
        }
    }
    writer
        .lines
        .extend([b"  }".to_vec(), b"}".to_vec(), Vec::new()]);
    writer.lines.join(&b'\n')
}
