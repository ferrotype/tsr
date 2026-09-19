//! Pinned vfsmatch patterns. Matching borrows path bytes and performs no path
//! concatenation on directory-entry checks.
use std::collections::BTreeSet;
use ts_jsstring::{equal_fold, helpers::to_lower_go, wtf8::decode_utf8, JsString};
use ts_tspath as path;
use ts_vfs::{Error, FileSystem};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    Files,
    Directories,
    Exclude,
}
pub const UNLIMITED_DEPTH: isize = isize::MAX;

/// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:IsImplicitGlob
pub fn is_implicit_glob(component: &[u8]) -> bool {
    !component.iter().any(|b| matches!(b, b'.' | b'*' | b'?'))
}

#[derive(Clone, Debug)]
enum Segment {
    Literal(Vec<u8>),
    Star,
    Question,
}
#[derive(Clone, Debug)]
enum Component {
    Literal(Vec<u8>),
    Wildcard(Vec<Segment>),
    DoubleAsterisk,
}
#[derive(Clone, Debug)]
struct Pattern {
    components: Vec<Component>,
    usage: Usage,
    case_sensitive: bool,
}
impl Pattern {
    /// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:compileGlobPattern
    fn compile(spec: &[u8], base: &[u8], usage: Usage, case_sensitive: bool) -> Option<Self> {
        let mut parts = path::normalized_components(spec, base);
        if usage != Usage::Exclude && parts.last().is_some_and(|s| s == b"**") {
            return None;
        }
        if parts[0].ends_with(b"/") {
            parts[0].pop();
        }
        if is_implicit_glob(parts.last().map_or(b"", Vec::as_slice)) {
            parts.extend([b"**".to_vec(), b"*".to_vec()]);
        }
        Some(Self {
            components: parts.into_iter().map(parse_component).collect(),
            usage,
            case_sensitive,
        })
    }
    fn strings_equal(&self, a: &[u8], b: &[u8]) -> bool {
        if self.case_sensitive {
            a == b
        } else {
            equal_fold(a, b)
        }
    }
    /// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:globPattern.matchPathParts
    fn matches_parts(
        &self,
        prefix: &[u8],
        suffix: &[u8],
        mut offset: usize,
        mut index: usize,
        prefix_only: bool,
    ) -> bool {
        loop {
            let Some((part, next)) = next_part(prefix, suffix, offset) else {
                return prefix_only
                    || self.components[index..]
                        .iter()
                        .all(|c| matches!(c, Component::DoubleAsterisk));
            };
            let Some(component) = self.components.get(index) else {
                return self.usage == Usage::Exclude && !prefix_only;
            };
            match component {
                Component::DoubleAsterisk => {
                    if self.matches_parts(prefix, suffix, offset, index + 1, prefix_only) {
                        return true;
                    }
                    if self.usage != Usage::Exclude
                        && (part.starts_with(b".") || is_package_folder(part))
                    {
                        return false;
                    }
                    offset = next;
                    continue;
                }
                Component::Literal(literal) => {
                    if !self.strings_equal(literal, part) {
                        return false;
                    }
                }
                Component::Wildcard(segments) => {
                    if self.usage != Usage::Exclude && is_package_folder(part) {
                        return false;
                    }
                    if !self.match_wildcard(segments, part) {
                        return false;
                    }
                }
            }
            offset = next;
            index += 1;
        }
    }
    /// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:globPattern.matchWildcard
    fn match_wildcard(&self, segments: &[Segment], value: &[u8]) -> bool {
        if self.usage != Usage::Exclude
            && value.starts_with(b".")
            && matches!(segments.first(), Some(Segment::Star | Segment::Question))
        {
            return false;
        }
        let matched = if let [Segment::Star, Segment::Literal(suffix)] = segments {
            value.len() >= suffix.len()
                && self.strings_equal(suffix, &value[value.len() - suffix.len()..])
        } else {
            self.match_segments(segments, value)
        };
        matched && self.should_include_min_js(value, segments)
    }
    /// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:globPattern.matchSegments
    fn match_segments(&self, segments: &[Segment], value: &[u8]) -> bool {
        let (mut segment_index, mut value_index) = (0, 0);
        let (mut star_segment, mut star_value) = (None, 0);
        while value_index < value.len() {
            if let Some(segment) = segments.get(segment_index) {
                match segment {
                    Segment::Literal(literal) => {
                        let end = value_index + literal.len();
                        if end <= value.len()
                            && self.strings_equal(literal, &value[value_index..end])
                        {
                            value_index = end;
                            segment_index += 1;
                            continue;
                        }
                    }
                    Segment::Question if value[value_index] != b'/' => {
                        value_index += decode_utf8(&value[value_index..]).1;
                        segment_index += 1;
                        continue;
                    }
                    Segment::Star => {
                        star_segment = Some(segment_index);
                        star_value = value_index;
                        segment_index += 1;
                        continue;
                    }
                    Segment::Question => {}
                }
            }
            if let Some(star) = star_segment {
                if star_value < value.len() && value[star_value] != b'/' {
                    star_value += decode_utf8(&value[star_value..]).1;
                    value_index = star_value;
                    segment_index = star + 1;
                    continue;
                }
            }
            return false;
        }
        while matches!(segments.get(segment_index), Some(Segment::Star)) {
            segment_index += 1;
        }
        segment_index == segments.len()
    }
    /// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:globPattern.shouldIncludeMinJs
    fn should_include_min_js(&self, name: &[u8], segments: &[Segment]) -> bool {
        if self.usage != Usage::Files
            || name.len() < 7
            || !self.strings_equal(&name[name.len() - 7..], b".min.js")
        {
            return true;
        }
        segments.iter().any(|segment| {
            let Segment::Literal(literal) = segment else {
                return false;
            };
            let lower;
            let literal = if self.case_sensitive {
                literal.as_slice()
            } else {
                lower = to_lower_go(literal);
                lower.as_ref()
            };
            literal.windows(5).any(|part| part == b".min.")
        })
    }
}
/// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:parseComponent
fn parse_component(bytes: Vec<u8>) -> Component {
    if bytes == b"**" {
        return Component::DoubleAsterisk;
    }
    if !bytes.iter().any(|b| matches!(b, b'*' | b'?')) {
        return Component::Literal(bytes);
    }
    let mut segments = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(byte, b'*' | b'?') {
            if index > start {
                segments.push(Segment::Literal(bytes[start..index].to_vec()));
            }
            segments.push(if *byte == b'*' {
                Segment::Star
            } else {
                Segment::Question
            });
            start = index + 1;
        }
    }
    if start < bytes.len() {
        segments.push(Segment::Literal(bytes[start..].to_vec()));
    }
    Component::Wildcard(segments)
}
/// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:nextPathPartSingle
fn next_single(value: &[u8], mut offset: usize) -> Option<(&[u8], usize)> {
    if offset >= value.len() {
        return None;
    }
    if offset == 0 && value[0] == b'/' {
        return Some((&value[..0], 1));
    }
    while value.get(offset) == Some(&b'/') {
        offset += 1;
    }
    if offset == value.len() {
        return None;
    }
    let next = value[offset..]
        .iter()
        .position(|b| *b == b'/')
        .map_or(value.len(), |i| offset + i);
    Some((&value[offset..next], next))
}
/// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:nextPathPartParts
fn next_part<'a>(
    prefix: &'a [u8],
    suffix: &'a [u8],
    mut offset: usize,
) -> Option<(&'a [u8], usize)> {
    if suffix.is_empty() {
        return next_single(prefix, offset);
    }
    if prefix.is_empty() {
        return next_single(suffix, offset);
    }
    if offset >= prefix.len() + suffix.len() {
        return None;
    }
    if offset == 0 && prefix[0] == b'/' {
        return Some((&prefix[..0], 1));
    }
    if offset < prefix.len() {
        while prefix.get(offset) == Some(&b'/') {
            offset += 1;
        }
        if offset < prefix.len() {
            let index = prefix[offset..]
                .iter()
                .position(|b| *b == b'/')
                .expect("directory prefix ends in slash");
            return Some((&prefix[offset..offset + index], offset + index));
        }
    }
    Some((
        &suffix[offset - prefix.len()..],
        prefix.len() + suffix.len(),
    ))
}
/// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:isPackageFolder
fn is_package_folder(name: &[u8]) -> bool {
    [
        b"node_modules".as_slice(),
        b"jspm_packages",
        b"bower_components",
    ]
    .iter()
    .any(|folder| name.len() == folder.len() && equal_fold(name, folder))
}

#[derive(Clone, Debug)]
pub struct SpecMatcher {
    patterns: Vec<Pattern>,
}
impl SpecMatcher {
    /// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:NewSpecMatcher
    pub fn new(
        specs: &[JsString],
        base: &[u8],
        usage: Usage,
        case_sensitive: bool,
    ) -> Option<Self> {
        let patterns: Vec<_> = specs
            .iter()
            .filter_map(|s| Pattern::compile(s.as_bytes(), base, usage, case_sensitive))
            .collect();
        (!patterns.is_empty()).then_some(Self { patterns })
    }
    /// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:SpecMatcher.MatchString
    pub fn matches(&self, value: &[u8]) -> bool {
        self.match_index(value).is_some()
    }
    /// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:SpecMatcher.MatchIndex
    pub fn match_index(&self, value: &[u8]) -> Option<usize> {
        self.patterns
            .iter()
            .position(|pattern| pattern.matches_parts(value, b"", 0, 0, false))
    }
}
struct Matcher {
    includes: Vec<Pattern>,
    excludes: Vec<Pattern>,
    had_includes: bool,
}
impl Matcher {
    fn new(
        includes: &[JsString],
        excludes: &[JsString],
        base: &[u8],
        case_sensitive: bool,
        usage: Usage,
    ) -> Self {
        Self {
            includes: includes
                .iter()
                .filter_map(|s| Pattern::compile(s.as_bytes(), base, usage, case_sensitive))
                .collect(),
            excludes: excludes
                .iter()
                .filter_map(|s| {
                    Pattern::compile(s.as_bytes(), base, Usage::Exclude, case_sensitive)
                })
                .collect(),
            had_includes: !includes.is_empty(),
        }
    }
    fn matches(&self, prefix: &[u8], suffix: &[u8], directory: bool) -> Option<usize> {
        if self
            .excludes
            .iter()
            .any(|p| p.matches_parts(prefix, suffix, 0, 0, false))
        {
            return None;
        }
        if self.includes.is_empty() {
            return (!self.had_includes).then_some(0);
        }
        self.includes
            .iter()
            .position(|p| p.matches_parts(prefix, suffix, 0, 0, directory))
    }
}
fn trailing_slash(mut path: Vec<u8>) -> Vec<u8> {
    if !path.is_empty() && !path.ends_with(b"/") {
        path.push(b'/');
    }
    path
}
/// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:getIncludeBasePath
fn include_base_path(absolute: &[u8]) -> Vec<u8> {
    let Some(index) = absolute.iter().position(|b| matches!(b, b'*' | b'?')) else {
        if !path::has_extension(absolute) {
            return absolute.to_vec();
        }
        let mut result = path::directory(absolute);
        if result.ends_with(b"/") {
            result.pop();
        }
        return result;
    };
    absolute[..absolute[..index]
        .iter()
        .rposition(|b| *b == b'/')
        .unwrap_or(0)]
        .to_vec()
}
/// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:getBasePaths
fn base_paths(path: &[u8], includes: &[JsString], case_sensitive: bool) -> Vec<Vec<u8>> {
    let mut result = vec![path.to_vec()];
    let mut bases: Vec<_> = includes
        .iter()
        .map(|s| {
            let absolute = if ts_tspath::encoded_root_length(s.as_bytes()) > 0 {
                s.as_bytes().to_vec()
            } else {
                ts_tspath::normalize(&ts_tspath::combine(path, &[s.as_bytes()])).into_owned()
            };
            include_base_path(&absolute)
        })
        .collect();
    bases.sort_by(|a, b| ts_tspath::compare_paths(a, b, path, case_sensitive));
    for base in bases {
        if result
            .iter()
            .all(|existing| !ts_tspath::contains_path(existing, &base, path, case_sensitive))
        {
            result.push(base);
        }
    }
    result
}
struct Visitor<'a> {
    host: &'a dyn FileSystem,
    files: Matcher,
    directories: Matcher,
    extensions: &'a [JsString],
    visited: BTreeSet<JsString>,
    results: Vec<Vec<JsString>>,
}
impl Visitor<'_> {
    /// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:globVisitor.visit
    fn visit(
        &mut self,
        path: &[u8],
        absolute: &[u8],
        mut depth: isize,
        resolved: &[u8],
    ) -> Result<(), Error> {
        let real_path = if resolved.is_empty() {
            self.host.realpath(absolute)?
        } else {
            JsString::from_bytes(resolved)
        };
        let key = if self.host.use_case_sensitive_file_names() {
            real_path.clone()
        } else {
            JsString::from_bytes(path::file_name_lower_case(real_path.as_bytes()).into_owned())
        };
        if !self.visited.insert(key) {
            return Ok(());
        }
        let entries = self.host.entries(absolute)?;
        let path_prefix = trailing_slash(path.to_vec());
        let abs_prefix = trailing_slash(absolute.to_vec());
        for file in entries.files {
            if !self.extensions.is_empty()
                && !self
                    .extensions
                    .iter()
                    .any(|e| file.len() > e.len() && file.as_bytes().ends_with(e.as_bytes()))
            {
                continue;
            }
            if let Some(index) = self.files.matches(&abs_prefix, file.as_bytes(), false) {
                let mut full = path_prefix.clone();
                full.extend_from_slice(file.as_bytes());
                self.results[index].push(JsString::from_bytes(full));
            }
        }
        if depth != UNLIMITED_DEPTH {
            depth = depth.wrapping_sub(1);
            if depth == 0 {
                return Ok(());
            }
        }
        for dir in entries.directories {
            if self
                .directories
                .matches(&abs_prefix, dir.as_bytes(), true)
                .is_none()
            {
                continue;
            }
            let mut abs_dir = abs_prefix.clone();
            abs_dir.extend_from_slice(dir.as_bytes());
            let mut path_dir = path_prefix.clone();
            path_dir.extend_from_slice(dir.as_bytes());
            let child_real = if entries
                .symlinks
                .as_ref()
                .is_some_and(|links| !links.contains(&dir))
            {
                path::combine(real_path.as_bytes(), &[dir.as_bytes()])
            } else {
                Vec::new()
            };
            self.visit(&path_dir, &abs_dir, depth, &child_real)?;
        }
        Ok(())
    }
}
/// port: tsc/internal/vfs/vfsmatch/vfsmatch.go:ReadDirectory
pub fn read_directory(
    host: &dyn FileSystem,
    current_directory: &[u8],
    path: &[u8],
    extensions: &[JsString],
    excludes: &[JsString],
    includes: &[JsString],
    depth: isize,
) -> Result<Vec<JsString>, Error> {
    let path = ts_tspath::normalize(path);
    let current = ts_tspath::normalize(current_directory);
    let absolute = ts_tspath::combine(&current, &[&path]);
    let case_sensitive = host.use_case_sensitive_file_names();
    let files = Matcher::new(includes, excludes, &absolute, case_sensitive, Usage::Files);
    let results = vec![Vec::new(); files.includes.len().max(1)];
    let mut visitor = Visitor {
        host,
        files,
        directories: Matcher::new(
            includes,
            excludes,
            &absolute,
            case_sensitive,
            Usage::Directories,
        ),
        extensions,
        visited: BTreeSet::new(),
        results,
    };
    for base in base_paths(&path, includes, case_sensitive) {
        visitor.visit(&base, &ts_tspath::combine(&current, &[&base]), depth, b"")?;
    }
    Ok(visitor.results.into_iter().flatten().collect())
}
