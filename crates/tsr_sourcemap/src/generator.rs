use hashbrown::HashMap;
use tsr_arena::hash::FastState;
use tsr_json::{Decode, Decoder, Encode, Encoder, Kind, Token};
use tsr_jsstring::JsString;

use crate::{base64, Error, Utf16Offset};

/// The pinned `SourceIndex`, a Go `int`.
pub type SourceIndex = isize;
/// The pinned `NameIndex`, a Go `int`.
pub type NameIndex = isize;

const SOURCE_INDEX_NOT_SET: SourceIndex = -1;
const NAME_INDEX_NOT_SET: NameIndex = -1;
const NOT_SET: isize = -1;
const NOT_SET_UTF16: Utf16Offset = -1;

/// Builds a source map's `mappings` incrementally. Mappings must arrive in
/// generated order; one pending mapping is held so later mappings at the same
/// generated position can replace it.
///
/// The pinned `tspath.ComparePathsOptions` is held as its two fields,
/// `current_directory` and `use_case_sensitive_file_names`.
#[derive(Clone, Debug)]
pub struct Generator {
    current_directory: JsString,
    use_case_sensitive_file_names: bool,
    file: JsString,
    source_root: JsString,
    sources_directory_path: JsString,
    raw_sources: Vec<JsString>,
    sources: Vec<JsString>,
    source_to_source_index_map: HashMap<JsString, SourceIndex, FastState>,
    /// Go's `[]*string`: `None` is a nil entry. Empty is Go's nil slice, the
    /// only empty state the pinned code can reach.
    sources_content: Vec<Option<JsString>>,
    names: Vec<JsString>,
    name_to_name_index_map: HashMap<JsString, NameIndex, FastState>,
    mappings: Vec<u8>,
    last_generated_line: isize,
    last_generated_character: Utf16Offset,
    last_source_index: SourceIndex,
    last_source_line: isize,
    last_source_character: Utf16Offset,
    last_name_index: NameIndex,
    has_last: bool,
    pending_generated_line: isize,
    pending_generated_character: Utf16Offset,
    pending_source_index: SourceIndex,
    pending_source_line: isize,
    pending_source_character: Utf16Offset,
    pending_name_index: NameIndex,
    has_pending: bool,
    has_pending_source: bool,
    has_pending_name: bool,
}

/// The pinned `RawSourceMap`. Its JSON form follows the Go struct: members in
/// declaration order, `sourcesContent` omitted when nil (`omitzero`), and a
/// nil `sources` or `names` written as `[]` (json v2's default for nil slices).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawSourceMap {
    pub version: isize,
    pub file: JsString,
    pub source_root: JsString,
    pub sources: Vec<JsString>,
    pub names: Vec<JsString>,
    pub mappings: JsString,
    /// `None` is Go's nil slice (omitted); an entry's `None` is a nil `*string`.
    pub sources_content: Option<Vec<Option<JsString>>>,
}

impl Encode for RawSourceMap {
    fn type_name(&self) -> &'static str {
        "sourcemap.RawSourceMap"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), tsr_json::Error> {
        out.write_token(Token::BeginObject)?;
        out.string(b"version")?;
        out.value(&(self.version as i64))?;
        out.string(b"file")?;
        out.value(&self.file)?;
        out.string(b"sourceRoot")?;
        out.value(&self.source_root)?;
        out.string(b"sources")?;
        out.value(&self.sources)?;
        out.string(b"names")?;
        out.value(&self.names)?;
        out.string(b"mappings")?;
        out.value(&self.mappings)?;
        if let Some(sources_content) = &self.sources_content {
            out.string(b"sourcesContent")?;
            out.value(sources_content)?;
        }
        out.write_token(Token::EndObject)
    }
}

impl Decode for RawSourceMap {
    fn type_name() -> &'static str {
        "sourcemap.RawSourceMap"
    }
    /// json v2's struct decoding: case-sensitive member names, unknown members
    /// skipped, and `null` leaving the destination as it is.
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), tsr_json::Error> {
        if input.peek_kind() == Kind::Null {
            input.read_token()?;
            return Ok(());
        }
        input.object(|name, input| match name {
            b"version" => input.value(&mut self.version),
            b"file" => input.value(&mut self.file),
            b"sourceRoot" => input.value(&mut self.source_root),
            b"sources" => input.value(&mut self.sources),
            b"names" => input.value(&mut self.names),
            b"mappings" => input.value(&mut self.mappings),
            b"sourcesContent" => input.value(&mut self.sources_content),
            _ => input.skip_value(),
        })
    }
}

// port: tsc/internal/sourcemap/generator.go:NewGenerator
pub fn new_generator(
    file: JsString,
    source_root: JsString,
    sources_directory_path: JsString,
    current_directory: JsString,
    use_case_sensitive_file_names: bool,
) -> Generator {
    Generator {
        current_directory,
        use_case_sensitive_file_names,
        file,
        source_root,
        sources_directory_path,
        raw_sources: Vec::new(),
        sources: Vec::new(),
        source_to_source_index_map: HashMap::default(),
        sources_content: Vec::new(),
        names: Vec::new(),
        name_to_name_index_map: HashMap::default(),
        mappings: Vec::new(),
        last_generated_line: 0,
        last_generated_character: 0,
        last_source_index: 0,
        last_source_line: 0,
        last_source_character: 0,
        last_name_index: 0,
        has_last: false,
        pending_generated_line: 0,
        pending_generated_character: 0,
        pending_source_index: 0,
        pending_source_line: 0,
        pending_source_character: 0,
        pending_name_index: 0,
        has_pending: false,
        has_pending_source: false,
        has_pending_name: false,
    }
}

impl Generator {
    /// The file names as added, not the relative `sources` the map records.
    // port: tsc/internal/sourcemap/generator.go:Generator.Sources
    #[allow(clippy::misnamed_getters)] // the pinned getter returns rawSources
    pub fn sources(&self) -> &[JsString] {
        &self.raw_sources
    }

    /// Adds a source to the source map.
    // port: tsc/internal/sourcemap/generator.go:Generator.AddSource
    pub fn add_source(&mut self, file_name: JsString) -> SourceIndex {
        let source = JsString::from_bytes(tsr_tspath::relative_to_directory_or_url(
            self.sources_directory_path.as_bytes(),
            file_name.as_bytes(),
            true, /*isAbsolutePathAnUrl*/
            self.current_directory.as_bytes(),
            self.use_case_sensitive_file_names,
        ));

        if let Some(&source_index) = self.source_to_source_index_map.get(&source) {
            return source_index;
        }
        let source_index = self.sources.len() as SourceIndex;
        self.sources.push(source.clone());
        self.raw_sources.push(file_name);
        self.source_to_source_index_map.insert(source, source_index);
        source_index
    }

    /// Sets the content for a source.
    // port: tsc/internal/sourcemap/generator.go:Generator.SetSourceContent
    pub fn set_source_content(
        &mut self,
        source_index: SourceIndex,
        content: JsString,
    ) -> Result<(), Error> {
        if source_index < 0 || source_index >= self.sources.len() as isize {
            return Err(Error::new("sourceIndex is out of range"));
        }
        while self.sources_content.len() as isize <= source_index {
            self.sources_content.push(None);
        }
        self.sources_content[source_index as usize] = Some(content);
        Ok(())
    }

    /// Declares a name in the source map, returning the index of the name.
    // port: tsc/internal/sourcemap/generator.go:Generator.AddName
    pub fn add_name(&mut self, name: JsString) -> NameIndex {
        if let Some(&name_index) = self.name_to_name_index_map.get(&name) {
            return name_index;
        }
        let name_index = self.names.len() as NameIndex;
        self.names.push(name.clone());
        self.name_to_name_index_map.insert(name, name_index);
        name_index
    }

    // port: tsc/internal/sourcemap/generator.go:Generator.isNewGeneratedPosition
    fn is_new_generated_position(
        &self,
        generated_line: isize,
        generated_character: Utf16Offset,
    ) -> bool {
        !self.has_pending
            || self.pending_generated_line != generated_line
            || self.pending_generated_character != generated_character
    }

    // port: tsc/internal/sourcemap/generator.go:Generator.isBacktrackingSourcePosition
    fn is_backtracking_source_position(
        &self,
        source_index: SourceIndex,
        source_line: isize,
        source_character: Utf16Offset,
    ) -> bool {
        source_index != SOURCE_INDEX_NOT_SET
            && source_line != NOT_SET
            && source_character != NOT_SET_UTF16
            && self.pending_source_index == source_index
            && (self.pending_source_line > source_line
                || self.pending_source_line == source_line
                    && self.pending_source_character > source_character)
    }

    // port: tsc/internal/sourcemap/generator.go:Generator.shouldCommitMapping
    fn should_commit_mapping(&self) -> bool {
        self.has_pending
            && (!self.has_last
                || self.last_generated_line != self.pending_generated_line
                || self.last_generated_character != self.pending_generated_character
                || self.last_source_index != self.pending_source_index
                || self.last_source_line != self.pending_source_line
                || self.last_source_character != self.pending_source_character
                || self.last_name_index != self.pending_name_index)
    }

    /// Every character the generator writes is ASCII, so Go's `WriteRune`
    /// appends exactly this byte.
    // port: tsc/internal/sourcemap/generator.go:Generator.appendMappingCharCode
    fn append_mapping_char_code(&mut self, char_code: u8) {
        self.mappings.push(char_code);
    }

    // port: tsc/internal/sourcemap/generator.go:Generator.appendBase64VLQ
    fn append_base64_vlq(&mut self, mut in_value: isize) {
        // Add a new least significant bit that has the sign of the value.
        // if negative number the least significant bit that gets added to the number has value 1
        // else least significant bit value that gets added is 0
        // eg. -1 changes to binary : 01 [1] => 3
        //     +1 changes to binary : 01 [0] => 2
        if in_value < 0 {
            in_value = in_value.wrapping_neg().wrapping_shl(1).wrapping_add(1);
        } else {
            in_value = in_value.wrapping_shl(1);
        }

        // Encode 5 bits at a time starting from least significant bits
        loop {
            let mut current_digit = in_value & 31; // 11111
            in_value >>= 5;
            if in_value > 0 {
                // There are still more digits to decode, set the msb (6th bit)
                current_digit |= 32;
            }
            self.append_mapping_char_code(base64_format_encode(current_digit));
            if in_value <= 0 {
                break;
            }
        }
    }

    // port: tsc/internal/sourcemap/generator.go:Generator.commitPendingMapping
    fn commit_pending_mapping(&mut self) {
        if !self.should_commit_mapping() {
            return;
        }

        // Line/Comma delimiters
        if self.last_generated_line < self.pending_generated_line {
            // Emit line delimiters
            loop {
                self.append_mapping_char_code(b';');
                self.last_generated_line = self.last_generated_line.wrapping_add(1);
                if self.last_generated_line >= self.pending_generated_line {
                    break;
                }
            }
            // Only need to set this once
            self.last_generated_character = 0;
        } else {
            // panic rather than error as an invariant has been violated
            assert!(
                self.last_generated_line == self.pending_generated_line,
                "generatedLine cannot backtrack"
            );
            // Emit comma to separate the entry
            if self.has_last {
                self.append_mapping_char_code(b',');
            }
        }

        // 1. Relative generated character
        self.append_base64_vlq(
            self.pending_generated_character
                .wrapping_sub(self.last_generated_character),
        );
        self.last_generated_character = self.pending_generated_character;

        if self.has_pending_source {
            // 2. Relative sourceIndex
            self.append_base64_vlq(
                self.pending_source_index
                    .wrapping_sub(self.last_source_index),
            );
            self.last_source_index = self.pending_source_index;

            // 3. Relative source line
            self.append_base64_vlq(self.pending_source_line.wrapping_sub(self.last_source_line));
            self.last_source_line = self.pending_source_line;

            // 4. Relative source character
            self.append_base64_vlq(
                self.pending_source_character
                    .wrapping_sub(self.last_source_character),
            );
            self.last_source_character = self.pending_source_character;

            if self.has_pending_name {
                // 5. Relative nameIndex
                self.append_base64_vlq(self.pending_name_index.wrapping_sub(self.last_name_index));
                self.last_name_index = self.pending_name_index;
            }
        }

        self.has_last = true;
    }

    // port: tsc/internal/sourcemap/generator.go:Generator.addMapping
    fn add_mapping(
        &mut self,
        generated_line: isize,
        generated_character: Utf16Offset,
        source_index: SourceIndex,
        source_line: isize,
        source_character: Utf16Offset,
        name_index: NameIndex,
    ) {
        if self.is_new_generated_position(generated_line, generated_character)
            || self.is_backtracking_source_position(source_index, source_line, source_character)
        {
            self.commit_pending_mapping();
            self.pending_generated_line = generated_line;
            self.pending_generated_character = generated_character;
            self.has_pending_source = false;
            self.has_pending_name = false;
            self.has_pending = true;
        }

        if source_index != SOURCE_INDEX_NOT_SET
            && source_line != NOT_SET
            && source_character != NOT_SET_UTF16
        {
            self.pending_source_index = source_index;
            self.pending_source_line = source_line;
            self.pending_source_character = source_character;
            self.has_pending_source = true;
            if name_index != NAME_INDEX_NOT_SET {
                self.pending_name_index = name_index;
                self.has_pending_name = true;
            }
        }
    }

    /// Adds a mapping without source information.
    // port: tsc/internal/sourcemap/generator.go:Generator.AddGeneratedMapping
    pub fn add_generated_mapping(
        &mut self,
        generated_line: isize,
        generated_character: Utf16Offset,
    ) -> Result<(), Error> {
        if generated_line < self.pending_generated_line {
            return Err(Error::new("generatedLine cannot backtrack"));
        }
        if generated_character < 0 {
            return Err(Error::new("generatedCharacter cannot be negative"));
        }
        self.add_mapping(
            generated_line,
            generated_character,
            SOURCE_INDEX_NOT_SET,
            NOT_SET,       /*sourceLine*/
            NOT_SET_UTF16, /*sourceCharacter*/
            NAME_INDEX_NOT_SET,
        );
        self.has_pending_source = false;
        self.has_pending_name = false;
        Ok(())
    }

    /// Adds a mapping with source information.
    // port: tsc/internal/sourcemap/generator.go:Generator.AddSourceMapping
    pub fn add_source_mapping(
        &mut self,
        generated_line: isize,
        generated_character: Utf16Offset,
        source_index: SourceIndex,
        source_line: isize,
        source_character: Utf16Offset,
    ) -> Result<(), Error> {
        if generated_line < self.pending_generated_line {
            return Err(Error::new("generatedLine cannot backtrack"));
        }
        if generated_character < 0 {
            return Err(Error::new("generatedCharacter cannot be negative"));
        }
        if source_index < 0 || source_index >= self.sources.len() as isize {
            return Err(Error::new("sourceIndex is out of range"));
        }
        if source_line < 0 {
            return Err(Error::new("sourceLine cannot be negative"));
        }
        if source_character < 0 {
            return Err(Error::new("sourceCharacter cannot be negative"));
        }
        if self.has_pending
            && !self.is_new_generated_position(generated_line, generated_character)
            && !self.has_pending_source
        {
            return Ok(());
        }
        self.add_mapping(
            generated_line,
            generated_character,
            source_index,
            source_line,
            source_character,
            NAME_INDEX_NOT_SET,
        );
        Ok(())
    }

    /// Adds a mapping with source and name information.
    // port: tsc/internal/sourcemap/generator.go:Generator.AddNamedSourceMapping
    pub fn add_named_source_mapping(
        &mut self,
        generated_line: isize,
        generated_character: Utf16Offset,
        source_index: SourceIndex,
        source_line: isize,
        source_character: Utf16Offset,
        name_index: NameIndex,
    ) -> Result<(), Error> {
        if generated_line < self.pending_generated_line {
            return Err(Error::new("generatedLine cannot backtrack"));
        }
        if generated_character < 0 {
            return Err(Error::new("generatedCharacter cannot be negative"));
        }
        if source_index < 0 || source_index >= self.sources.len() as isize {
            return Err(Error::new("sourceIndex is out of range"));
        }
        if source_line < 0 {
            return Err(Error::new("sourceLine cannot be negative"));
        }
        if source_character < 0 {
            return Err(Error::new("sourceCharacter cannot be negative"));
        }
        if name_index < 0 || name_index >= self.names.len() as isize {
            return Err(Error::new("nameIndex is out of range"));
        }
        if self.has_pending
            && !self.is_new_generated_position(generated_line, generated_character)
            && !self.has_pending_source
        {
            return Ok(());
        }
        self.add_mapping(
            generated_line,
            generated_character,
            source_index,
            source_line,
            source_character,
            name_index,
        );
        Ok(())
    }

    /// Gets the source map as a `RawSourceMap` object. This commits the
    /// pending mapping, as the pinned method does.
    // port: tsc/internal/sourcemap/generator.go:Generator.RawSourceMap
    pub fn raw_source_map(&mut self) -> RawSourceMap {
        self.commit_pending_mapping();
        RawSourceMap {
            version: 3,
            file: self.file.clone(),
            source_root: self.source_root.clone(),
            sources: self.sources.clone(),
            names: self.names.clone(),
            mappings: JsString::from_bytes(self.mappings.as_slice()),
            sources_content: (!self.sources_content.is_empty())
                .then(|| self.sources_content.clone()),
        }
    }

    // port: tsc/internal/sourcemap/generator.go:Generator.bytes
    fn bytes(&mut self) -> Vec<u8> {
        match tsr_json::marshal(&self.raw_source_map(), tsr_json::Options::default()) {
            Ok(buf) => buf,
            Err(err) => panic!("{err}"),
        }
    }

    /// Gets the string representation of the source map.
    // port: tsc/internal/sourcemap/generator.go:Generator.String
    pub fn string(&mut self) -> JsString {
        JsString::from_bytes(self.bytes())
    }

    // port: tsc/internal/sourcemap/generator.go:Generator.Base64DataURL
    pub fn base64_data_url(&mut self) -> JsString {
        const PREFIX: &[u8] = b"data:application/json;base64,";
        let data = self.bytes();
        let mut sb = Vec::with_capacity(PREFIX.len() + data.len().div_ceil(3) * 4);
        sb.extend_from_slice(PREFIX);
        base64::std_encode(&data, &mut sb);
        JsString::from_bytes(sb)
    }
}

// port: tsc/internal/sourcemap/generator.go:base64FormatEncode
fn base64_format_encode(value: isize) -> u8 {
    match value {
        0..=25 => b'A' + value as u8,
        26..=51 => b'a' + value as u8 - 26,
        52..=61 => b'0' + value as u8 - 52,
        62 => b'+',
        63 => b'/',
        _ => panic!("not a base64 value"),
    }
}
