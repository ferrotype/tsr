//! Local paths and extension preferences from modulespecifiers/{preferences,specifiers}.go.
use super::{Error, Generation, Import};
use tsr_core::{CompilerOptions, JsxEmit, ModuleResolutionKind as MR, ResolutionMode as Mode};
use tsr_tspath as path;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Ending {
    Minimal,
    Index,
    Js,
    Ts,
}

pub(super) fn has_extension(file: &[u8], extensions: &[&[u8]]) -> bool {
    extensions
        .iter()
        .any(|ext| file.len() > ext.len() && file.ends_with(ext))
}
const TS: &[&[u8]] = &[b".ts", b".tsx", b".mts", b".cts"];
const JS: &[&[u8]] = &[b".js", b".jsx", b".mjs", b".cjs"];
const REQUIRED: &[&[u8]] = &[b".mts", b".d.mts", b".mjs", b".cts", b".d.cts", b".cjs"];

fn inferred_ending(imports: &[Import], mode: Mode, node_next: bool) -> Ending {
    let mut uses_js = false;
    for import in imports {
        let name = import.text.as_bytes();
        if !path::is_relative(name)
            || node_next && mode == Mode::COMMON_JS
            || has_extension(name, REQUIRED)
        {
            continue;
        }
        if has_extension(name, TS) {
            return Ending::Ts;
        }
        uses_js |= has_extension(name, JS);
    }
    if uses_js {
        Ending::Js
    } else {
        Ending::Minimal
    }
}

// port: tsc/internal/modulespecifiers/preferences.go:GetAllowedEndingsInPreferredOrder
pub(super) fn allowed_endings(
    options: &CompilerOptions,
    file: &[u8],
    imports: &[Import],
    default_mode: Mode,
    syntax_mode: Mode,
    request_js: bool,
) -> Vec<Ending> {
    let resolution = options.module_resolution_kind();
    let node_next = MR::NODE16 <= resolution && resolution <= MR::NODE_NEXT;
    let mode = if syntax_mode == Mode::NONE {
        default_mode
    } else {
        syntax_mode
    };
    let allow_ts = options.allow_importing_ts_extensions_from(file);
    if mode == Mode::ESNEXT && node_next {
        return if allow_ts {
            vec![Ending::Ts, Ending::Js]
        } else {
            vec![Ending::Js]
        };
    }
    let preferred = if request_js || mode == Mode::ESNEXT && node_next {
        if options.allow_importing_ts_extensions()
            && inferred_ending(imports, mode, node_next) != Ending::Js
        {
            Ending::Ts
        } else {
            Ending::Js
        }
    } else if options.allow_importing_ts_extensions() {
        inferred_ending(imports, mode, node_next)
    } else {
        let first = imports.iter().find(|import| {
            path::is_relative(import.text.as_bytes())
                && !has_extension(import.text.as_bytes(), REQUIRED)
        });
        if first.is_some_and(|import| {
            has_extension(import.text.as_bytes(), TS) || has_extension(import.text.as_bytes(), JS)
        }) {
            Ending::Js
        } else {
            Ending::Minimal
        }
    };
    match (preferred, allow_ts) {
        (Ending::Js, true) => vec![Ending::Js, Ending::Ts, Ending::Minimal, Ending::Index],
        (Ending::Js, false) => vec![Ending::Js, Ending::Minimal, Ending::Index],
        (Ending::Ts, _) => vec![Ending::Ts, Ending::Minimal, Ending::Js, Ending::Index],
        (Ending::Index, true) => vec![Ending::Index, Ending::Minimal, Ending::Ts, Ending::Js],
        (Ending::Index, false) => vec![Ending::Index, Ending::Minimal, Ending::Js],
        (Ending::Minimal, true) => vec![Ending::Minimal, Ending::Index, Ending::Ts, Ending::Js],
        (Ending::Minimal, false) => vec![Ending::Minimal, Ending::Index, Ending::Js],
    }
}

// port: tsc/internal/module/util.go:TryGetJSExtensionForFile
pub(super) fn js_extension(file: &[u8], options: &CompilerOptions) -> Option<&'static [u8]> {
    if has_extension(file, &[b".json"]) {
        Some(b".json")
    } else if has_extension(file, &[b".mts", b".mjs"]) {
        Some(b".mjs")
    } else if has_extension(file, &[b".cts", b".cjs"]) {
        Some(b".cjs")
    } else if has_extension(file, &[b".jsx"]) {
        Some(b".jsx")
    } else if has_extension(file, &[b".tsx"]) {
        Some(if options.jsx == JsxEmit::PRESERVE {
            b".jsx"
        } else {
            b".js"
        })
    } else if has_extension(file, &[b".ts", b".js"]) {
        Some(b".js")
    } else {
        None
    }
}

pub(super) fn ensure_non_module(path: Vec<u8>) -> Vec<u8> {
    if !path::is_relative(&path) && path::root_length(&path) == 0 {
        [b"./".as_slice(), &path].concat()
    } else {
        path
    }
}

pub(super) fn same_volume_relative(file: &[u8], directory: &[u8], case_sensitive: bool) -> Vec<u8> {
    let relative =
        path::relative_to_directory_or_url(directory, file, false, directory, case_sensitive);
    if path::encoded_root_length(&relative) > 0 {
        vec![]
    } else {
        relative
    }
}

impl Generation<'_> {
    // port: tsc/internal/modulespecifiers/specifiers.go:processEnding
    pub(super) fn process_ending(&self, file: &[u8], endings: &[Ending]) -> Result<Vec<u8>, Error> {
        if has_extension(file, &[b".json", b".mjs", b".cjs"]) {
            return Ok(file.to_vec());
        }
        let base = path::remove_file_extension(file);
        if file == base {
            return Ok(file.to_vec());
        }
        let priority = |ending| {
            endings
                .iter()
                .position(|e| *e == ending)
                .map_or(-1, |n| n as isize)
        };
        let js_priority = priority(Ending::Js);
        let ts_priority = priority(Ending::Ts);
        if has_extension(file, &[b".mts", b".cts"])
            && ts_priority != -1
            && ts_priority < js_priority
        {
            return Ok(file.to_vec());
        }
        if has_extension(file, &[b".d.mts", b".d.cts", b".mts", b".cts"]) {
            return Ok([
                base,
                js_extension(file, self.options())
                    .ok_or(Error::MissingLink("specifier JS extension"))?,
            ]
            .concat());
        }
        if !has_extension(file, &[b".d.ts"])
            && has_extension(file, &[b".ts"])
            && file.windows(3).any(|s| s == b".d.")
        {
            let name = path::base_name(file);
            if name.windows(3).any(|s| s == b".d.") {
                let without_ts = file.strip_suffix(b".ts").expect("TS file suffix");
                let last_dot = without_ts
                    .iter()
                    .rposition(|c| *c == b'.')
                    .expect("arbitrary extension");
                let first_marker = without_ts
                    .windows(3)
                    .position(|s| s == b".d.")
                    .expect("arbitrary declaration marker");
                return Ok([&without_ts[..first_marker], &without_ts[last_dot..]].concat());
            }
        }
        Ok(match endings[0] {
            Ending::Minimal => {
                let without_index = base.strip_suffix(b"/index").unwrap_or(base);
                if without_index != base && self.any_file_at(without_index)? {
                    base.to_vec()
                } else {
                    without_index.to_vec()
                }
            }
            Ending::Index => base.to_vec(),
            Ending::Js => [
                base,
                js_extension(file, self.options())
                    .ok_or(Error::MissingLink("specifier JS extension"))?,
            ]
            .concat(),
            Ending::Ts if path::is_declaration_file_name(file) => {
                let extensionless = endings
                    .iter()
                    .position(|e| matches!(e, Ending::Minimal | Ending::Index))
                    .map_or(-1, |n| n as isize);
                if extensionless != -1 && extensionless < js_priority {
                    base.to_vec()
                } else {
                    [
                        base,
                        js_extension(file, self.options())
                            .ok_or(Error::MissingLink("specifier declaration extension"))?,
                    ]
                    .concat()
                }
            }
            Ending::Ts => file.to_vec(),
        })
    }

    // port: tsc/internal/modulespecifiers/util.go:tryGetAnyFileFromPath
    fn any_file_at(&self, base: &[u8]) -> Result<bool, Error> {
        for extension in [
            b".ts".as_slice(),
            b".tsx",
            b".d.ts",
            b".js",
            b".jsx",
            b".cts",
            b".d.cts",
            b".cjs",
            b".mts",
            b".d.mts",
            b".mjs",
            b".node",
            b".json",
        ] {
            let file = path::absolute(
                &[base, extension].concat(),
                self.host.get_current_directory(),
            );
            if self.host.file_exists(&file)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/modulespecifiers/specifiers.go:tryGetModuleNameFromRootDirs
    #[allow(
        clippy::naive_bytecount,
        reason = "Count separators in module paths with the standard library; a byte-counting dependency is not justified here"
    )]
    pub(super) fn root_dirs_path(
        &self,
        target: &[u8],
        source_directory: &[u8],
        endings: &[Ending],
    ) -> Result<Vec<u8>, Error> {
        let options = self.options();
        let Some(roots) = &options.root_dirs else {
            return Ok(vec![]);
        };
        let relative_paths = |file: &[u8]| {
            roots
                .iter()
                .map(|root| same_volume_relative(file, root.as_bytes(), self.case_sensitive()))
                .filter(|relative| !relative.starts_with(b".."))
                .collect::<Vec<_>>()
        };
        let targets = relative_paths(target);
        let mut shortest = vec![];
        let mut separators = 0;
        for source in relative_paths(source_directory) {
            for target in &targets {
                let candidate = ensure_non_module(path::relative_from_directory(
                    &source,
                    target,
                    self.host.get_current_directory(),
                    self.case_sensitive(),
                ));
                let count = candidate.iter().filter(|b| **b == b'/').count();
                if shortest.is_empty() || count < separators {
                    shortest = candidate;
                    separators = count;
                }
            }
        }
        if shortest.is_empty() {
            Ok(shortest)
        } else {
            self.process_ending(&shortest, endings)
        }
    }

    // port: tsc/internal/modulespecifiers/specifiers.go:tryGetModuleNameFromPaths
    pub(super) fn module_name_from_paths(
        &self,
        relative: &[u8],
        mappings: &tsr_core::PathMappings,
        endings: &[Ending],
        base: &[u8],
    ) -> Result<Vec<u8>, Error> {
        for (key, values) in mappings {
            for pattern in values.iter().flatten() {
                let normalized = path::normalize(pattern.as_bytes());
                let relative_pattern =
                    same_volume_relative(&normalized, base, self.case_sensitive());
                let pattern = if relative_pattern.is_empty() {
                    normalized.as_ref()
                } else {
                    &relative_pattern
                };
                let mut candidates = vec![];
                for ending in endings {
                    candidates.push((*ending, self.process_ending(relative, &[*ending])?));
                }
                if path::remove_file_extension(pattern) != pattern {
                    candidates.push((Ending::Js, relative.to_vec()));
                }
                if let Some(star) = pattern.iter().position(|b| *b == b'*') {
                    let (prefix, suffix) = (&pattern[..star], &pattern[star + 1..]);
                    for (ending, value) in candidates {
                        if value.len() < prefix.len() + suffix.len()
                            || !self.prefix(&value, prefix)
                            || !self.suffix(&value, suffix)
                        {
                            continue;
                        }
                        if ending == Ending::Minimal
                            && value != self.process_ending(relative, &[ending])?
                        {
                            continue;
                        }
                        let matched = &value[prefix.len()..value.len() - suffix.len()];
                        if !path::is_relative(matched) {
                            return Ok(replace_star(key.as_bytes(), matched));
                        }
                    }
                } else {
                    for (ending, value) in candidates {
                        if pattern == value
                            && (ending != Ending::Minimal
                                || value == self.process_ending(relative, &[ending])?)
                        {
                            return Ok(key.as_bytes().to_vec());
                        }
                    }
                }
            }
        }
        Ok(vec![])
    }
    pub(super) fn prefix(&self, value: &[u8], prefix: &[u8]) -> bool {
        if self.case_sensitive() {
            value.starts_with(prefix)
        } else {
            value
                .get(..prefix.len())
                .is_some_and(|part| path::equal_fold(part, prefix))
        }
    }
    pub(super) fn suffix(&self, value: &[u8], suffix: &[u8]) -> bool {
        if self.case_sensitive() {
            value.ends_with(suffix)
        } else {
            value
                .len()
                .checked_sub(suffix.len())
                .is_some_and(|start| path::equal_fold(&value[start..], suffix))
        }
    }
}

pub(super) fn replace_star(pattern: &[u8], value: &[u8]) -> Vec<u8> {
    if let Some(star) = pattern.iter().position(|b| *b == b'*') {
        [&pattern[..star], value, &pattern[star + 1..]].concat()
    } else {
        pattern.to_vec()
    }
}
