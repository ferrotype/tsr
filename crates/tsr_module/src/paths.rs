//! Option path patterns and virtual root-directory resolution.
use crate::resolver::{extension, is_relative, Error, ResolvedModule, Resolver};
use crate::trace::trace;
use tsr_core::pattern::Pattern;
use tsr_diagnostics as diagnostics;
use tsr_jsstring::JsString;
use tsr_tspath as path;
impl Resolver {
    // resolver.go:tryLoadModuleUsingOptionalResolutionSettings. This pin has no
    // standalone baseUrl lookup; GetPathsBasePath only uses pathsBasePath/cwd.
    pub(super) fn optional_paths(
        &mut self,
        name: &[u8],
        directory: &[u8],
        extensions: u8,
        esm: bool,
    ) -> Result<Option<ResolvedModule>, Error> {
        if !path::is_relative(name)
            && self
                .options
                .paths
                .as_ref()
                .is_some_and(|p| !p.read().is_empty())
        {
            trace!(self,diagnostics::X_paths_option_is_specified_looking_for_a_pattern_to_match_module_name_0,name);
            // Retain only while the recursive loader borrows self mutably; the
            // option vectors remain borrowed and are never copied per candidate.
            let options = self.options.clone();
            if let Some(result) = self.paths(
                name,
                options
                    .paths_base_path(self.cwd.as_bytes())
                    .to_vec()
                    .as_slice(),
                &options.paths.as_ref().unwrap().read(),
                extensions,
                esm,
            )? {
                return Ok(Some(result));
            };
        }
        if is_relative(name)
            && self
                .options
                .root_dirs
                .as_ref()
                .is_some_and(|p| !p.is_empty())
        {
            return self.root_dirs(name, directory, extensions, esm);
        }
        Ok(None)
    }
    pub(super) fn paths(
        &mut self,
        name: &[u8],
        base: &[u8],
        paths: &tsr_core::PathMappings,
        extensions: u8,
        esm: bool,
    ) -> Result<Option<ResolvedModule>, Error> {
        self.paths_using(
            name,
            base,
            paths,
            extensions,
            |resolver, ext, candidate, from_config| {
                resolver.relative(ext, candidate, esm, true, from_config)
            },
        )
    }
    pub(super) fn paths_using(
        &mut self,
        name: &[u8],
        base: &[u8],
        paths: &tsr_core::PathMappings,
        extensions: u8,
        mut loader: impl FnMut(&mut Self, u8, &[u8], bool) -> Result<Option<ResolvedModule>, Error>,
    ) -> Result<Option<ResolvedModule>, Error> {
        let mut best = None;
        let mut longest = -1;
        for (key, substitutions) in paths {
            let pattern = Pattern::parse(key.as_bytes());
            if !pattern.is_valid() {
                continue;
            }
            if pattern.star_index == -1 && pattern.matches(name) {
                best = Some((substitutions, pattern));
                break;
            }
            if pattern.star_index > longest && pattern.matches(name) {
                longest = pattern.star_index;
                best = Some((substitutions, pattern));
            }
        }
        let Some((substitutions, pattern)) = best else {
            return Ok(None);
        };
        trace!(
            self,
            diagnostics::Module_name_0_matched_pattern_1,
            name,
            pattern.text.as_ref()
        );
        let matched = pattern.matched_text(name);
        for substitution in substitutions.iter().flatten() {
            let replacement = replace_first(substitution.as_bytes(), matched);
            let candidate = path::resolve(base, &[&replacement]);
            trace!(
                self,
                diagnostics::Trying_substitution_0_candidate_module_location_Colon_1,
                &substitution,
                &replacement
            );
            let ext = extension(substitution.as_bytes());
            if !ext.is_empty() {
                if let Some(filename) = self.lookup(&candidate)? {
                    return Ok(Some(ResolvedModule {
                        resolved_file_name: filename,
                        extension: JsString::from_bytes(ext),
                        ..Default::default()
                    }));
                }
            }
            if let Some(result) = loader(self, extensions, &candidate, !ext.is_empty())? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }
    pub(super) fn root_dirs(
        &mut self,
        name: &[u8],
        directory: &[u8],
        extensions: u8,
        esm: bool,
    ) -> Result<Option<ResolvedModule>, Error> {
        trace!(
            self,
            diagnostics::X_rootDirs_option_is_set_using_it_to_resolve_relative_module_name_0,
            name
        );
        let options = self.options.clone();
        let roots = options.root_dirs.as_ref().unwrap();
        let candidate = path::resolve(directory, &[name]);
        let mut matched = None;
        let mut length = 0;
        for (i, root) in roots.iter().enumerate() {
            let mut normalized = path::normalize(root.as_bytes()).into_owned();
            if !normalized.ends_with(b"/") {
                normalized.push(b'/');
            }
            let is_longest = candidate.starts_with(&normalized) && normalized.len() > length;
            trace!(
                self,
                diagnostics::Checking_if_0_is_the_longest_matching_prefix_for_1_2,
                &normalized,
                &candidate,
                is_longest
            );
            if is_longest {
                matched = Some(i);
                length = normalized.len();
            }
        }
        let Some(index) = matched else {
            return Ok(None);
        };
        trace!(
            self,
            diagnostics::Longest_matching_prefix_for_0_is_1,
            &candidate,
            &candidate[..length]
        );
        trace!(
            self,
            diagnostics::Loading_0_from_the_root_dir_1_candidate_location_2,
            &candidate[length..],
            &candidate[..length],
            &candidate
        );
        if let Some(result) = self.relative(extensions, &candidate, esm, true, false)? {
            return Ok(Some(result));
        }
        trace!(self, diagnostics::Trying_other_entries_in_rootDirs);
        let suffix = &candidate[length..];
        for root in roots {
            if Some(root.clone()) == roots.get(index) {
                continue;
            }
            let candidate = path::combine(&path::normalize(root.as_bytes()), &[suffix]);
            trace!(
                self,
                diagnostics::Loading_0_from_the_root_dir_1_candidate_location_2,
                suffix,
                root,
                &candidate
            );
            if let Some(result) = self.relative(extensions, &candidate, esm, true, false)? {
                return Ok(Some(result));
            }
        }
        trace!(
            self,
            diagnostics::Module_resolution_using_rootDirs_has_failed
        );
        Ok(None)
    }
}
fn replace_first(pattern: &[u8], replacement: &[u8]) -> Vec<u8> {
    if let Some(index) = pattern.iter().position(|&b| b == b'*') {
        [&pattern[..index], replacement, &pattern[index + 1..]].concat()
    } else {
        pattern.to_vec()
    }
}
