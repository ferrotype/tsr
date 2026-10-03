//! `harnessutil.go`, the configuration half: which options a test may vary
//! (`// @strict: true, false`), the expansion into named configurations,
//! and the pin's skip rules for options the Go compiler does not support.
use crate::harness_options::{
    get_all_values_for_option, get_value_of_option_string, try_get_value_of_option_string,
    NamedTestConfiguration, TestConfiguration,
};
use crate::test_case_parser::{to_lower, trim_space, RawCompilerSettings};
use crate::Stop;
use std::collections::BTreeSet;
use std::sync::LazyLock;
use tsr_core::{CompilerOptions, ModuleKind, ModuleResolutionKind, ScriptTarget};
use tsr_tsoptions::ConfigValue;

/// The lowercased names of the options a compiler test may vary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VaryBy {
    pub options: BTreeSet<String>,
}

/// The most configurations one test file may expand into.
const MAX_VARIATIONS: usize = 25;

/// `compilerVaryBy` at the pin, lowercased and sorted: every option of
/// `tsoptions.OptionsDeclarations` that is not command-line-only, is a
/// boolean or an enum, and has one of the eight `Affects*` flags, plus
/// `noEmit` and `isolatedModules`. `tests::compiler_vary_by_is_derived_from_the_pin`
/// re-derives it from `tsoptions/declscompiler.go`.
const COMPILER_VARY_BY: [&str; 72] = [
    "allowarbitraryextensions",
    "allowimportingtsextensions",
    "allowjs",
    "allowsyntheticdefaultimports",
    "allowumdglobalaccess",
    "allowunreachablecode",
    "allowunusedlabels",
    "alwaysstrict",
    "assumechangesonlyaffectdirectdependencies",
    "checkjs",
    "composite",
    "declaration",
    "declarationmap",
    "deduplicatepackages",
    "disablesizelimit",
    "downleveliteration",
    "emitbom",
    "emitdeclarationonly",
    "emitdecoratormetadata",
    "erasablesyntaxonly",
    "esmoduleinterop",
    "exactoptionalpropertytypes",
    "experimentaldecorators",
    "forceconsistentcasinginfilenames",
    "importhelpers",
    "inlinesourcemap",
    "inlinesources",
    "isolateddeclarations",
    "isolatedmodules",
    "jsx",
    "libreplacement",
    "module",
    "moduledetection",
    "moduleresolution",
    "newline",
    "noemit",
    "noemithelpers",
    "noemitonerror",
    "noerrortruncation",
    "nofallthroughcasesinswitch",
    "noimplicitany",
    "noimplicitoverride",
    "noimplicitreturns",
    "noimplicitthis",
    "nolib",
    "nopropertyaccessfromindexsignature",
    "noresolve",
    "nouncheckedindexedaccess",
    "nouncheckedsideeffectimports",
    "nounusedlocals",
    "nounusedparameters",
    "preserveconstenums",
    "removecomments",
    "resolvejsonmodule",
    "resolvepackagejsonexports",
    "resolvepackagejsonimports",
    "rewriterelativeimportextensions",
    "skipdefaultlibcheck",
    "skiplibcheck",
    "sourcemap",
    "stabletypeordering",
    "strict",
    "strictbindcallapply",
    "strictbuiltiniteratorreturn",
    "strictfunctiontypes",
    "strictnullchecks",
    "strictpropertyinitialization",
    "stripinternal",
    "target",
    "usedefineforclassfields",
    "useunknownincatchvariables",
    "verbatimmodulesyntax",
];

/// `getCompilerVaryByMap`: every non-command-line boolean or enum option
/// that affects program structure, emit, module resolution, bind or
/// semantic diagnostics, source files, declaration paths or build info,
/// plus `noEmit` and `isolatedModules`. The Rust option tables do not carry
/// all of those `Affects*` flags, so the set is a constant taken from the
/// pin's `tsoptions/declscompiler.go`, with a unit test that re-derives it
/// from that Go source text.
// port: tsc/internal/testrunner/compiler_runner.go:getCompilerVaryByMap
pub fn compiler_vary_by() -> &'static VaryBy {
    static VARY_BY: LazyLock<VaryBy> = LazyLock::new(|| VaryBy {
        options: COMPILER_VARY_BY
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
    });
    &VARY_BY
}

/// Expands `settings` into configurations: options in `vary_by` with more
/// than one value multiply (at most 25 variations, else fatal); the rest are
/// shared. An option in `vary_by` whose value lists nothing is dropped. With
/// nothing varying but some settings, one unnamed configuration; with no
/// settings, none.
///
/// The pin ranges over Go maps, so its configurations come in a random
/// order; here the varying options multiply in name order and each one's
/// values keep `split_option_values`' order. The names do not depend on it.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:GetFileBasedTestConfigurations
pub fn get_file_based_test_configurations(
    settings: &RawCompilerSettings,
    vary_by: &VaryBy,
) -> Result<Vec<NamedTestConfiguration>, Stop> {
    // Each entry has the option name and its values
    let mut option_entries: Vec<(String, Vec<String>)> = Vec::new();
    let mut variation_count = 1;
    let mut non_varying_options = TestConfiguration::new();
    for (option, value) in settings {
        if vary_by.options.contains(option) {
            let entries = split_option_values(value, option)?;
            if entries.len() > 1 {
                variation_count *= entries.len();
                if variation_count > MAX_VARIATIONS {
                    return Err(Stop::fatal(
                        "Provided test options exceeded the maximum number of variations",
                    ));
                }
                option_entries.push((option.clone(), entries));
            } else if let [entry] = entries.as_slice() {
                non_varying_options.insert(option.clone(), entry.clone());
            }
        } else {
            // Variation is not supported for the option
            non_varying_options.insert(option.clone(), value.clone());
        }
    }

    let mut configurations = Vec::new();
    if !option_entries.is_empty() {
        // Merge varying and non-varying options
        for mut varying_config in compute_variations(&option_entries) {
            let name = description(&varying_config);
            varying_config.extend(
                non_varying_options
                    .iter()
                    .map(|(option, value)| (option.clone(), value.clone())),
            );
            configurations.push(NamedTestConfiguration {
                name,
                config: varying_config,
            });
        }
    } else if !non_varying_options.is_empty() {
        // Only non-varying options
        configurations.push(NamedTestConfiguration {
            name: String::new(),
            config: non_varying_options,
        });
    }
    Ok(configurations)
}

/// `key=value,key=value` over the sorted keys, values lowercased.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:getFileBasedTestConfigurationDescription
pub fn description(config: &TestConfiguration) -> String {
    let mut output = String::new();
    for (index, (key, value)) in config.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(key);
        output.push('=');
        output.push_str(&to_lower(value));
    }
    output
}

/// `esnext, es2015, es6` → the distinct values by normalized option value,
/// `*` → every value, `-x` / `!x` → exclusions (unknown exclusions are
/// skipped). An empty result is a panic in the pin, `Stop::Fatal` here; an
/// unknown included value is fatal.
///
/// ```text
/// split_option_values("esnext, es2015, es6", "target") => ["esnext", "es2015"]
/// split_option_values("*", "strict") => ["true", "false"]
/// split_option_values("*, -true", "strict") => ["false"]
/// ```
///
/// The pin returns its map's values in a random order; here a value keeps
/// the position and the spelling of its first include, then `*` appends the
/// values not yet present, each spelled as the first enum key denoting it.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:splitOptionValues
pub fn split_option_values(value: &str, option: &str) -> Result<Vec<String>, Stop> {
    if value.is_empty() {
        return Ok(Vec::new());
    }

    let mut star = false;
    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    for part in value.as_bytes().split(|&byte| byte == b',') {
        let part = trim_space(part);
        if part.is_empty() {
            continue;
        }
        // `part` is a run of whole characters: commas and the trimmed
        // white space are complete UTF-8 sequences.
        let part = std::str::from_utf8(part).expect("a trimmed comma-separated part of a str");
        if part == "*" {
            star = true;
        } else if let Some(exclude) = part.strip_prefix(['-', '!']) {
            excludes.push(exclude);
        } else {
            includes.push(part);
        }
    }

    if includes.is_empty() && !star && excludes.is_empty() {
        return Ok(Vec::new());
    }

    // Dedupe the variations by their normalized values
    let mut variations: Vec<(ConfigValue, String)> = Vec::new();
    let add = |variations: &mut Vec<(ConfigValue, String)>, include: &str| {
        let value = get_value_of_option_string(option, include)?;
        if !variations.iter().any(|(existing, _)| *existing == value) {
            variations.push((value, include.to_string()));
        }
        Ok::<(), Stop>(())
    };

    // add (and deduplicate) all included entries
    for include in includes {
        add(&mut variations, include)?;
    }

    let all_values = get_all_values_for_option(option);
    if star && !all_values.is_empty() {
        // add all entries
        for include in &all_values {
            add(&mut variations, include)?;
        }
    }

    // remove all excluded entries
    for exclude in excludes {
        // An unrecognized excluded value (a removed option value like "es3")
        // has nothing to remove.
        let Some(value) = try_get_value_of_option_string(option, exclude) else {
            continue;
        };
        variations.retain(|(existing, _)| *existing != value);
    }

    if variations.is_empty() {
        return Err(Stop::fatal(format!(
            "Variations in test option '@{option}' resulted in an empty set."
        )));
    }
    Ok(variations.into_iter().map(|(_, include)| include).collect())
}

/// The cross product of the varying options' values, in the pin's order:
/// the first option varies slowest.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:computeFileBasedTestConfigurationVariations
pub fn compute_variations(option_entries: &[(String, Vec<String>)]) -> Vec<TestConfiguration> {
    let capacity = option_entries
        .iter()
        .map(|(_, entries)| entries.len())
        .product();
    let mut configurations = Vec::with_capacity(capacity);
    compute_variations_worker(
        &mut configurations,
        option_entries,
        0,
        &mut TestConfiguration::new(),
    );
    configurations
}

// port: tsc/internal/testutil/harnessutil/harnessutil.go:computeFileBasedTestConfigurationVariationsWorker
fn compute_variations_worker(
    configurations: &mut Vec<TestConfiguration>,
    option_entries: &[(String, Vec<String>)],
    index: usize,
    variation_state: &mut TestConfiguration,
) {
    let Some((option_key, entries)) = option_entries.get(index) else {
        configurations.push(variation_state.clone());
        return;
    };
    for entry in entries {
        // set or overwrite the variation, then compute the next variation
        variation_state.insert(option_key.clone(), entry.clone());
        compute_variations_worker(configurations, option_entries, index + 1, variation_state);
    }
}

/// Fatal for `module: amd` and `outFile`; skip for `module: umd|system`,
/// `moduleResolution: node10|classic`, `esModuleInterop: false`,
/// `allowSyntheticDefaultImports: false`, a `baseUrl`, `target: es5` and
/// `alwaysStrict: false`. The messages are the pin's `Fatalf`/`Skipf` text.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:SkipUnsupportedCompilerOptions
pub fn skip_unsupported_compiler_options(options: &CompilerOptions) -> Result<(), Stop> {
    fail_on_unsupported_compiler_options(options)?;
    if matches!(options.module, ModuleKind::UMD | ModuleKind::SYSTEM) {
        return Err(Stop::skip(format!(
            "unsupported module kind {}",
            options.module
        )));
    }
    if matches!(
        options.module_resolution,
        ModuleResolutionKind::NODE10 | ModuleResolutionKind::CLASSIC
    ) {
        return Err(Stop::skip(format!(
            "unsupported module resolution kind {}",
            options.module_resolution.0
        )));
    }
    if options.es_module_interop.is_false() {
        return Err(Stop::skip("esModuleInterop=false is unsupported"));
    }
    if options.allow_synthetic_default_imports.is_false() {
        return Err(Stop::skip(
            "allowSyntheticDefaultImports=false is unsupported",
        ));
    }
    if !options.base_url.is_empty() {
        return Err(Stop::skip(format!(
            "unsupported baseUrl {}",
            String::from_utf8_lossy(options.base_url.as_bytes())
        )));
    }
    if options.target == ScriptTarget::ES5 {
        return Err(Stop::skip(format!("unsupported target {}", options.target)));
    }
    if options.always_strict.is_false() {
        return Err(Stop::skip("alwaysStrict=false is unsupported"));
    }
    Ok(())
}

// port: tsc/internal/testutil/harnessutil/harnessutil.go:failOnUnsupportedCompilerOptions
fn fail_on_unsupported_compiler_options(options: &CompilerOptions) -> Result<(), Stop> {
    if options.module == ModuleKind::AMD {
        return Err(Stop::fatal(format!(
            "unsupported module kind {}",
            options.module
        )));
    }
    if !options.out_file.is_empty() {
        return Err(Stop::fatal(format!(
            "unsupported outFile {}",
            String::from_utf8_lossy(options.out_file.as_bytes())
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        compiler_vary_by, description, get_file_based_test_configurations,
        skip_unsupported_compiler_options, split_option_values, VaryBy,
    };
    use crate::harness_options::{NamedTestConfiguration, TestConfiguration};
    use crate::Stop;
    use std::collections::{BTreeMap, BTreeSet};
    use tsr_core::{CompilerOptions, ModuleKind, ModuleResolutionKind, ScriptTarget, Tristate};
    use tsr_jsstring::JsString;
    use tsr_tsoptions::{OptionKind, COMPILER_OPTIONS};

    /// The configured names of `configurations` for a test `foo.ts`.
    fn configured_names(configurations: &[NamedTestConfiguration]) -> BTreeSet<String> {
        configurations
            .iter()
            .map(|configuration| crate::enumerate::configured_name("foo.ts", Some(configuration)))
            .collect()
    }

    fn settings(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    /// One declaration of `declscompiler.go`: its top-level `Field: value`
    /// lines, trailing commas and comments removed.
    type GoDeclaration = BTreeMap<String, String>;

    /// The `[]*CommandLineOption{...}` literal named `name`: each `\t{` ...
    /// `\t},` element at the top level of the slice.
    fn go_declarations(source: &str, name: &str) -> Vec<GoDeclaration> {
        let header = format!("var {name} = []*CommandLineOption{{\n");
        let start = source.find(&header).expect("declaration slice") + header.len();
        let end = start + source[start..].find("\n}\n").expect("end of slice");
        let mut declarations = Vec::new();
        let mut current: Option<GoDeclaration> = None;
        for line in source[start..end].lines() {
            match line {
                "\t{" => current = Some(GoDeclaration::new()),
                "\t}," => declarations.push(current.take().expect("open declaration")),
                _ => {
                    let Some(fields) = current.as_mut() else {
                        continue;
                    };
                    // Only the declaration's own fields: two tabs, then a name.
                    let Some(field) = line.strip_prefix("\t\t") else {
                        continue;
                    };
                    if field.starts_with(['\t', '/']) {
                        continue;
                    }
                    let Some((key, value)) = field.split_once(':') else {
                        continue;
                    };
                    let value = value.split(" //").next().unwrap_or_default().trim();
                    fields.insert(
                        key.trim().to_string(),
                        value.trim_end_matches(',').trim().to_string(),
                    );
                }
            }
        }
        assert!(current.is_none(), "unterminated declaration in {name}");
        declarations
    }

    /// `getCompilerVaryByMap` evaluated over the pin's Go source text.
    #[test]
    fn compiler_vary_by_is_derived_from_the_pin() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../upstream/tsc/internal/tsoptions/declscompiler.go"
        );
        let source = std::fs::read_to_string(path).expect("the pinned declscompiler.go");
        assert!(source.contains(
            "var OptionsDeclarations = slices.Concat(commonOptionsWithBuild, optionsForCompiler)"
        ));
        let declarations: Vec<GoDeclaration> = ["commonOptionsWithBuild", "optionsForCompiler"]
            .iter()
            .flat_map(|name| go_declarations(&source, name))
            .collect();

        // The parse reads the same declarations as the Rust table, in order.
        assert_eq!(declarations.len(), COMPILER_OPTIONS.len());
        for (go, rust) in declarations.iter().zip(COMPILER_OPTIONS) {
            assert_eq!(go["Name"], format!("\"{}\"", rust.name));
            let kind = match rust.kind {
                OptionKind::String => "CommandLineOptionTypeString",
                OptionKind::Number => "CommandLineOptionTypeNumber",
                OptionKind::Boolean => "CommandLineOptionTypeBoolean",
                OptionKind::Object => "CommandLineOptionTypeObject",
                OptionKind::List => "CommandLineOptionTypeList",
                OptionKind::ListOrElement => "CommandLineOptionTypeListOrElement",
                OptionKind::Enum => "CommandLineOptionTypeEnum",
            };
            assert_eq!(go["Kind"], kind, "{}", rust.name);
            assert_eq!(
                go.get("IsCommandLineOnly")
                    .is_some_and(|value| value == "true"),
                rust.is_command_line_only,
                "{}",
                rust.name
            );
        }

        const AFFECTS: [&str; 8] = [
            "AffectsProgramStructure",
            "AffectsEmit",
            "AffectsModuleResolution",
            "AffectsBindDiagnostics",
            "AffectsSemanticDiagnostics",
            "AffectsSourceFile",
            "AffectsDeclarationPath",
            "AffectsBuildInfo",
        ];
        let flag = |declaration: &GoDeclaration, name: &str| {
            declaration.get(name).is_some_and(|value| value == "true")
        };
        let mut expected: BTreeSet<String> = declarations
            .iter()
            .filter(|declaration| {
                !flag(declaration, "IsCommandLineOnly")
                    && matches!(
                        declaration["Kind"].as_str(),
                        "CommandLineOptionTypeBoolean" | "CommandLineOptionTypeEnum"
                    )
                    && AFFECTS.iter().any(|affects| flag(declaration, affects))
            })
            .map(|declaration| declaration["Name"].trim_matches('"').to_lowercase())
            .collect();
        expected.extend(["noemit".to_string(), "isolatedmodules".to_string()]);
        assert_eq!(compiler_vary_by().options, expected);
    }

    #[test]
    fn split_option_values_matches_the_pins_examples() {
        assert_eq!(
            split_option_values("esnext, es2015, es6", "target"),
            Ok(vec!["esnext".to_string(), "es2015".to_string()])
        );
        assert_eq!(
            split_option_values("*", "strict"),
            Ok(vec!["true".to_string(), "false".to_string()])
        );
        assert_eq!(
            split_option_values("*, -true", "strict"),
            Ok(vec!["false".to_string()])
        );
    }

    #[test]
    fn split_option_values_edge_cases() {
        assert_eq!(split_option_values("", "target"), Ok(vec![]));
        assert_eq!(split_option_values(" , ", "target"), Ok(vec![]));
        // Unknown exclusions are skipped; `!` excludes like `-`.
        assert_eq!(
            split_option_values("ES2015, -es3, !esnext, esnext", "target"),
            Ok(vec!["ES2015".to_string()])
        );
        assert_eq!(
            split_option_values("true, -true", "strict"),
            Err(Stop::fatal(
                "Variations in test option '@strict' resulted in an empty set."
            ))
        );
        assert_eq!(
            split_option_values("es3, es5", "target"),
            Err(Stop::fatal("Unknown value 'es3' for option 'target'"))
        );
        // `*` spells each value as its first enum key: es6, not es2015.
        let all = split_option_values("*", "target").expect("every target");
        assert_eq!(all.len(), 13);
        assert!(all.contains(&"es6".to_string()));
        assert!(!all.contains(&"es2015".to_string()));
        assert_eq!(
            split_option_values("es2015, *, -es5", "target").expect("targets")[0],
            "es2015"
        );
    }

    #[test]
    fn configurations_multiply_varying_options_and_share_the_rest() {
        let configurations = get_file_based_test_configurations(
            &settings(&[
                ("target", "ES2015, es5"),
                ("strict", "true,false"),
                ("noemit", "true"),
                ("lib", "es2015, dom"),
                ("declaration", ""),
            ]),
            compiler_vary_by(),
        )
        .expect("configurations");
        let names: Vec<&str> = configurations
            .iter()
            .map(|configuration| configuration.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "strict=true,target=es2015",
                "strict=true,target=es5",
                "strict=false,target=es2015",
                "strict=false,target=es5",
            ]
        );
        let expected: TestConfiguration = settings(&[
            ("target", "ES2015"),
            ("strict", "true"),
            ("noemit", "true"),
            ("lib", "es2015, dom"),
        ]);
        assert_eq!(configurations[0].config, expected);

        // One value of a varying option: one unnamed configuration.
        let single = get_file_based_test_configurations(
            &settings(&[("strict", "true"), ("filename", "a.ts")]),
            compiler_vary_by(),
        )
        .expect("configurations");
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].name, "");
        assert_eq!(
            single[0].config,
            settings(&[("strict", "true"), ("filename", "a.ts")])
        );
        assert!(
            get_file_based_test_configurations(&settings(&[]), compiler_vary_by())
                .expect("none")
                .is_empty()
        );
        // An option outside the vary-by set is shared as written.
        let unvaried = get_file_based_test_configurations(
            &settings(&[("target", "es2015, es5")]),
            &VaryBy::default(),
        )
        .expect("configurations");
        assert_eq!(unvaried.len(), 1);
        assert_eq!(unvaried[0].config, settings(&[("target", "es2015, es5")]));
    }

    #[test]
    fn more_than_twenty_five_variations_are_fatal() {
        let five_by_five = get_file_based_test_configurations(
            &settings(&[
                ("target", "es2015, es2016, es2017, es2018, es2019"),
                ("module", "commonjs, es2015, es2020, es2022, esnext"),
            ]),
            compiler_vary_by(),
        )
        .expect("25 variations");
        assert_eq!(five_by_five.len(), 25);
        assert_eq!(configured_names(&five_by_five).len(), 25);
        assert_eq!(
            get_file_based_test_configurations(
                &settings(&[
                    ("target", "es2015, es2016, es2017, es2018, es2019"),
                    ("module", "commonjs, es2015, es2020, es2022, esnext, node16"),
                ]),
                compiler_vary_by(),
            ),
            Err(Stop::fatal(
                "Provided test options exceeded the maximum number of variations"
            ))
        );
    }

    #[test]
    fn descriptions_sort_keys_and_lowercase_values() {
        assert_eq!(
            description(&settings(&[("target", "ESNext"), ("module", "NodeNext")])),
            "module=nodenext,target=esnext"
        );
        assert_eq!(description(&TestConfiguration::new()), "");
    }

    #[test]
    fn unsupported_options_skip_or_fail() {
        let check = |options: CompilerOptions| skip_unsupported_compiler_options(&options);
        assert_eq!(check(CompilerOptions::default()), Ok(()));
        assert_eq!(
            check(CompilerOptions {
                module: ModuleKind::AMD,
                target: ScriptTarget::ES5,
                ..CompilerOptions::default()
            }),
            Err(Stop::fatal("unsupported module kind AMD"))
        );
        assert_eq!(
            check(CompilerOptions {
                out_file: JsString::from_bytes(b"/out.js".as_slice()),
                ..CompilerOptions::default()
            }),
            Err(Stop::fatal("unsupported outFile /out.js"))
        );
        assert_eq!(
            check(CompilerOptions {
                module: ModuleKind::SYSTEM,
                ..CompilerOptions::default()
            }),
            Err(Stop::skip("unsupported module kind System"))
        );
        assert_eq!(
            check(CompilerOptions {
                module_resolution: ModuleResolutionKind::NODE10,
                ..CompilerOptions::default()
            }),
            Err(Stop::skip("unsupported module resolution kind 2"))
        );
        assert_eq!(
            check(CompilerOptions {
                es_module_interop: Tristate::FALSE,
                ..CompilerOptions::default()
            }),
            Err(Stop::skip("esModuleInterop=false is unsupported"))
        );
        assert_eq!(
            check(CompilerOptions {
                allow_synthetic_default_imports: Tristate::FALSE,
                ..CompilerOptions::default()
            }),
            Err(Stop::skip(
                "allowSyntheticDefaultImports=false is unsupported"
            ))
        );
        assert_eq!(
            check(CompilerOptions {
                base_url: JsString::from_bytes(b"/.src".as_slice()),
                ..CompilerOptions::default()
            }),
            Err(Stop::skip("unsupported baseUrl /.src"))
        );
        assert_eq!(
            check(CompilerOptions {
                target: ScriptTarget::ES5,
                ..CompilerOptions::default()
            }),
            Err(Stop::skip("unsupported target ES5"))
        );
        assert_eq!(
            check(CompilerOptions {
                always_strict: Tristate::FALSE,
                ..CompilerOptions::default()
            }),
            Err(Stop::skip("alwaysStrict=false is unsupported"))
        );
        assert_eq!(
            check(CompilerOptions {
                always_strict: Tristate::TRUE,
                es_module_interop: Tristate::TRUE,
                module: ModuleKind::NODE_NEXT,
                ..CompilerOptions::default()
            }),
            Ok(())
        );
    }
}
