"""Physical syntax exporter staged into an immutable CP1 source copy only."""
import argparse
import difflib
import hashlib
import json
from pathlib import Path
import re

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
BASE = ROOT / "target/s07-bis/cp1-node-read-candidate/source"
SCHEMA = "crates/tsr_ast/src/data_generated.rs"
SCHEMA_SHA256 = "8563c53e83fe66ea8d7a353102858dabdb5d570556150be53f96ec57f5879428"
MARKER = "// S07-bis access trace: staged physical state observer."
FIELD_OPS = {"Option<NodeId>": 17, "Option<NodeListId>": 18, "bool": 19,
             "i32": 19, "NodeKind": 19, "NodeSlice": 20, "TextSlice": 21,
             "JsString": 22}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def schema(raw):
    structs = {}
    for match in re.finditer(r"pub struct (\w+Data) \{([^}]*)\}", raw):
        name, body = match.groups()
        require(name not in structs, "duplicate payload struct")
        fields = re.findall(r"\s*pub (\S+): ([^\n]+),", body)
        require(not re.sub(r"\s*pub (\S+): ([^\n]+),", "", body).strip(), "unparsed payload field")
        require(len({name for name, _ in fields}) == len(fields), "duplicate payload field")
        require(all(kind in FIELD_OPS for _, kind in fields), "unsupported payload field type")
        structs[name] = fields
    enum = re.search(r"pub enum NodeData \{([^}]+)\}", raw)
    require(enum is not None, "missing NodeData enum")
    variants = re.findall(r"\s*(\w+)\((?:Box<)?(\w+Data)>?\),", enum[1])
    require(len(variants) == 192 and len({name for name, _ in variants}) == 192, "payload shape inventory changed")
    require(set(structs) == {name for _, name in variants}, "payload structs and variants disagree")
    require(not re.sub(r"\s*\w+\((?:Box<)?\w+Data>?\),", "", enum[1]).strip(), "unparsed payload variant")
    result = []
    field_id = 0
    for shape_id, (variant, name) in enumerate(variants, 1):
        fields = []
        for field, kind in structs[name]:
            field_id += 1
            fields.append({"id": field_id, "name": field, "type": kind, "op": FIELD_OPS[kind]})
        result.append({"id": shape_id, "name": variant, "fields": fields})
    return result


def generated(shapes):
    lines = ["// Generated in staging from the exact pinned payload declarations.",
             "fn shape(data: &NodeData) -> u16 {", "    match data {"]
    lines += [f"        NodeData::{item['name']}(_) => {item['id']}," for item in shapes]
    lines += ["    }", "}", "fn export_payload(id: u64, data: &NodeData, summary: &mut StateSummary) {", "    match data {"]
    for item in shapes:
        fields = item["fields"]
        lines.append(f"        NodeData::{item['name']}({'data' if fields else '_'}) => {{")
        for field in fields:
            name, kind, site, op = (field[k] for k in ("name", "type", "id", "op"))
            value = "data." + name
            if kind == "JsString":
                lines.append(f"            bytes({op}, {site}, id, 0, {value}.as_bytes(), summary);")
            elif kind in ("NodeSlice", "TextSlice"):
                lines.append(f"            event({op}, {site}, id, {value}.backing_id().map_or(0, |id| id.bits()), u64::from({value}.start()), count({value}.len()));")
            else:
                if kind in ("Option<NodeId>", "Option<NodeListId>"):
                    value += ".map_or(0, |id| id.bits())"
                elif kind == "NodeKind":
                    value = f"signed(i64::from({value}.raw()))"
                elif kind == "i32":
                    value = f"signed(i64::from({value}))"
                elif kind == "bool":
                    value = f"u64::from({value})"
                lines.append(f"            event({op}, {site}, id, {value}, 0, 0);")
        lines += [f"            summary.payload_fields += {len(fields)};", "        }"]
    return "\n".join(lines + ["    }", "}", ""])


def registry(shapes):
    def event(op, name, fields, sites=None, blob=False):
        value = {"id": op, "name": name, "domain": 1, "blob": blob,
                 "fields": dict(zip(("a", "b", "c", "d"), fields))}
        if sites is not None:
            value["sites"] = [{"id": key, "name": label} for key, label in sites]
        return value
    zero = [(0, "state")]
    arenas = [(1, "core_nodes"), (2, "core_auxiliary")]
    auxiliary = [(1, "List"), (2, "Nodes"), (3, "Text"), (4, "File"), (5, "SourceMetadata"), (6, "SourceFiles")]
    metadata = [(1, "Nodes"), (2, "Text"), (3, "Comments"), (4, "Pragmas"), (5, "References"), (6, "DiagnosticDirectives")]
    source_scalar = [(1, "language_variant"), (2, "script_kind"), (3, "is_declaration_file"),
        (4, "uses_uri_style_node_core_modules"), (5, "identifier_count"), (6, "has_lazy_jsdoc"),
        (7, "node_count"), (8, "text_count"), (9, "common_js_module_indicator"),
        (10, "external_module_indicator"), (11, "hash_hi"), (12, "hash_lo"),
        (13, "module_indicator_jsx"), (14, "module_indicator_force"), (15, "check_js_present")]
    source_slices = [(1, "imports"), (2, "module_augmentations"), (3, "ambient_module_names"),
        (4, "comment_directives"), (5, "pragmas"), (6, "referenced_files"),
        (7, "type_reference_directives"), (8, "lib_reference_directives")]
    events = [
        event(10, "state_owner", ["core_arena", "auxiliary_arena", "metadata_aux_id_or_zero", "source_bytes"], zero),
        event(11, "arena_counts", ["arena", "len", "capacity", "pages"], arenas),
        event(12, "arena_directory_layout", ["arena", "directory_capacity", "element_bytes", "page_descriptor_bytes"], arenas),
        event(13, "arena_page", ["arena", "zero_based_page", "len", "capacity"], arenas),
        event(14, "node_header", ["node_id", "signed_kind_bits", "parent_id_or_zero", "flags"], [(s["id"], s["name"]) for s in shapes]),
        event(15, "node_range", ["node_id", "signed_pos_bits", "signed_end_bits", "unused_zero"], zero),
        event(16, "node_existing_caches", ["node_id", "cached_subtree_facts", "existing_runtime_id", "unused_zero"], zero),
    ]
    for op, name in ((17, "payload_node"), (18, "payload_list"), (19, "payload_scalar"),
                     (20, "payload_node_slice"), (21, "payload_text_slice"), (22, "payload_text_bytes")):
        sites = [(f["id"], s["name"] + "." + f["name"]) for s in shapes for f in s["fields"] if f["op"] == op]
        fields = (["node_id", "backing_aux_id_or_zero", "start", "len"] if op in (20, 21)
                  else ["node_id", "unused_zero", "byte_offset", "logical_blob_bytes"] if op == 22
                  else ["node_id", "value_or_zero", "unused_zero", "unused_zero"])
        events.append(event(op, name, fields, sites, op == 22))
    events += [
        event(23, "owner_source_bytes", ["core_arena", "unused_zero", "byte_offset", "logical_blob_bytes"], zero, True),
        event(24, "auxiliary_header", ["aux_id", "logical_elements", "unused_zero", "unused_zero"], auxiliary),
        event(25, "list_header", ["aux_id", "signed_pos_bits", "signed_end_bits", "modifier_flags"], zero),
        event(26, "list_slice", ["list_aux_id", "backing_aux_id_or_zero", "start", "len"], zero),
        event(27, "backing_node", ["aux_id", "zero_based_index", "node_id_or_zero", "unused_zero"], zero),
        event(28, "backing_text_bytes", ["aux_id", "zero_based_index", "byte_offset", "logical_blob_bytes"], zero, True),
        event(29, "file_frame", ["aux_id", "root_node_id_or_zero", "signed_node_count_bits", "signed_text_count_bits"], zero),
        event(30, "file_source_frames", ["aux_id", "source_files_aux_id_or_zero", "unused_zero", "unused_zero"], zero),
        event(31, "metadata_header", ["aux_id", "len", "unused_zero", "unused_zero"], metadata),
        event(32, "metadata_node", ["aux_id", "zero_based_index", "node_id_or_zero", "unused_zero"], zero),
        event(33, "metadata_text_bytes", ["aux_id", "zero_based_index", "byte_offset", "logical_blob_bytes"], zero, True),
        event(34, "metadata_range", ["aux_id", "zero_based_index", "signed_pos_bits", "signed_end_bits"], [(3, "Comments"), (5, "References")]),
        event(35, "metadata_scalar", ["aux_id", "zero_based_index", "value", "unused_zero"], [(1, "comment_kind"), (2, "reference_resolution_mode"), (3, "reference_preserve")]),
        event(36, "reference_file_name", ["aux_id", "zero_based_index", "byte_offset", "logical_blob_bytes"], zero, True),
        event(37, "metadata_payload_omitted", ["aux_id", "observed_element_count", "unused_zero", "unused_zero"], [(4, "Pragmas"), (6, "DiagnosticDirectives")]),
        event(40, "source_frame", ["aux_id", "source_node_id", "unused_zero", "unused_zero"], zero),
        event(41, "source_scalar", ["source_node_id", "value", "unused_zero", "unused_zero"], source_scalar),
        event(42, "source_bytes", ["source_node_id", "unused_zero", "byte_offset", "logical_blob_bytes"], [(1, "file_name"), (2, "path"), (3, "text")], True),
        event(43, "source_metadata_slice", ["source_node_id", "backing_aux_id_or_zero", "start", "len"], source_slices),
        event(44, "source_vector_node", ["source_node_id", "zero_based_index", "node_id", "unused_zero"], [(1, "reparsed_clones")]),
        event(45, "source_vector_counts", ["source_node_id", "len", "capacity", "unused_zero"], [(1, "reparsed_clones"), (2, "diagnostics_payload_omitted"), (3, "js_diagnostics_payload_omitted"), (4, "jsdoc_diagnostics_payload_omitted")]),
        event(46, "check_js_range", ["source_node_id", "signed_pos_bits", "signed_end_bits", "enabled"], zero),
        event(47, "check_js_metadata", ["source_node_id", "signed_kind_bits", "has_trailing_new_line", "unused_zero"], zero),
        event(48, "source_content_mapper_payload_omitted", ["source_node_id", "present", "unused_zero", "unused_zero"], zero),
        event(74, "arena_element_alignment", ["arena", "element_alignment", "unused_zero", "unused_zero"], arenas),
        event(75, "owner_external_storage_omitted", ["core_arena", "import_roots", "imported_arena_routes", "supplemental_owners"], zero),
        event(76, "lazy_storage_counts", ["reserved_nodes", "reserved_auxiliary", "initialized_nodes", "initialized_auxiliary"], zero),
        event(79, "state_export_end", ["physical_core_nodes", "physical_core_auxiliary", "payload_fields", "selected_bytes_exported"], zero),
    ]
    return {"version": 1, "events": events, "shapes": shapes, "schema_sha256": SCHEMA_SHA256,
        "blob_framing": "Recorder splits at at most64KiB; c=byte offset,d=logical byte length;a/b identity fixed; empty bytes emit one empty record.",
        "signed_encoding": "Signed integers are sign-extended to i64, then represented by their u64 two's-complement bits.",
        "coverage": {"complete": ["Every physical core node in slot allocation order, including unreachable/obsolete nodes", "Node kind independently from payload shape, parent, flags, range, already-cached facts and runtime ID", "All192 payload shapes and every nondeferred stored field, including selected raw JsString bytes", "Every core auxiliary identity and syntax list header/backing entry/range, including nil, allocated empty and missing sentinel", "Source metadata nodes/text/comments/references and exposed source scalar/slice fields", "Core node/auxiliary arena capacities, page occupancies and directory capacities"],
            "omitted": ["Allocator pointer identity, JsString backing sharing/cached validity and source byte Arc sharing", "Lazy record payloads/caches (reserved and initialized counts only), imported/bundle owner contents", "Pragma and mapped-diagnostic-directive payloads (variant and element counts recorded)", "Source diagnostics payloads, content mapper contents and derived source caches (vector counts/presence recorded where exposed)", "Parse-time allocation/mutation order before this endpoint and complete initial-state replay"],
            "side_effects": "No runtime ID assignment, subtree computation, position/line-map computation or lazy initialization; raw cached atomic loads and a read-only lazy-count lock."}}


ADDITIONS = {
    "crates/tsr_arena/src/arena.rs": '''
impl<T> Arena<T> {
    pub(crate) fn access_trace_layout(&self) -> [u64; 8] {
        let n = |value: usize| u64::try_from(value).expect("trace count fits u64");
        [u64::from(self.id.get()), n(self.len), n(self.pages.iter().map(|page| page.values.capacity()).sum()),
         n(self.pages.len()), n(self.pages.capacity()), n(std::mem::size_of::<T>()),
         n(std::mem::align_of::<T>()), n(std::mem::size_of::<Page<T>>())]
    }
    pub(crate) fn access_trace_pages(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.pages.iter().map(|page| (page.values.len(), page.values.capacity()))
    }
}
''',
    "crates/tsr_arena/src/file.rs": '''
impl<'a, N: NodeRecord, S> StorageView<'a, N, S> {
    pub fn access_trace_core_nodes(self) -> impl Iterator<Item = (NodeId, &'a N)> {
        let arena = self.owner.core.id;
        self.owner.core.values().enumerate().map(move |(index, value)|
            (NodeId::new(arena, crate::ids::next_slot(index).expect("allocated core slot")), value))
    }
    pub fn access_trace_core_auxiliary(self) -> impl Iterator<Item = (AuxId, &'a N::Aux)> {
        let arena = self.owner.auxiliary.id;
        self.owner.auxiliary.values().enumerate().map(move |(index, value)|
            (AuxId::new(arena, crate::ids::next_slot(index).expect("allocated auxiliary slot")), value))
    }
    pub fn access_trace_layout(self) -> [[u64; 8]; 2] {
        [self.owner.core.access_trace_layout(), self.owner.auxiliary.access_trace_layout()]
    }
    pub fn access_trace_pages(self, auxiliary: bool) -> impl Iterator<Item = (usize, usize)> + 'a {
        let core = (!auxiliary).then_some(&self.owner.core).into_iter().flat_map(|arena| arena.access_trace_pages());
        let aux = auxiliary.then_some(&self.owner.auxiliary).into_iter().flat_map(|arena| arena.access_trace_pages());
        core.chain(aux)
    }
    pub fn access_trace_external_counts(self) -> [usize; 3] {
        [self.owner.imports.len(), self.owner.imported_arenas.len(), self.owner.supplemental.len()]
    }
    pub fn access_trace_lazy_counts(self) -> [usize; 4] { self.owner.lazy.access_trace_counts() }
}
''',
    "crates/tsr_arena/src/lazy.rs": '''
impl<N: NodeRecord> LazyArena<N> {
    pub(crate) fn access_trace_counts(&self) -> [usize; 4] {
        let state = self.read();
        [state.pages.reserved, state.auxiliary.reserved,
         state.pages.directory.iter().map(|page| page.slots.iter().filter(|value| value.get().is_some()).count()).sum(),
         state.auxiliary.directory.iter().map(|page| page.slots.iter().filter(|value| value.get().is_some()).count()).sum()]
    }
}
''',
    "crates/tsr_ast/src/metadata.rs": "\n".join('''
impl NAME {
    pub(crate) fn access_trace_descriptor(self) -> (u64, u32, u32) {
        (self.backing.map_or(0, |id| id.bits()), self.start, self.len)
    }
}
'''.replace("NAME", name) for name in ("SourceNodeSlice", "SourceTextSlice", "CommentSlice", "PragmaSlice", "ReferenceSlice")),
    "crates/tsr_ast/src/lib.rs": "\npub mod access_trace_state;\n",
}


def apply(stage):
    stage = Path(stage).resolve()
    require(stage not in (ROOT, BASE.resolve()), "refusing to modify production or frozen source")
    raw = (stage / SCHEMA).read_bytes()
    require(hashlib.sha256(raw).hexdigest() == SCHEMA_SHA256, "staged payload schema differs from pinned CP1")
    shapes = schema(raw.decode())
    require(json.loads((HERE / "state-registry.json").read_text()) == registry(shapes), "state registry drift")
    changes = {}
    originals = {}
    for name, addition in ADDITIONS.items():
        path = stage / name
        require(not path.is_symlink(), "staged observer target is a symlink")
        original = path.read_text()
        require(MARKER not in original, "state observer already applied")
        originals[name] = original
        changes[name] = original + "\n" + MARKER + "\n" + addition
    for name, body in (("crates/tsr_ast/src/access_trace_state.rs", (HERE / "src/state.rs").read_text()),
                       ("crates/tsr_ast/src/access_trace_state_generated.rs", generated(shapes))):
        require(not (stage / name).exists(), "state observer target already exists")
        originals[name] = ""
        changes[name] = body
    patch = []
    for name, body in sorted(changes.items()):
        patch.extend(difflib.unified_diff(originals[name].splitlines(keepends=True), body.splitlines(keepends=True), fromfile=name, tofile=name))
        (stage / name).write_text(body)
    (stage / "access-trace-state.patch").write_text("".join(patch))
    return sorted(changes)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write-registry", action="store_true")
    args = parser.parse_args()
    raw = (BASE / SCHEMA).read_bytes()
    require(hashlib.sha256(raw).hexdigest() == SCHEMA_SHA256, "frozen schema changed")
    value = registry(schema(raw.decode()))
    path = HERE / "state-registry.json"
    if args.write_registry:
        path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    else:
        require(json.loads(path.read_text()) == value, "state registry drift")
    print(json.dumps({"shapes": len(value["shapes"]), "fields": sum(len(s["fields"]) for s in value["shapes"]), "verified": True}))
