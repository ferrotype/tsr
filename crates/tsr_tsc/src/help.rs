//! The pinned command-line help, including byte-based terminal wrapping.
#[cfg(test)]
#[path = "help_tests.rs"]
mod tests;

use crate::colors::{create_colors, Colors};
use crate::{write_all, System};
use tsr_diagnostics::{self as d, Argument, Message};
use tsr_locale::Locale;
use tsr_tsoptions::{
    DefaultValueDescription as DefaultValue, EnumValue, OptionDeclaration, OptionKind,
    ParsedCommandLine, BUILD_HELP_OPTIONS, COMPILER_OPTIONS, WATCH_OPTIONS,
};

fn localize(message: &Message, locale: &Locale) -> Vec<u8> {
    message.localize(locale, &[])
}

fn localize_code(code: i32, locale: &Locale) -> Vec<u8> {
    localize(
        d::by_code(code).expect("generated help diagnostic exists"),
        locale,
    )
}

// port: tsc/internal/execute/tsc/help.go:PrintVersion
pub fn print_version(sys: &dyn System, locale: &Locale) {
    let mut text = d::Version_0.localize(
        locale,
        &[Argument::Bytes(tsr_core::version().as_bytes().to_vec())],
    );
    text.push(b'\n');
    write_all(&*sys.writer(), &text);
}

// port: tsc/internal/execute/tsc/help.go:PrintHelp
pub fn print_help(sys: &dyn System, locale: &Locale, command_line: &ParsedCommandLine) {
    let options = get_options_for_help(command_line);
    let output = if command_line.options.all.is_false_or_unknown() {
        print_easy_help(sys, locale, &options)
    } else {
        print_all_help(sys, locale, &options)
    };
    write_all(&*sys.writer(), &output);
}

// port: tsc/internal/execute/tsc/help.go:getOptionsForHelp
fn get_options_for_help(command_line: &ParsedCommandLine) -> Vec<&'static OptionDeclaration> {
    let mut options: Vec<_> = COMPILER_OPTIONS.iter().collect();
    options.push(
        BUILD_HELP_OPTIONS
            .iter()
            .find(|option| option.name == "build")
            .expect("build declaration"),
    );
    if command_line.options.all.is_true() {
        options.sort_by_cached_key(|option| {
            tsr_jsstring::helpers::to_lower_go(option.name.as_bytes())
        });
    } else {
        options.retain(|option| option.show_in_simplified_help_view);
    }
    options
}

/// Go fmt's string width counts Unicode code points; all header/left-label
/// strings originate in the pinned UTF-8 diagnostic/declaration tables.
fn pad(text: &[u8], width: usize, right_align: bool) -> Vec<u8> {
    let rune_count = std::str::from_utf8(text)
        .expect("help label is UTF-8")
        .chars()
        .count();
    let padding = vec![b' '; width.saturating_sub(rune_count)];
    if right_align {
        [padding.as_slice(), text].concat()
    } else {
        [text, padding.as_slice()].concat()
    }
}

// port: tsc/internal/execute/tsc/help.go:getHeader
pub(crate) fn get_header(sys: &dyn System, message: &[u8]) -> Vec<u8> {
    let colors = create_colors(sys);
    let terminal_width = sys.get_width_of_terminal();
    if terminal_width >= message.len() as i64 + 5 {
        let left_align = terminal_width.min(120) as usize - 5;
        [
            pad(message, left_align, false),
            colors.blue_background(b"     "),
            b"\n".to_vec(),
            vec![b' '; left_align],
            colors.blue_background(&colors.bright_white(b"  TS ")),
            b"\n".to_vec(),
        ]
        .concat()
    } else {
        [message, b"\n\n"].concat()
    }
}

fn header(sys: &dyn System, locale: &Locale) -> Vec<u8> {
    let message = [
        localize(d::X_tsc_Colon_The_TypeScript_Compiler, locale),
        b" - ".to_vec(),
        d::Version_0.localize(
            locale,
            &[Argument::Bytes(tsr_core::version().as_bytes().to_vec())],
        ),
    ]
    .concat();
    get_header(sys, &message)
}

fn learn_more(locale: &Locale) -> Vec<u8> {
    d::You_can_learn_about_all_of_the_compiler_options_at_0
        .localize(locale, &[Argument::Bytes(b"https://aka.ms/tsc".to_vec())])
}

fn build_description(locale: &Locale) -> Vec<u8> {
    d::Using_build_b_will_make_tsc_behave_more_like_a_build_orchestrator_than_a_compiler_This_is_used_to_trigger_building_composite_projects_which_you_can_learn_more_about_at_0.localize(locale, &[Argument::Bytes(b"https://aka.ms/tsc-composite-builds".to_vec())])
}

// port: tsc/internal/execute/tsc/help.go:printEasyHelp
fn print_easy_help(sys: &dyn System, locale: &Locale, options: &[&OptionDeclaration]) -> Vec<u8> {
    let colors = create_colors(sys);
    let mut output = header(sys, locale);
    output.extend(colors.bold(&localize(d::COMMON_COMMANDS, locale)));
    output.extend(b"\n\n");
    let examples: &[(&[&[u8]], &Message)] = &[
        (
            &[b"tsc"],
            d::Compiles_the_current_project_tsconfig_json_in_the_working_directory,
        ),
        (
            &[b"tsc app.ts util.ts"],
            d::Ignoring_tsconfig_json_compiles_the_specified_files_with_default_compiler_options,
        ),
        (
            &[b"tsc -b"],
            d::Build_a_composite_project_in_the_working_directory,
        ),
        (
            &[b"tsc --init"],
            d::Creates_a_tsconfig_json_with_the_recommended_settings_in_the_working_directory,
        ),
        (
            &[b"tsc -p ./path/to/tsconfig.json"],
            d::Compiles_the_TypeScript_project_located_at_the_specified_path,
        ),
        (
            &[b"tsc --help --all"],
            d::An_expanded_version_of_this_information_showing_all_possible_compiler_options,
        ),
        (
            &[b"tsc --noEmit", b"tsc --target esnext"],
            d::Compiles_the_current_project_with_additional_settings,
        ),
    ];
    for (commands, description) in examples {
        for command in *commands {
            output.extend(b"  ");
            output.extend(colors.blue(command));
            output.push(b'\n');
        }
        output.extend(b"  ");
        output.extend(localize(description, locale));
        output.extend(b"\n\n");
    }
    let (commands, config): (Vec<_>, Vec<_>) = options.iter().copied().partition(|option| {
        option.is_command_line_only || option.category == Some(d::Command_line_Options.code)
    });
    output.extend(generate_section_options_output(
        sys,
        locale,
        &localize(d::COMMAND_LINE_FLAGS, locale),
        &commands,
        false,
        None,
        None,
    ));
    output.extend(generate_section_options_output(
        sys,
        locale,
        &localize(d::COMMON_COMPILER_OPTIONS, locale),
        &config,
        false,
        None,
        Some(&learn_more(locale)),
    ));
    output
}

// port: tsc/internal/execute/tsc/help.go:printAllHelp
fn print_all_help(sys: &dyn System, locale: &Locale, options: &[&OptionDeclaration]) -> Vec<u8> {
    let mut output = header(sys, locale);
    output.extend(generate_section_options_output(
        sys,
        locale,
        &localize(d::ALL_COMPILER_OPTIONS, locale),
        options,
        true,
        None,
        Some(&learn_more(locale)),
    ));
    output.extend(generate_section_options_output(sys, locale, &localize(d::WATCH_OPTIONS, locale), &WATCH_OPTIONS.iter().collect::<Vec<_>>(), false, Some(&localize(d::Including_watch_w_will_start_watching_the_current_project_for_the_file_changes_Once_set_you_can_config_watch_mode_with_Colon, locale)), None));
    let build: Vec<_> = BUILD_HELP_OPTIONS
        .iter()
        .filter(|option| option.name != "build")
        .collect();
    output.extend(generate_section_options_output(
        sys,
        locale,
        &localize(d::BUILD_OPTIONS, locale),
        &build,
        false,
        Some(&build_description(locale)),
        None,
    ));
    output
}

// port: tsc/internal/execute/tsc/help.go:PrintBuildHelp
pub fn print_build_help(sys: &dyn System, locale: &Locale, build_options: &[OptionDeclaration]) {
    let mut output = header(sys, locale);
    let options: Vec<_> = build_options
        .iter()
        .filter(|option| option.name != "build")
        .collect();
    output.extend(generate_section_options_output(
        sys,
        locale,
        &localize(d::BUILD_OPTIONS, locale),
        &options,
        false,
        Some(&build_description(locale)),
        None,
    ));
    write_all(&*sys.writer(), &output);
}

// port: tsc/internal/execute/tsc/help.go:generateSectionOptionsOutput
fn generate_section_options_output(
    sys: &dyn System,
    locale: &Locale,
    section_name: &[u8],
    options: &[&OptionDeclaration],
    sub_category: bool,
    before: Option<&[u8]>,
    after: Option<&[u8]>,
) -> Vec<u8> {
    let mut output = create_colors(sys).bold(section_name);
    output.extend(b"\n\n");
    if let Some(before) = before {
        output.extend(before);
        output.extend(b"\n\n");
    }
    if sub_category {
        let mut categories: Vec<(Vec<u8>, Vec<&OptionDeclaration>)> = Vec::new();
        for &option in options {
            let Some(code) = option.category else {
                continue;
            };
            let category = localize_code(code, locale);
            if let Some((_, entries)) = categories.iter_mut().find(|(key, _)| *key == category) {
                entries.push(option);
            } else {
                categories.push((category, vec![option]));
            }
        }
        for (category, entries) in categories {
            output.extend(b"### ");
            output.extend(category);
            output.extend(b"\n\n");
            output.extend(generate_group_option_output(sys, locale, &entries));
        }
    } else {
        output.extend(generate_group_option_output(sys, locale, options));
    }
    if let Some(after) = after {
        output.extend(after);
        output.extend(b"\n\n");
    }
    output
}

// port: tsc/internal/execute/tsc/help.go:generateGroupOptionOutput
fn generate_group_option_output(
    sys: &dyn System,
    locale: &Locale,
    options: &[&OptionDeclaration],
) -> Vec<u8> {
    let max_length = options
        .iter()
        .map(|option| get_display_name_text_of_option(option).len())
        .max()
        .unwrap_or(0);
    let mut output = Vec::new();
    for option in options {
        output.extend(generate_option_output(
            sys,
            locale,
            option,
            max_length + 2,
            max_length + 4,
        ));
    }
    // Each option appends two separate newline fragments; an empty group
    // still appends the one newline the pin's fragment check requires.
    if options.is_empty() {
        output.push(b'\n');
    }
    output
}

// port: tsc/internal/execute/tsc/help.go:generateOptionOutput
fn generate_option_output(
    sys: &dyn System,
    locale: &Locale,
    option: &OptionDeclaration,
    right_align_of_left: usize,
    left_align_of_right: usize,
) -> Vec<u8> {
    let colors = create_colors(sys);
    let name = get_display_name_text_of_option(option);
    let candidates = get_value_candidate(locale, option);
    let default = match option.default_value_description {
        DefaultValue::Message(code) => localize_code(code, locale),
        value => format_default_value(
            value,
            if matches!(option.kind, OptionKind::List | OptionKind::ListOrElement) {
                option.element.expect("list element")
            } else {
                option
            },
        ),
    };
    let description = option
        .description
        .map(|code| localize_code(code, locale))
        .unwrap_or_default();
    let width = sys.get_width_of_terminal();
    let mut output = Vec::new();
    if width >= 80 {
        output.extend(get_pretty_output(
            &colors,
            name.as_bytes(),
            &description,
            right_align_of_left,
            left_align_of_right,
            width as usize,
            true,
        ));
        output.push(b'\n');
        if show_additional_info_output(candidates.as_ref(), option) {
            if let Some(candidate) = &candidates {
                output.extend(get_pretty_output(
                    &colors,
                    &candidate.value_type,
                    candidate.possible_values.as_bytes(),
                    right_align_of_left,
                    left_align_of_right,
                    width as usize,
                    false,
                ));
                output.push(b'\n');
            }
            if !default.is_empty() {
                output.extend(get_pretty_output(
                    &colors,
                    &localize(d::X_default_Colon, locale),
                    &default,
                    right_align_of_left,
                    left_align_of_right,
                    width as usize,
                    false,
                ));
                output.push(b'\n');
            }
        }
        output.push(b'\n');
    } else {
        output.extend(colors.blue(name.as_bytes()));
        output.push(b'\n');
        output.extend(description);
        output.push(b'\n');
        if show_additional_info_output(candidates.as_ref(), option) {
            if let Some(candidate) = &candidates {
                output.extend(&candidate.value_type);
                output.push(b' ');
                output.extend(candidate.possible_values.as_bytes());
            }
            if !default.is_empty() {
                if candidates.is_some() {
                    output.push(b'\n');
                }
                output.extend(localize(d::X_default_Colon, locale));
                output.push(b' ');
                output.extend(default);
            }
            output.push(b'\n');
        }
        output.push(b'\n');
    }
    output
}

// port: tsc/internal/execute/tsc/help.go:formatDefaultValue
fn format_default_value(value: DefaultValue, option: &OptionDeclaration) -> Vec<u8> {
    if matches!(value, DefaultValue::Nil | DefaultValue::Unknown) {
        return b"undefined".to_vec();
    }
    if option.kind == OptionKind::Enum {
        return option
            .enum_values
            .iter()
            .filter(|(_, candidate)| match (value, candidate) {
                (DefaultValue::Number(value), EnumValue::Number(candidate)) => {
                    value == i64::from(*candidate)
                }
                (DefaultValue::String(value), EnumValue::String(candidate)) => value == *candidate,
                _ => false,
            })
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join("/")
            .into_bytes();
    }
    match value {
        DefaultValue::Boolean(value) => value.to_string().into_bytes(),
        DefaultValue::Number(value) => value.to_string().into_bytes(),
        DefaultValue::String(value) => value.as_bytes().to_vec(),
        _ => unreachable!("message and undefined defaults are handled before scalar formatting"),
    }
}

struct ValueCandidate {
    value_type: Vec<u8>,
    possible_values: String,
}

// port: tsc/internal/execute/tsc/help.go:showAdditionalInfoOutput
fn show_additional_info_output(
    candidate: Option<&ValueCandidate>,
    option: &OptionDeclaration,
) -> bool {
    if option.category == Some(d::Command_line_Options.code) {
        return false;
    }
    !(candidate.is_some_and(|candidate| candidate.possible_values == "string")
        && matches!(
            option.default_value_description,
            DefaultValue::Nil | DefaultValue::String("false" | "n/a")
        ))
}

// port: tsc/internal/execute/tsc/help.go:getValueCandidate
fn get_value_candidate(locale: &Locale, option: &OptionDeclaration) -> Option<ValueCandidate> {
    let message = match option.kind {
        OptionKind::Object => return None,
        OptionKind::ListOrElement => panic!("no value candidate for list or element"),
        OptionKind::String | OptionKind::Number | OptionKind::Boolean => d::X_type_Colon,
        OptionKind::List => d::X_one_or_more_Colon,
        OptionKind::Enum => d::X_one_of_Colon,
    };
    Some(ValueCandidate {
        value_type: localize(message, locale),
        possible_values: get_possible_values(option),
    })
}

// port: tsc/internal/execute/tsc/help.go:getPossibleValues
fn get_possible_values(option: &OptionDeclaration) -> String {
    match option.kind {
        OptionKind::String | OptionKind::Number | OptionKind::Boolean => {
            option.kind.as_str().to_owned()
        }
        OptionKind::List | OptionKind::ListOrElement => {
            get_possible_values(option.element.expect("list element"))
        }
        OptionKind::Object => String::new(),
        OptionKind::Enum => {
            let mut groups: Vec<(EnumValue, Vec<&str>)> = Vec::new();
            for &(name, value) in option.enum_values {
                if option.deprecated_keys.contains(&name) {
                    continue;
                }
                if let Some((_, names)) = groups.iter_mut().find(|(key, _)| *key == value) {
                    names.push(name);
                } else {
                    groups.push((value, vec![name]));
                }
            }
            groups
                .iter()
                .map(|(_, names)| names.join("/"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
}

// port: tsc/internal/execute/tsc/help.go:getPrettyOutput
fn get_pretty_output(
    colors: &Colors,
    left: &[u8],
    right: &[u8],
    right_align_of_left: usize,
    left_align_of_right: usize,
    terminal_width: usize,
    color_left: bool,
) -> Vec<u8> {
    let mut output = Vec::new();
    let width = terminal_width - left_align_of_right;
    // The public help paths use terminal width >=80 and shorter static labels.
    assert!(width > 0, "help right column must fit in the terminal");
    for (index, line) in right.chunks(width).enumerate() {
        let left = if index == 0 {
            let left = pad(
                &pad(left, right_align_of_left, true),
                left_align_of_right,
                false,
            );
            if color_left {
                colors.blue(&left)
            } else {
                left
            }
        } else {
            vec![b' '; left_align_of_right]
        };
        output.extend(left);
        output.extend(line);
        output.push(b'\n');
    }
    output
}

// port: tsc/internal/execute/tsc/help.go:getDisplayNameTextOfOption
fn get_display_name_text_of_option(option: &OptionDeclaration) -> String {
    let mut result = format!("--{}", option.name);
    if !option.short_name.is_empty() {
        result.push_str(&format!(", -{}", option.short_name));
    }
    result
}
