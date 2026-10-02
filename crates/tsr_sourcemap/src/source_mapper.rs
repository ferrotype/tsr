use std::sync::Arc;

use hashbrown::HashMap;
use tsr_arena::hash::FastState;
use tsr_core::debug::{self, Argument};
use tsr_jsstring::scanner_positions::compute_position_of_line_and_utf16_character;
use tsr_jsstring::{equal_fold, JsString};

use crate::base64;
use crate::decoder::{decode_mappings, MISSING_SOURCE};
use crate::generator::{NameIndex, RawSourceMap, SourceIndex};
use crate::lineinfo::EcmaLineInfo;

/// What the document position mapper reads from its host.
pub trait Host {
    fn use_case_sensitive_file_names(&self) -> bool;
    /// The file's text and line starts, shared with the host's cache (the
    /// pinned host returns a pointer, nil when the file is unknown).
    fn get_ecma_line_info(&self, file_name: &[u8]) -> Option<Arc<EcmaLineInfo>>;
    fn read_file(&self, file_name: &[u8]) -> Option<JsString>;
}

/// Similar to `Mapping`, but position-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MappedPosition {
    generated_position: isize,
    source_position: isize,
    source_index: SourceIndex,
    name_index: NameIndex,
}

const MISSING_POSITION: isize = -1;

impl MappedPosition {
    // port: tsc/internal/sourcemap/source_mapper.go:MappedPosition.isSourceMappedPosition
    fn is_source_mapped_position(&self) -> bool {
        self.source_index != MISSING_SOURCE && self.source_position != MISSING_POSITION
    }
}

type SourceMappedPosition = MappedPosition;

/// Maps source positions to generated positions and vice versa.
///
/// The pinned methods accept a nil receiver and return nil; a caller here
/// holds an `Option<DocumentPositionMapper>` and answers `None` itself.
#[derive(Clone, Debug)]
pub struct DocumentPositionMapper {
    use_case_sensitive_file_names: bool,

    source_file_absolute_paths: Vec<JsString>,
    source_to_source_index_map: HashMap<JsString, SourceIndex, FastState>,
    generated_absolute_file_path: JsString,

    generated_mappings: Vec<MappedPosition>,
    /// Never iterated where the order can reach a result: each list is sorted
    /// on its own, and only its key count is observed (`len` in
    /// [`DocumentPositionMapper::get_generated_position`]).
    source_mappings: HashMap<SourceIndex, Vec<SourceMappedPosition>, FastState>,
}

/// A byte position in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentPosition {
    pub file_name: JsString,
    pub pos: isize,
}

/// Go's `int` comparator result as an ordering.
fn sign(difference: isize) -> std::cmp::Ordering {
    difference.cmp(&0)
}

/// Go's `slices.BinarySearchFunc` position: the first index whose comparison
/// with the target is not negative.
fn binary_search_func(
    x: &[MappedPosition],
    target: isize,
    cmp: impl Fn(&MappedPosition, isize) -> isize,
) -> usize {
    let n = x.len();
    let (mut i, mut j) = (0, n);
    while i < j {
        let h = (i + j) >> 1;
        if cmp(&x[h], target) < 0 {
            i = h + 1;
        } else {
            j = h;
        }
    }
    i
}

// port: tsc/internal/sourcemap/source_mapper.go:createDocumentPositionMapper
fn create_document_position_mapper(
    host: &dyn Host,
    source_map: &RawSourceMap,
    map_path: &[u8],
) -> DocumentPositionMapper {
    let map_directory = tsr_tspath::directory(map_path);
    let source_root = if source_map.source_root.is_empty() {
        map_directory.clone()
    } else {
        tsr_tspath::absolute(source_map.source_root.as_bytes(), &map_directory)
    };
    let generated_absolute_file_path = JsString::from_bytes(tsr_tspath::absolute(
        source_map.file.as_bytes(),
        &map_directory,
    ));
    let source_file_absolute_paths: Vec<JsString> = source_map
        .sources
        .iter()
        .map(|source| JsString::from_bytes(tsr_tspath::absolute(source.as_bytes(), &source_root)))
        .collect();
    let use_case_sensitive_file_names = host.use_case_sensitive_file_names();
    let mut source_to_source_index_map: HashMap<JsString, SourceIndex, FastState> =
        HashMap::with_capacity_and_hasher(source_file_absolute_paths.len(), FastState::default());
    for (i, source) in source_file_absolute_paths.iter().enumerate() {
        source_to_source_index_map.insert(
            JsString::from_bytes(
                tsr_tspath::canonical(source.as_bytes(), use_case_sensitive_file_names)
                    .into_owned(),
            ),
            i as SourceIndex,
        );
    }

    let mut decoded_mappings: Vec<MappedPosition> = Vec::new();
    let mut source_mappings: HashMap<SourceIndex, Vec<SourceMappedPosition>, FastState> =
        HashMap::default();

    // getDecodedMappings()
    let mut decoder = decode_mappings(source_map.mappings.clone());
    for mapping in decoder.values() {
        // processMapping()
        let mut generated_position = -1;
        let line_info = host.get_ecma_line_info(generated_absolute_file_path.as_bytes());
        if let Some(line_info) = line_info {
            generated_position = compute_position_of_line_and_utf16_character(
                &line_info.line_starts,
                mapping.generated_line,
                mapping.generated_character,
                line_info.text.as_bytes(),
                true, /*allowEdits*/
            );
        }

        let mut source_position = -1;
        if mapping.is_source_mapping() {
            let line_info = host.get_ecma_line_info(
                source_file_absolute_paths[mapping.source_index as usize].as_bytes(),
            );
            if let Some(line_info) = line_info {
                let pos = compute_position_of_line_and_utf16_character(
                    &line_info.line_starts,
                    mapping.source_line,
                    mapping.source_character,
                    line_info.text.as_bytes(),
                    true, /*allowEdits*/
                );
                source_position = pos;
            }
        }

        decoded_mappings.push(MappedPosition {
            generated_position,
            source_index: mapping.source_index,
            source_position,
            name_index: mapping.name_index,
        });
    }
    if decoder.error().is_some() {
        decoded_mappings = Vec::new();
    }

    // getSourceMappings()
    for mapping in &decoded_mappings {
        if !mapping.is_source_mapped_position() {
            continue;
        }
        let source_index = mapping.source_index;
        source_mappings
            .entry(source_index)
            .or_default()
            .push(SourceMappedPosition {
                generated_position: mapping.generated_position,
                source_index,
                source_position: mapping.source_position,
                name_index: mapping.name_index,
            });
    }
    for list in source_mappings.values_mut() {
        tsr_core::sort_like_go(list, &mut |a, b| {
            debug::assert(
                a.source_index == b.source_index,
                &[Argument::String(
                    "All source mappings should have the same source index",
                )],
            );
            sign(a.source_position.wrapping_sub(b.source_position))
        });
        // core.DeduplicateSorted
        list.dedup_by(|next, last| {
            last.generated_position == next.generated_position
                && last.source_index == next.source_index
                && last.source_position == next.source_position
        });
    }

    // getGeneratedMappings()
    let mut generated_mappings = decoded_mappings;
    tsr_core::sort_like_go(&mut generated_mappings, &mut |a, b| {
        sign(a.generated_position.wrapping_sub(b.generated_position))
    });
    // core.DeduplicateSorted
    generated_mappings.dedup_by(|next, last| {
        last.generated_position == next.generated_position
            && last.source_index == next.source_index
            && last.source_position == next.source_position
    });

    DocumentPositionMapper {
        use_case_sensitive_file_names,
        source_file_absolute_paths,
        source_to_source_index_map,
        generated_absolute_file_path,
        generated_mappings,
        source_mappings,
    }
}

impl DocumentPositionMapper {
    /// The closest source position at or after `loc` in the generated file.
    // port: tsc/internal/sourcemap/source_mapper.go:DocumentPositionMapper.GetSourcePosition
    pub fn get_source_position(&self, loc: &DocumentPosition) -> Option<DocumentPosition> {
        if self.generated_mappings.is_empty() {
            return None;
        }

        let target_index = binary_search_func(&self.generated_mappings, loc.pos, |m, pos| {
            m.generated_position.wrapping_sub(pos)
        });

        if target_index >= self.generated_mappings.len() {
            return None;
        }

        let mapping = &self.generated_mappings[target_index];
        if !mapping.is_source_mapped_position() {
            return None;
        }

        // Closest position
        Some(DocumentPosition {
            file_name: self.source_file_absolute_paths[mapping.source_index as usize].clone(),
            pos: mapping.source_position,
        })
    }

    /// The closest generated position for a source position at or after `loc`.
    // port: tsc/internal/sourcemap/source_mapper.go:DocumentPositionMapper.GetGeneratedPosition
    pub fn get_generated_position(&self, loc: &DocumentPosition) -> Option<DocumentPosition> {
        let &source_index = self.source_to_source_index_map.get(
            tsr_tspath::canonical(loc.file_name.as_bytes(), self.use_case_sensitive_file_names)
                .as_ref(),
        )?;
        if source_index < 0 || source_index >= self.source_mappings.len() as isize {
            return None;
        }
        let source_mappings = self
            .source_mappings
            .get(&source_index)
            .map_or(&[][..], Vec::as_slice);
        let target_index = binary_search_func(source_mappings, loc.pos, |m, pos| {
            m.source_position.wrapping_sub(pos)
        });

        if target_index >= source_mappings.len() {
            return None;
        }

        let mapping = &source_mappings[target_index];
        if mapping.source_index != source_index {
            return None;
        }

        // Closest position
        Some(DocumentPosition {
            file_name: self.generated_absolute_file_path.clone(),
            pos: mapping.generated_position,
        })
    }
}

/// Finds the generated file's source map (an inline `data:` URL, the
/// `sourceMappingURL` file, or `<file>.map`) and builds its mapper.
// port: tsc/internal/sourcemap/source_mapper.go:GetDocumentPositionMapper
pub fn get_document_position_mapper(
    host: &dyn Host,
    generated_file_name: &[u8],
) -> Option<DocumentPositionMapper> {
    let mut map_file_name = try_get_source_mapping_url(host, generated_file_name);
    if !map_file_name.is_empty() {
        let (base64_object, matched) = try_parse_base64_url(&map_file_name);
        if matched {
            if !base64_object.is_empty() {
                if let Some(decoded) = base64::std_decode_string(base64_object) {
                    return convert_document_to_source_mapper(
                        host,
                        &JsString::from_bytes(decoded),
                        generated_file_name,
                    );
                }
            }
            // Not a data URL we can parse, skip it
            map_file_name = Vec::new();
        }
    }

    let mut possible_map_locations: Vec<Vec<u8>> = Vec::new();
    if !map_file_name.is_empty() {
        possible_map_locations.push(map_file_name);
    }
    possible_map_locations.push([generated_file_name, b".map"].concat());
    for location in &possible_map_locations {
        let map_file_name =
            tsr_tspath::absolute(location, &tsr_tspath::directory(generated_file_name));
        if let Some(map_file_contents) = host.read_file(&map_file_name) {
            return convert_document_to_source_mapper(host, &map_file_contents, &map_file_name);
        }
    }
    None
}

// port: tsc/internal/sourcemap/source_mapper.go:convertDocumentToSourceMapper
fn convert_document_to_source_mapper(
    host: &dyn Host,
    contents: &JsString,
    map_file_name: &[u8],
) -> Option<DocumentPositionMapper> {
    let source_map = try_parse_raw_source_map(contents)?;
    if source_map.sources.is_empty() || source_map.file.is_empty() || source_map.mappings.is_empty()
    {
        // invalid map
        return None;
    }

    // Don't support source maps that contain inlined sources
    if source_map
        .sources_content
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(Option::is_some)
    {
        return None;
    }

    Some(create_document_position_mapper(
        host,
        &source_map,
        map_file_name,
    ))
}

/// The pinned `json.Unmarshal` with its default options: strict UTF-8,
/// case-sensitive member names and duplicate names rejected.
// port: tsc/internal/sourcemap/source_mapper.go:tryParseRawSourceMap
fn try_parse_raw_source_map(contents: &JsString) -> Option<RawSourceMap> {
    let mut source_map = RawSourceMap::default();
    if tsr_json::unmarshal(
        contents.as_bytes(),
        &mut source_map,
        tsr_json::Options::default(),
    )
    .is_err()
    {
        return None;
    }
    if source_map.version != 3 {
        return None;
    }
    Some(source_map)
}

// port: tsc/internal/sourcemap/source_mapper.go:tryGetSourceMappingURL
fn try_get_source_mapping_url(host: &dyn Host, file_name: &[u8]) -> Vec<u8> {
    let line_info = host.get_ecma_line_info(file_name);
    crate::util::try_get_source_mapping_url(line_info.as_deref()).to_vec()
}

/// Equivalent to `/^data:(?:application\/json;(?:charset=[uU][tT][fF]-8;)?base64,([A-Za-z0-9+/=]+)$)?/`.
/// A `charset=` followed by fewer than six bytes panics, as the pinned slice
/// expression does.
// port: tsc/internal/sourcemap/source_mapper.go:tryParseBase64Url
pub(crate) fn try_parse_base64_url(url: &[u8]) -> (&[u8], bool) {
    let Some(mut url) = url.strip_prefix(b"data:") else {
        return (b"", false);
    };
    let Some(rest) = url.strip_prefix(b"application/json;") else {
        return (b"", true);
    };
    url = rest;
    if let Some(rest) = url.strip_prefix(b"charset=") {
        url = rest;
        if !equal_fold(&url[..b"utf-8;".len()], b"utf-8;") {
            return (b"", true);
        }
        url = &url[b"utf-8;".len()..];
    }
    let Some(rest) = url.strip_prefix(b"base64,") else {
        return (b"", true);
    };
    url = rest;
    let mut offset = 0;
    while offset < url.len() {
        let (r, width) = tsr_jsstring::wtf8::decode_utf8(&url[offset..]);
        if !(tsr_jsstring::classify::is_ascii_letter(r)
            || tsr_jsstring::classify::is_digit(r)
            || r == i32::from(b'+')
            || r == i32::from(b'/')
            || r == i32::from(b'='))
        {
            return (b"", true);
        }
        offset += width;
    }
    (url, true)
}
