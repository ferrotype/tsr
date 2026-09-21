package diagnosticwriter

// Phase 1 F3a, `diagnosticWriter` group: access only. It replays an ordered
// action trace against the pinned internal/diagnosticwriter package and records
// what each action observed. It computes nothing of its own beyond rendering,
// and it never carries an expected value.
//
// It is an IN-PACKAGE test file, and it is the FIRST _test.go this package has
// ever had -- upstream/tsc/internal/diagnosticwriter holds exactly one source
// file, diagnosticwriter.go, and no test. In-package is required, not merely
// convenient: eleven of the eighteen operations this group prepares are
// unexported (diagnosticPrefix :375, getCategoryFormat :382, writeWithStyleAndReset
// :398, prettyPathForFileError :544, writeTabularErrorsDisplay :514,
// flattenDiagnosticMessageChain :357, newOriginalTextFile :319 and the three
// methods each of originalTextFile :327-:329 and renamedFile :337-:339 --
// line numbers are upstream/tsc/internal/diagnosticwriter/diagnosticwriter.go),
// and an external `diagnosticwriter_test` file could reach none of them.
//
// Because the package has no test file at all, this overlay is also the only
// thing that makes `go test ./internal/diagnosticwriter` build a test binary;
// without it the package reports "[no test files]". Nothing else about the
// build changes: there is no existing package-level test scope to collide with,
// and there is no `diagnosticwriter_test` external package. Every name declared
// here is still prefixed `phase1` so a later pin that adds its own test file
// cannot collide with a probe helper.
//
// Byte payloads are hex on the wire. encoding/json replaces invalid UTF-8 with
// U+FFFD, and this group's whole subject is bytes: ANSI escape vocabulary,
// gutter padding, squiggles and -- in the ToValidUTF8 case -- deliberately
// invalid argument bytes. An argument travels as UTF-8 text under its own key
// or as hex under `<key>_hex`, and exactly one of the two must be present.
// Actions decode into a raw key map rather than into a struct, so a missing key
// is a harness failure instead of a silently defaulted observation: a defaulted
// `category: 0` would make two sides agree on a colour neither was asked for.
// An unknown action is a harness failure for the same reason.
//
// Nothing here is rendered as a multi-key JSON object below the row level. Each
// row is one flat object of named result fields; everything nested inside is a
// positional array, because the comparison canonicalises with sorted keys and a
// nested object's member order would not survive it.
//
// Two pinned entry points panic on inputs this file supplies on purpose:
// getCategoryFormat panics on a category outside 0..3 (:393), and
// diagnostics.Localize panics on an unknown message key. Only the
// getCategoryFormat call is guarded, and its guard records the recovered value
// as an observation -- the panic IS the pinned behaviour of that arm. Every
// other panic reaching the test is a harness failure and fails the capture,
// which is what it should do.
//
// Nothing in this group depends on goroutine scheduling: there is no concurrent
// map, no worker pool and no time source. The two watch-mode status renderers
// take their timestamp as a caller-supplied string, so this file passes a fixed
// literal from the request and observes no clock.

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
	"github.com/microsoft/TypeScript/tsc/internal/locale"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/spanmap"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

// phase1Action is one action's raw key map. Decoding into a map rather than a
// struct is what makes a missing key observable.
type phase1Action map[string]json.RawMessage

type phase1Request struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared family schedule and
	// another group's actions need not fit this group's shapes.
	Actions json.RawMessage `json:"actions"`
}

const phase1Subject = "diagnosticWriter"

func phase1Hex(value string) string {
	return hex.EncodeToString([]byte(value))
}

func phase1Raw(act phase1Action, key string) json.RawMessage {
	raw, ok := act[key]
	if !ok {
		panic("phase1: action is missing the key " + key)
	}
	return raw
}

func phase1JSONString(raw json.RawMessage, key string) string {
	var out string
	if err := json.Unmarshal(raw, &out); err != nil {
		panic("phase1: key " + key + " is not a JSON string: " + err.Error())
	}
	return out
}

func phase1DecodeHex(text string, key string) string {
	decoded, err := hex.DecodeString(text)
	if err != nil {
		panic("phase1: key " + key + " is malformed hex: " + err.Error())
	}
	return string(decoded)
}

// bytes reads a byte argument: UTF-8 text under `key`, or hex under
// `key+"_hex"`. Exactly one of the two must be present.
func (act phase1Action) bytes(key string) string {
	plain, hasPlain := act[key]
	encoded, hasEncoded := act[key+"_hex"]
	if hasPlain == hasEncoded {
		panic("phase1: action must carry exactly one of " + key + " and " + key + "_hex")
	}
	if hasPlain {
		return phase1JSONString(plain, key)
	}
	return phase1DecodeHex(phase1JSONString(encoded, key+"_hex"), key+"_hex")
}

// optionalBytes reads a byte argument that the action may omit entirely. Absent
// is distinct from empty: an absent `source` leaves the diagnostic a compiler
// diagnostic, an empty one is the same value written out.
func (act phase1Action) optionalBytes(key string) (string, bool) {
	_, hasPlain := act[key]
	_, hasEncoded := act[key+"_hex"]
	if !hasPlain && !hasEncoded {
		return "", false
	}
	return act.bytes(key), true
}

func (act phase1Action) op() string {
	return phase1JSONString(phase1Raw(act, "op"), "op")
}

func (act phase1Action) number(key string) int {
	var out int
	if err := json.Unmarshal(phase1Raw(act, key), &out); err != nil {
		panic("phase1: key " + key + " is not a JSON integer: " + err.Error())
	}
	return out
}

func (act phase1Action) flag(key string) bool {
	var out bool
	if err := json.Unmarshal(phase1Raw(act, key), &out); err != nil {
		panic("phase1: key " + key + " is not a JSON boolean: " + err.Error())
	}
	return out
}

func (act phase1Action) names(key string) []string {
	var out []string
	if err := json.Unmarshal(phase1Raw(act, key), &out); err != nil {
		panic("phase1: key " + key + " is not a JSON array of strings: " + err.Error())
	}
	return out
}

func phase1Actions(raw json.RawMessage) []phase1Action {
	if len(raw) == 0 {
		panic("phase1: missing actions")
	}
	var out []phase1Action
	if err := json.Unmarshal(raw, &out); err != nil {
		// Decoding happens before any pinned call. A malformed request must
		// fail the probe, never turn into an empty observation.
		panic("phase1: invalid action payload: " + err.Error())
	}
	if len(out) == 0 {
		panic("phase1: empty action trace")
	}
	return out
}

// phase1DiagnosticSpec is the request's description of one ast.Diagnostic. It
// is decoded recursively so a chain or a related entry is described the same
// way as its parent.
type phase1DiagnosticSpec struct {
	File        string                 `json:"file"`
	Pos         int                    `json:"pos"`
	End         int                    `json:"end"`
	Code        int32                  `json:"code"`
	Category    int32                  `json:"category"`
	Source      string                 `json:"source"`
	MessageKey  string                 `json:"message_key"`
	MessageText string                 `json:"message_text"`
	Args        []string               `json:"args"`
	ArgsHex     []string               `json:"args_hex"`
	Chain       []phase1DiagnosticSpec `json:"chain"`
	Related     []phase1DiagnosticSpec `json:"related"`
}

func (spec phase1DiagnosticSpec) arguments() []string {
	if spec.Args != nil && spec.ArgsHex != nil {
		panic("phase1: a diagnostic spec carries both args and args_hex")
	}
	if spec.ArgsHex == nil {
		return spec.Args
	}
	out := make([]string, len(spec.ArgsHex))
	for i, encoded := range spec.ArgsHex {
		out[i] = phase1DecodeHex(encoded, "args_hex")
	}
	return out
}

// phase1State holds the live fixtures of one replay, keyed by the name every
// defining action carries under `target`.
type phase1State struct {
	files map[string]*ast.SourceFile
	diags map[string]*ast.Diagnostic
}

func (state *phase1State) file(name string) *ast.SourceFile {
	if name == "" {
		return nil
	}
	file, ok := state.files[name]
	if !ok {
		panic("phase1: no file named " + name)
	}
	return file
}

func (state *phase1State) diagnostic(name string) *ast.Diagnostic {
	diag, ok := state.diags[name]
	if !ok {
		panic("phase1: no diagnostic named " + name)
	}
	return diag
}

func (state *phase1State) diagnostics(names []string) []*ast.Diagnostic {
	out := make([]*ast.Diagnostic, 0, len(names))
	for _, name := range names {
		out = append(out, state.diagnostic(name))
	}
	return out
}

// phase1Build turns a spec into a pinned *ast.Diagnostic. Two pinned
// constructors are used and nothing else: NewExternalDiagnostic for a spec that
// carries already-localized text (ast/diagnostic.go:247), and
// NewDiagnosticFromSerialized for a spec that names a message key
// (ast/diagnostic.go:166). Both leave `message` nil, which is what makes the
// Localize path a key lookup rather than a pointer the probe chose.
func (state *phase1State) build(spec phase1DiagnosticSpec) *ast.Diagnostic {
	loc := core.NewTextRange(spec.Pos, spec.End)
	file := state.file(spec.File)
	chain := make([]*ast.Diagnostic, 0, len(spec.Chain))
	for _, child := range spec.Chain {
		chain = append(chain, state.build(child))
	}
	related := make([]*ast.Diagnostic, 0, len(spec.Related))
	for _, child := range spec.Related {
		related = append(related, state.build(child))
	}
	if spec.MessageText != "" {
		diag := ast.NewExternalDiagnostic(
			file, loc, spec.Source, diagnostics.Category(spec.Category), spec.Code, spec.MessageText)
		if len(chain) > 0 {
			diag.SetMessageChain(chain)
		}
		if len(related) > 0 {
			diag.SetRelatedInfo(related)
		}
		return diag
	}
	if spec.MessageKey == "" {
		panic("phase1: a diagnostic spec needs exactly one of message_text and message_key")
	}
	diag := ast.NewDiagnosticFromSerialized(
		file,
		loc,
		spec.Code,
		diagnostics.Category(spec.Category),
		diagnostics.Key(spec.MessageKey),
		spec.arguments(),
		chain,
		related,
		false, /*reportsUnnecessary*/
		false, /*reportsDeprecated*/
		false, /*skippedOnNoEmit*/
	)
	if spec.Source != "" {
		// SetExternalData is the only pinned way to set `source` on a keyed
		// diagnostic; the second argument stays empty so the message is still
		// resolved through the key rather than short-circuited by text.
		diag.SetExternalData(spec.Source, "")
	}
	return diag
}

// phase1Segments decodes a span map. A segment is a positional array
// [virtualStart, virtualEnd, originalStart, originalEnd, kind]; anything the
// array does not cover is a synthesized gap, which is the pin's own model
// (spanmap.go:219-221).
func phase1Segments(act phase1Action) *spanmap.SpanMap {
	raw, ok := act["segments"]
	if !ok {
		return nil
	}
	var rows [][]int
	if err := json.Unmarshal(raw, &rows); err != nil {
		panic("phase1: key segments is not a JSON array of arrays: " + err.Error())
	}
	segments := make([]spanmap.Segment, 0, len(rows))
	for _, row := range rows {
		if len(row) != 5 {
			panic("phase1: a segment must be [virtual_start, virtual_end, original_start, original_end, kind]")
		}
		segments = append(segments, spanmap.Segment{
			VirtualStart:  core.TextPos(row[0]),
			VirtualEnd:    core.TextPos(row[1]),
			OriginalStart: core.TextPos(row[2]),
			OriginalEnd:   core.TextPos(row[3]),
			Kind:          spanmap.Kind(row[4]),
			Features:      spanmap.FeatureAll,
		})
	}
	return spanmap.New(segments)
}

// phase1FormatOpts builds the pinned FormattingOptions from the action. Every
// field is required: a defaulted current directory would silently change what
// ConvertToRelativePath returns.
func phase1FormatOpts(act phase1Action) *FormattingOptions {
	return &FormattingOptions{
		Locale: locale.Default,
		ComparePathsOptions: tspath.ComparePathsOptions{
			UseCaseSensitiveFileNames: act.flag("case_sensitive"),
			CurrentDirectory:          act.bytes("current_directory"),
		},
		NewLine: act.bytes("new_line"),
	}
}

// phase1FileKind names which concrete FileLike an arm produced. The type switch
// is the observation: it is how a capture records that ASTDiagnostic.File took
// the original-text arm rather than the rename arm, which no byte rendering of
// the result can distinguish on its own.
func phase1FileKind(file FileLike) string {
	switch file.(type) {
	case nil:
		return "nil"
	case *originalTextFile:
		return "original_text_file"
	case *renamedFile:
		return "renamed_file"
	case *ast.SourceFile:
		return "source_file"
	default:
		return fmt.Sprintf("%T", file)
	}
}

// phase1LineMap renders a FileLike's ECMA line starts as a flat array. It is
// the cheapest complete observation of which text a wrapper is presenting: the
// original and the virtual text of a content-mapped file differ in length and
// in line structure, so the two wrappers cannot produce the same array.
func phase1LineMap(file FileLike) []any {
	starts := file.ECMALineMap()
	out := make([]any, 0, len(starts))
	for _, start := range starts {
		out = append(out, int(start))
	}
	return out
}

func phase1RowDefine(state *phase1State, act phase1Action, op string, row map[string]any) bool {
	switch op {
	case "define_file":
		name := act.bytes("target")
		fileName := act.bytes("file_name")
		text := act.bytes("text")
		file := parser.ParseSourceFile(
			ast.SourceFileParseOptions{FileName: fileName, Path: tspath.Path(fileName)},
			text,
			core.ScriptKindTS,
		)
		mapper, hasMapper := act.optionalBytes("content_mapper")
		canonicalName, hasCanonical := act.optionalBytes("canonical")
		originalText, hasOriginal := act.optionalBytes("original_text")
		spans := phase1Segments(act)
		if hasMapper || hasCanonical || hasOriginal || spans != nil {
			info := ast.ContentMapperSourceFileInfo{
				ContentMapper:   mapper,
				VirtualFileName: fileName,
				OriginalText:    originalText,
				SpanMap:         spans,
			}
			if hasCanonical {
				info.CanonicalSourceFile = state.file(canonicalName)
			}
			file.SetContentMapperInfo(info)
		}
		state.files[name] = file
		// Read back off the constructed file rather than echoing the request:
		// a fixture that did not take the metadata is then visible here and not
		// three actions later.
		row["file_name_hex"] = phase1Hex(file.FileName())
		row["text_len"] = len(file.Text())
		row["original_text_len"] = len(file.OriginalText())
		row["content_mapper_hex"] = phase1Hex(file.ContentMapper())
		row["has_span_map"] = file.SpanMap() != nil
		row["has_canonical"] = file.CanonicalSourceFile() != nil
	case "define_diagnostic":
		name := act.bytes("target")
		var spec phase1DiagnosticSpec
		if err := json.Unmarshal(phase1Raw(act, "diagnostic"), &spec); err != nil {
			panic("phase1: key diagnostic is not a diagnostic spec: " + err.Error())
		}
		diag := state.build(spec)
		state.diags[name] = diag
		row["code"] = int(diag.Code())
		row["category"] = int(diag.Category())
		row["has_file"] = diag.File() != nil
		row["chain_len"] = len(diag.MessageChain())
		row["related_len"] = len(diag.RelatedInformation())
	default:
		return false
	}
	return true
}

func phase1RowASTDiagnostic(state *phase1State, act phase1Action, op string, row map[string]any) bool {
	var wrapped *ASTDiagnostic
	switch op {
	case "astdiag_positions", "astdiag_source", "astdiag_prefix",
		"astdiag_message_chain", "astdiag_related", "astdiag_file":
		wrapped = WrapASTDiagnostic(state.diagnostic(act.bytes("target")))
	default:
		return false
	}
	switch op {
	case "astdiag_positions":
		row["pos"] = wrapped.Pos()
		row["end"] = wrapped.End()
		row["len"] = wrapped.Len()
	case "astdiag_source":
		row["source_hex"] = phase1Hex(wrapped.Source())
	case "astdiag_prefix":
		row["prefix_hex"] = phase1Hex(diagnosticPrefix(wrapped))
		row["source_hex"] = phase1Hex(wrapped.Source())
	case "astdiag_message_chain":
		chain := wrapped.MessageChain()
		entries := make([]any, 0, len(chain))
		for _, entry := range chain {
			entries = append(entries, []any{
				int(entry.Code()),
				phase1Hex(entry.Localize(locale.Default)),
			})
		}
		row["len"] = len(chain)
		row["entries"] = entries
	case "astdiag_related":
		related := wrapped.RelatedInformation()
		entries := make([]any, 0, len(related))
		for _, entry := range related {
			entries = append(entries, []any{
				int(entry.Code()),
				phase1Hex(entry.Source()),
				entry.File() != nil,
			})
		}
		row["len"] = len(related)
		row["entries"] = entries
	case "astdiag_file":
		file := wrapped.File()
		row["kind"] = phase1FileKind(file)
		if file == nil {
			row["file_name_hex"] = ""
			row["text_len"] = -1
			row["line_map"] = []any{}
			break
		}
		row["file_name_hex"] = phase1Hex(file.FileName())
		row["text_len"] = len(file.Text())
		row["line_map"] = phase1LineMap(file)
	}
	return true
}

func phase1RowFileWrappers(state *phase1State, act phase1Action, op string, row map[string]any) bool {
	var file FileLike
	switch op {
	case "new_original_text_file":
		file = newOriginalTextFile(state.file(act.bytes("target")), act.bytes("file_name"))
	case "renamed_file":
		file = &renamedFile{file: state.file(act.bytes("target")), fileName: act.bytes("file_name")}
	default:
		return false
	}
	row["kind"] = phase1FileKind(file)
	row["file_name_hex"] = phase1Hex(file.FileName())
	row["text_hex"] = phase1Hex(file.Text())
	row["line_map"] = phase1LineMap(file)
	return true
}

// phase1CategoryFormat calls the pinned getCategoryFormat under a guard. The
// panic on an unhandled category (:393) is that arm's pinned behaviour, so the
// recovered value is recorded as an observation rather than failing the probe.
// The recovered string is the pin's own literal, not a library error message.
func phase1CategoryFormat(category int32) (format string, panicked bool, message string) {
	defer func() {
		if recovered := recover(); recovered != nil {
			panicked = true
			message = fmt.Sprintf("%v", recovered)
		}
	}()
	return getCategoryFormat(diagnostics.Category(category)), false, ""
}

func phase1RowVocabulary(state *phase1State, act phase1Action, op string, row map[string]any) bool {
	switch op {
	case "category_format":
		category := int32(act.number("category"))
		format, panicked, message := phase1CategoryFormat(category)
		row["panicked"] = panicked
		row["format_hex"] = phase1Hex(format)
		row["panic_hex"] = phase1Hex(message)
		row["category_name_hex"] = ""
		if !panicked {
			row["category_name_hex"] = phase1Hex(diagnostics.Category(category).Name())
		}
	case "style_and_reset":
		var out bytes.Buffer
		writeWithStyleAndReset(&out, act.bytes("text"), act.bytes("style"))
		row["output_hex"] = phase1Hex(out.String())
	default:
		return false
	}
	return true
}

func phase1RowSummary(state *phase1State, act phase1Action, op string, row map[string]any) bool {
	switch op {
	case "pretty_path":
		file := state.file(act.bytes("target"))
		errorsForFile := FromASTDiagnostics(state.diagnostics(act.names("errors")))
		row["path_hex"] = phase1Hex(prettyPathForFileError(file, errorsForFile, phase1FormatOpts(act)))
	case "pretty_path_nil_file":
		// The guard arm: a nil file or an empty error slice returns "" before
		// anything is read (:545-547).
		row["path_hex"] = phase1Hex(prettyPathForFileError(nil, nil, phase1FormatOpts(act)))
	case "tabular_errors":
		// The ErrorSummary is built from the request, NOT from getErrorSummary.
		// getErrorSummary is already witnessed elsewhere, and assembling the
		// summary here is what isolates the tabular renderer from the grouping
		// and sorting that normally precede it.
		var rows [][]string
		if err := json.Unmarshal(phase1Raw(act, "files"), &rows); err != nil {
			panic("phase1: key files is not a JSON array of arrays: " + err.Error())
		}
		summary := &ErrorSummary{ErrorsByFile: map[FileLike][]Diagnostic{}}
		for _, entry := range rows {
			if len(entry) < 2 {
				panic("phase1: a tabular entry must be [file, diagnostic, ...]")
			}
			file := FileLike(state.file(entry[0]))
			errorsForFile := FromASTDiagnostics(state.diagnostics(entry[1:]))
			summary.ErrorsByFile[file] = errorsForFile
			summary.SortedFiles = append(summary.SortedFiles, file)
			summary.TotalErrorCount += len(errorsForFile)
		}
		var out bytes.Buffer
		writeTabularErrorsDisplay(&out, summary, phase1FormatOpts(act))
		row["output_hex"] = phase1Hex(out.String())
	default:
		return false
	}
	return true
}

func phase1RowWatch(state *phase1State, act phase1Action, op string, row map[string]any) bool {
	switch op {
	case "status_with_color_and_time":
		var out bytes.Buffer
		FormatDiagnosticsStatusWithColorAndTime(
			&out, act.bytes("time"), WrapASTDiagnostic(state.diagnostic(act.bytes("target"))),
			phase1FormatOpts(act))
		row["output_hex"] = phase1Hex(out.String())
	case "status_and_time":
		var out bytes.Buffer
		FormatDiagnosticsStatusAndTime(
			&out, act.bytes("time"), WrapASTDiagnostic(state.diagnostic(act.bytes("target"))),
			phase1FormatOpts(act))
		row["output_hex"] = phase1Hex(out.String())
	case "try_clear_screen":
		var out bytes.Buffer
		options := &core.CompilerOptions{
			PreserveWatchOutput: core.Tristate(act.number("preserve_watch_output")),
			ExtendedDiagnostics: core.Tristate(act.number("extended_diagnostics")),
			Diagnostics:         core.Tristate(act.number("diagnostics")),
		}
		cleared := TryClearScreen(
			&out, WrapASTDiagnostic(state.diagnostic(act.bytes("target"))), options)
		row["cleared"] = cleared
		row["output_hex"] = phase1Hex(out.String())
	default:
		return false
	}
	return true
}

// phase1RowWrappers observes the Go-only wrapper layer. The contract of all
// four is allocation plus element identity, so identity is what is recorded:
// whether element i of the result still points at input i. A rendering of the
// elements would observe the wrapped diagnostics instead, which is a different
// question already answered elsewhere.
func phase1RowWrappers(state *phase1State, act phase1Action, op string, row map[string]any) bool {
	switch op {
	case "wrap_one":
		input := state.diagnostic(act.bytes("target"))
		wrapped := WrapASTDiagnostic(input)
		row["not_nil"] = wrapped != nil
		row["same_pointer"] = wrapped.Diagnostic == input
	case "wrap_many":
		input := state.diagnostics(act.names("targets"))
		wrapped := WrapASTDiagnostics(input)
		identity := make([]any, 0, len(wrapped))
		for i, entry := range wrapped {
			identity = append(identity, entry.Diagnostic == input[i])
		}
		row["len"] = len(wrapped)
		row["same_pointers"] = identity
		row["input_len"] = len(input)
	case "from_ast":
		input := state.diagnostics(act.names("targets"))
		converted := FromASTDiagnostics(input)
		identity := make([]any, 0, len(converted))
		for i, entry := range converted {
			wrapped, ok := entry.(*ASTDiagnostic)
			identity = append(identity, []any{ok, ok && wrapped.Diagnostic == input[i]})
		}
		row["len"] = len(converted)
		row["same_pointers"] = identity
		row["input_len"] = len(input)
	case "to_diagnostics":
		input := state.diagnostics(act.names("targets"))
		wrapped := WrapASTDiagnostics(input)
		converted := ToDiagnostics(wrapped)
		identity := make([]any, 0, len(converted))
		for i, entry := range converted {
			same, ok := entry.(*ASTDiagnostic)
			identity = append(identity, []any{ok, ok && same == wrapped[i]})
		}
		row["len"] = len(converted)
		row["same_pointers"] = identity
		row["input_len"] = len(input)
	case "compare":
		left := WrapASTDiagnostic(state.diagnostic(act.bytes("left")))
		right := WrapASTDiagnostic(state.diagnostic(act.bytes("right")))
		forward := CompareASTDiagnostics(left, right)
		backward := CompareASTDiagnostics(right, left)
		// The sign, not the magnitude: the pin subtracts ints and returns the
		// difference, so the magnitude is an accident of the inputs and no port
		// contract depends on it.
		row["forward_sign"] = phase1Sign(forward)
		row["backward_sign"] = phase1Sign(backward)
	default:
		return false
	}
	return true
}

func phase1Sign(value int) int {
	switch {
	case value < 0:
		return -1
	case value > 0:
		return 1
	default:
		return 0
	}
}

func phase1RowFlatten(state *phase1State, act phase1Action, op string, row map[string]any) bool {
	switch op {
	case "flatten":
		diag := WrapASTDiagnostic(state.diagnostic(act.bytes("target")))
		row["text_hex"] = phase1Hex(
			FlattenDiagnosticMessage(diag, act.bytes("new_line"), locale.Default))
	case "flatten_chain":
		// flattenDiagnosticMessageChain is reached directly, at a caller-chosen
		// level, so the indent arithmetic is observed without the parent
		// message that WriteFlattenedDiagnosticMessage would print first.
		var out bytes.Buffer
		chain := WrapASTDiagnostic(state.diagnostic(act.bytes("target")))
		flattenDiagnosticMessageChain(
			&out, chain, act.bytes("new_line"), locale.Default, act.number("level"))
		row["text_hex"] = phase1Hex(out.String())
	case "write_flattened_ast":
		var out bytes.Buffer
		WriteFlattenedASTDiagnosticMessage(
			&out, state.diagnostic(act.bytes("target")), act.bytes("new_line"), locale.Default)
		row["text_hex"] = phase1Hex(out.String())
	default:
		return false
	}
	return true
}

func phase1Row(state *phase1State, act phase1Action) map[string]any {
	op := act.op()
	row := map[string]any{"op": op}
	claimed := phase1RowDefine(state, act, op, row) ||
		phase1RowASTDiagnostic(state, act, op, row) ||
		phase1RowFileWrappers(state, act, op, row) ||
		phase1RowVocabulary(state, act, op, row) ||
		phase1RowSummary(state, act, op, row) ||
		phase1RowWatch(state, act, op, row) ||
		phase1RowWrappers(state, act, op, row) ||
		phase1RowFlatten(state, act, op, row)
	if !claimed {
		panic("phase1: unsupported action: " + op)
	}
	return row
}

func phase1Replay(request phase1Request) []any {
	state := &phase1State{
		files: map[string]*ast.SourceFile{},
		diags: map[string]*ast.Diagnostic{},
	}
	rows := []any{}
	for _, act := range phase1Actions(request.Actions) {
		rows = append(rows, phase1Row(state, act))
	}
	return rows
}

func TestPhase1ConfigDiagnosticWriter(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []phase1Request `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Subject == phase1Subject {
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": phase1Replay(request)}
		} else {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the diagnosticWriter probe"
		}
		observations = append(observations, row)
	}

	hash := sha256.Sum256(input)
	output := map[string]any{
		"request_sha256": hex.EncodeToString(hash[:]),
		"go":             runtime.Version(),
		"goos":           runtime.GOOS,
		"goarch":         runtime.GOARCH,
		"version":        1,
		"observations":   observations,
	}
	data, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
