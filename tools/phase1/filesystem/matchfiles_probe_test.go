package tsoptions_test

// Access only, and the carried `config/matchFiles` test renderer the owner
// approved on 2026-09-20 (docs/PHASE1-progress.md, "Approved: carry the
// `config/matchFiles` test renderer").
//
// The 142 frozen `config/matchFiles` reference outputs have no producer at this
// pin: no pinned test writes that subfolder, and the envelope they carry
// (`config:`, `Fs::`, `configFileName::`, `Result`, `Errors::`) matches no
// pinned renderer. This file carries the envelope and nothing else. Every value
// inside it comes from the pinned configuration parse:
//
//   * the two entry points are the pinned test helpers `getParsedWithJsonApi`
//     and `getParsedWithJsonSourceFileApi`, called directly rather than
//     reimplemented, which is why this file compiles into `tsoptions_test`;
//   * the `Fs::` section is the pinned `printFS`, called directly;
//   * the `Errors::` section is the pinned
//     `diagnosticwriter.FormatDiagnosticsWithColorAndContext`;
//   * every `Result` field is read off the `*tsoptions.ParsedCommandLine` the
//     pinned parse returned;
//   * the `wildcardDirectories` section is the pinned
//     `ParsedCommandLine.WildcardDirectories()` on the `jsonSourceFile` path,
//     and on the raw-`json` path -- where the parse attaches no ConfigFile and
//     that accessor cannot run -- the pinned `getWildcardDirectories` reached
//     through the overlay's in-package companion,
//     `matchfiles_inpackage_test.go`, which re-derives the validated specs the
//     worker computes and discards. Each row records which of the two it used
//     under `wildcard_source`. That section's *key order* is the pinned
//     calculation's own insertion order, which its `map[string]bool` return
//     discards; it is recovered from the include specs the calculation was
//     given, again through the in-package companion, and never from the frozen
//     `Result`. Each row records how under `wildcard_order_source`.
//
// Each case's inputs are recovered from the baseline's own *input* sections --
// `config:`, `Fs::` and `configFileName::`. The probe never reads `Result` or
// `Errors::` to build a case, and never edits, repairs or special-cases a
// baseline: it renders what the pinned parse answered and reports whether those
// bytes equal the frozen ones.

import (
	"crypto/sha256"
	"encoding/hex"
	stdjson "encoding/json"
	"fmt"
	"io"
	"io/fs"
	"maps"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
	"github.com/microsoft/TypeScript/tsc/internal/diagnosticwriter"
	"github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/repo"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions/tsoptionstest"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs"
)

const (
	matchFilesOperation  = "tsoptions.matchFilesBaseline"
	matchFilesEntryMark  = "//// ["
	matchFilesConfigHead = "config:\n"
	matchFilesFsHead     = "\nFs::\n"
	matchFilesNameHead   = "\nconfigFileName:: "
	matchFilesResultHead = "\nResult\n"
)

type matchFilesRequest struct {
	Case       string `json:"case"`
	Operation  string `json:"operation"`
	Baseline   string `json:"baseline"`
	API        string `json:"api"`
	EntryPoint string `json:"entry_point"`
}

// matchFilesEntry is one `//// [path]` block of the `Fs::` section: either a
// regular file with its content, or a symlink with its target.
type matchFilesEntry struct {
	path    string
	content string
	target  string
	link    bool
}

// matchFilesInputs is everything the baseline's input sections declare.
type matchFilesInputs struct {
	configText     string
	configFileName string
	entries        []matchFilesEntry
	// The root the `Fs::` walk starts from, taken from the entries' own root
	// length: `/` for a posix filesystem, `c:/` for a windows one. Walking a
	// windows filesystem from `/` prefixes every printed path with a slash.
	root string
	// The `Fs::` paths name the host: the TypeScript matchFiles suite ran a
	// case-insensitive host rooted at `c:/dev` and a case-sensitive host rooted
	// at `/dev`, and vfstest.FromMap panics on a map that mixes the two path
	// styles, so the style is the host and is read off the input.
	caseSensitive bool
}

func matchFilesSplit(text string, head string) (string, string, bool) {
	index := strings.Index(text, head)
	if index < 0 {
		return "", "", false
	}
	return text[:index], text[index+len(head):], true
}

// matchFilesParseFS reads the `Fs::` section back into its entries. The section
// is exactly what `printFS` wrote: `//// [path]\r\n<content>\r\n\r\n` for a
// regular file, and `//// [path] symlink(target)\r\n` for a symlink.
func matchFilesParseFS(section string) ([]matchFilesEntry, error) {
	var starts []int
	for offset := 0; offset < len(section); {
		relative := strings.Index(section[offset:], matchFilesEntryMark)
		if relative < 0 {
			break
		}
		index := offset + relative
		if index == 0 || section[index-1] == '\n' {
			starts = append(starts, index)
		}
		offset = index + len(matchFilesEntryMark)
	}
	entries := make([]matchFilesEntry, 0, len(starts))
	for position, start := range starts {
		end := len(section)
		if position+1 < len(starts) {
			end = starts[position+1]
		}
		block := section[start:end]
		lineEnd := strings.Index(block, "\r\n")
		if lineEnd < 0 {
			return nil, fmt.Errorf("Fs:: entry at offset %d has no CRLF header terminator", start)
		}
		header := block[:lineEnd]
		bracket := strings.Index(header, "]")
		if bracket < 0 {
			return nil, fmt.Errorf("Fs:: header %q has no closing bracket", header)
		}
		path := header[len(matchFilesEntryMark):bracket]
		trailer := header[bracket+1:]
		body := block[lineEnd+len("\r\n"):]
		if target, ok := strings.CutPrefix(trailer, " symlink("); ok {
			target, closed := strings.CutSuffix(target, ")")
			if !closed {
				return nil, fmt.Errorf("Fs:: symlink header %q is unterminated", header)
			}
			if body != "" {
				return nil, fmt.Errorf("Fs:: symlink %q carries a body", path)
			}
			entries = append(entries, matchFilesEntry{path: path, target: target, link: true})
			continue
		}
		if trailer != "" {
			return nil, fmt.Errorf("Fs:: header %q has an unknown trailer %q", header, trailer)
		}
		content, ok := strings.CutSuffix(body, "\r\n\r\n")
		if !ok {
			return nil, fmt.Errorf("Fs:: entry %q does not end in a blank CRLF line", path)
		}
		entries = append(entries, matchFilesEntry{path: path, content: content})
	}
	return entries, nil
}

func matchFilesReadInputs(text string) (matchFilesInputs, error) {
	var inputs matchFilesInputs
	body, ok := strings.CutPrefix(text, matchFilesConfigHead)
	if !ok {
		return inputs, fmt.Errorf("baseline does not open with %q", matchFilesConfigHead)
	}
	configText, rest, ok := matchFilesSplit(body, matchFilesFsHead)
	if !ok {
		return inputs, fmt.Errorf("baseline has no %q section", matchFilesFsHead)
	}
	section, rest, ok := matchFilesSplit(rest, matchFilesNameHead)
	if !ok {
		return inputs, fmt.Errorf("baseline has no %q section", matchFilesNameHead)
	}
	configFileName, _, ok := matchFilesSplit(rest, matchFilesResultHead)
	if !ok {
		return inputs, fmt.Errorf("baseline has no %q section", matchFilesResultHead)
	}
	if strings.Contains(configFileName, "\n") {
		return inputs, fmt.Errorf("configFileName:: %q spans more than one line", configFileName)
	}
	entries, err := matchFilesParseFS(section)
	if err != nil {
		return inputs, err
	}
	if len(entries) == 0 {
		return inputs, fmt.Errorf("the Fs:: section declares no entries")
	}
	posix, windows := false, false
	for _, entry := range entries {
		if strings.HasPrefix(entry.path, "/") {
			posix = true
		} else {
			windows = true
		}
	}
	if posix && windows {
		return inputs, fmt.Errorf("the Fs:: section mixes posix and windows paths")
	}
	root := entries[0].path[:tspath.GetRootLength(entries[0].path)]
	if root == "" {
		return inputs, fmt.Errorf("the Fs:: entry %q is not rooted", entries[0].path)
	}
	inputs.configText = configText
	inputs.configFileName = configFileName
	inputs.entries = entries
	inputs.root = root
	inputs.caseSensitive = posix
	return inputs, nil
}

// matchFilesPrintFS writes the `Fs::` section. With no symlink in the input it
// is the pinned `printFS`, called directly. `printFS` writes only entries whose
// type `IsRegular`, so a filesystem that declares symlinked directories needs
// the same walk with the one extra entry shape; the choice is made from the
// input, never from the expected bytes.
func matchFilesPrintFS(output io.Writer, files vfs.FS, inputs matchFilesInputs) error {
	targets := map[string]string{}
	for _, entry := range inputs.entries {
		if entry.link {
			targets[entry.path] = entry.target
		}
	}
	if len(targets) == 0 {
		return printFS(output, files, inputs.root)
	}
	return files.WalkDir(inputs.root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.Type()&fs.ModeSymlink != 0 {
			target, ok := targets[path]
			if !ok {
				return fmt.Errorf("symlink %s has no declared target", path)
			}
			_, err := fmt.Fprintf(output, "//// [%s] symlink(%s)\r\n", path, target)
			return err
		}
		if d.Type().IsRegular() {
			content, ok := files.ReadFile(path)
			if !ok {
				return fmt.Errorf("failed to read file %s", path)
			}
			_, err := fmt.Fprintf(output, "//// [%s]\r\n%s\r\n\r\n", path, content)
			return err
		}
		return nil
	})
}

// matchFilesResult is the carried shape of the `Result` section. It is the
// TypeScript `ParsedCommandLine` the promoted outputs were rendered from, in
// its property order and with its `errors` member dropped; every value in it is
// read off the pinned Go parse.
type matchFilesResult struct {
	Options             *collections.OrderedMap[string, json.Value] `json:"options"`
	FileNames           []string                                    `json:"fileNames"`
	TypeAcquisition     matchFilesTypeAcquisition                   `json:"typeAcquisition"`
	Raw                 any                                         `json:"raw"`
	WildcardDirectories *collections.OrderedMap[string, string]     `json:"wildcardDirectories"`
	CompileOnSave       bool                                        `json:"compileOnSave"`
}

// matchFilesTypeAcquisition is the carried shape of TypeScript's
// `TypeAcquisition`, whose three members are always present. Go's
// `core.TypeAcquisition` tags all of its fields `omitzero`, so marshalling it
// directly would drop all three; the values are still the pinned parse's.
type matchFilesTypeAcquisition struct {
	Enable  bool     `json:"enable"`
	Include []string `json:"include"`
	Exclude []string `json:"exclude"`
}

func matchFilesTypeAcquisitionOf(acquisition *core.TypeAcquisition) matchFilesTypeAcquisition {
	rendered := matchFilesTypeAcquisition{Include: []string{}, Exclude: []string{}}
	if acquisition == nil {
		return rendered
	}
	rendered.Enable = acquisition.Enable == core.TSTrue
	if acquisition.Include != nil {
		rendered.Include = acquisition.Include
	}
	if acquisition.Exclude != nil {
		rendered.Exclude = acquisition.Exclude
	}
	return rendered
}

// matchFilesWildcardDirectories renders the pinned
// `ParsedCommandLine.WildcardDirectories()` map. Its values are TypeScript's
// stringified `WatchDirectoryFlags`, which this pin has no equivalent of.
//
// Its keys carry an order the Go map cannot. TypeScript's `wildcardDirectories`
// is an object literal filled by the `for (const file of include)` loop at
// commandLineParser.ts:4132-4151, so its key order is that loop's insertion
// order, and the frozen baselines record it; the pinned Go port runs the same
// loop (wildcarddirectories.go:35-39) but returns a `map[string]bool`, which
// has no order to return. `order` is that insertion order recovered from the
// include specs the pinned calculation was given -- not a second calculation of
// which directories are watched -- by the overlay's in-package
// `Phase1WildcardDirectoryOrder`. When the recovery declined, `order` is empty
// and the keys fall back to sorted, which is what this section rendered before
// the order was recoverable; the row's `wildcard_order_source` records which of
// the two it was.
func matchFilesWildcardDirectories(directories map[string]bool, order []string) *collections.OrderedMap[string, string] {
	keys := order
	if len(keys) != len(directories) {
		keys = slices.Sorted(maps.Keys(directories))
	}
	rendered := collections.NewOrderedMapWithSizeHint[string, string](len(directories))
	for _, key := range keys {
		flag := "WatchDirectoryFlags.None"
		if directories[key] {
			flag = "WatchDirectoryFlags.Recursive"
		}
		rendered.Set(key, flag)
	}
	return rendered
}

// matchFilesOptions renders the `options` member. Its values are exactly what
// `json.Marshal` makes of the pinned `*core.CompilerOptions`; only the member
// order is carried. TypeScript's `options` is an object whose members were
// inserted as `convertCompilerOptionsFromJsonWorker` walked the config's
// `compilerOptions` in source order, with `configFilePath` assigned last, while
// Go's struct carries no order at all. The order is therefore recovered from
// the config's own key order -- an input section -- and never from the frozen
// `Result`.
func matchFilesOptions(options *core.CompilerOptions, raw any) (*collections.OrderedMap[string, json.Value], error) {
	encoded, err := json.Marshal(options)
	if err != nil {
		return nil, err
	}
	marshalled := &collections.OrderedMap[string, json.Value]{}
	if err := json.Unmarshal(encoded, marshalled); err != nil {
		return nil, err
	}
	var declared []string
	if rawMap, ok := raw.(*collections.OrderedMap[string, any]); ok {
		if compilerOptions, ok := rawMap.GetOrZero("compilerOptions").(*collections.OrderedMap[string, any]); ok {
			declared = slices.Collect(compilerOptions.Keys())
		}
	}
	ordered := collections.NewOrderedMapWithSizeHint[string, json.Value](marshalled.Size())
	for _, name := range declared {
		if value, ok := marshalled.Get(name); ok {
			ordered.Set(name, value)
		}
	}
	for name, value := range marshalled.Entries() {
		if !ordered.Has(name) {
			ordered.Set(name, value)
		}
	}
	return ordered, nil
}

// matchFilesWriteErrors renders the `Errors::` section. Each diagnostic is
// written by the pinned per-diagnostic writer; only the newline discipline is
// carried. TypeScript's `formatDiagnosticsWithColorAndContext` ends every
// diagnostic with a newline, while the pinned Go plural writer emits one
// *between* diagnostics and the singular writer already emits one after a code
// snippet, so supplying the newline the pinned writer does not write reproduces
// TypeScript's layout without reimplementing any formatting.
func matchFilesWriteErrors(output io.Writer, diags []diagnosticwriter.Diagnostic, formatOpts *diagnosticwriter.FormattingOptions) {
	for _, diagnostic := range diags {
		diagnosticwriter.FormatDiagnosticWithColorAndContext(output, diagnostic, formatOpts)
		// The exact condition under which the pinned singular writer already
		// ended the diagnostic with a newline; see diagnosticwriter.go:239-244.
		if diagnostic.File() != nil && diagnostic.Code() != diagnostics.File_appears_to_be_binary.Code() {
			continue
		}
		fmt.Fprint(output, formatOpts.NewLine)
	}
}

func matchFilesRender(inputs matchFilesInputs, host *tsoptionstest.VfsParseConfigHost, basePath string, parsed *tsoptions.ParsedCommandLine, wildcards map[string]bool, wildcardOrder []string) (string, error) {
	var out strings.Builder
	out.WriteString("config:\n")
	out.WriteString(inputs.configText)
	out.WriteString("\n")

	out.WriteString("Fs::\n")
	if err := matchFilesPrintFS(&out, host.FS(), inputs); err != nil {
		return "", err
	}
	out.WriteString("\n")

	out.WriteString("configFileName:: ")
	out.WriteString(inputs.configFileName)
	out.WriteString("\n")

	fileNames := parsed.ParsedConfig.FileNames
	if fileNames == nil {
		fileNames = []string{}
	}
	compileOnSave := parsed.CompileOnSave != nil && *parsed.CompileOnSave
	options, err := matchFilesOptions(parsed.ParsedConfig.CompilerOptions, parsed.Raw)
	if err != nil {
		return "", err
	}
	result := matchFilesResult{
		Options:             options,
		FileNames:           fileNames,
		TypeAcquisition:     matchFilesTypeAcquisitionOf(parsed.ParsedConfig.TypeAcquisition),
		Raw:                 parsed.Raw,
		WildcardDirectories: matchFilesWildcardDirectories(wildcards, wildcardOrder),
		CompileOnSave:       compileOnSave,
	}
	out.WriteString("Result\n")
	if err := json.MarshalIndentWrite(&out, result, "", "  "); err != nil {
		return "", err
	}
	out.WriteString("\n")

	out.WriteString("Errors::\n")
	matchFilesWriteErrors(&out, diagnosticwriter.FromASTDiagnostics(parsed.Errors), &diagnosticwriter.FormattingOptions{
		NewLine: "\r\n",
		ComparePathsOptions: tspath.ComparePathsOptions{
			CurrentDirectory:          basePath,
			UseCaseSensitiveFileNames: host.FS().UseCaseSensitiveFileNames(),
		},
	})
	out.WriteString("\n")
	return out.String(), nil
}

func matchFilesDigest(text string) string {
	sum := sha256.Sum256([]byte(text))
	return hex.EncodeToString(sum[:])
}

// matchFilesFirstDifference locates the first differing byte and quotes a short
// window of each side. It runs only after `reproduced` has already been decided
// from the whole bytes, so it cannot launder a mismatch into a match.
func matchFilesFirstDifference(rendered, expected string) map[string]any {
	limit := min(len(rendered), len(expected))
	offset := limit
	for index := range limit {
		if rendered[index] != expected[index] {
			offset = index
			break
		}
	}
	window := func(text string) string {
		start := max(offset-40, 0)
		end := min(offset+80, len(text))
		return text[start:end]
	}
	return map[string]any{
		"offset":   offset,
		"rendered": window(rendered),
		"expected": window(expected),
	}
}

// matchFilesObserve builds one case and renders it. It returns the row's result
// and payload; a recovered panic from a pinned entry point is reported, never
// swallowed and never turned into a rendered section.
func matchFilesObserve(request matchFilesRequest) (result string, payload map[string]any, reason string) {
	path := filepath.Join(repo.TestDataPath(), "baselines", "reference", filepath.FromSlash(request.Baseline))
	raw, err := os.ReadFile(path)
	if err != nil {
		return "harness_failed", nil, fmt.Sprintf("cannot read the frozen baseline: %v", err)
	}
	expected := string(raw)
	inputs, err := matchFilesReadInputs(expected)
	if err != nil {
		return "harness_failed", nil, fmt.Sprintf("cannot recover the inputs of %s: %v", request.Baseline, err)
	}

	files := map[string]string{}
	symlinks := map[string]string{}
	for _, entry := range inputs.entries {
		if entry.link {
			symlinks[entry.path] = entry.target
			continue
		}
		files[entry.path] = entry.content
	}
	basePath := tspath.GetNormalizedAbsolutePath(tspath.GetDirectoryPath(inputs.configFileName), "")
	host := tsoptionstest.NewVFSParseConfigHostWithSymlinks(files, symlinks, basePath, inputs.caseSensitive)
	config := testConfig{
		jsonText:       inputs.configText,
		configFileName: inputs.configFileName,
		basePath:       basePath,
	}

	var rendered string
	var renderErr error
	var unknown string
	var unavailable string
	var unavailableRaw string
	var unavailableCompileOnSave bool
	// How the `wildcardDirectories` section was obtained, recorded on every
	// row: the pinned accessor when the parse attached a ConfigFile, the
	// overlay's in-package hook when it did not.
	var wildcardSource string
	var wildcardInclude []string
	var wildcardExclude []string
	// How the section's key order was obtained, recorded on every row beside
	// the calculation's own source.
	var wildcardOrder []string
	var wildcardOrderSource string
	panicked := func() (recovered any) {
		defer func() {
			if value := recover(); value != nil {
				recovered = value
			}
		}()
		var parsed *tsoptions.ParsedCommandLine
		switch request.API {
		case "json":
			parsed = getParsedWithJsonApi(config, host, basePath)
		case "jsonSourceFile":
			parsed = getParsedWithJsonSourceFileApi(config, host, basePath)
		default:
			unknown = request.API
			return nil
		}
		var wildcards map[string]bool
		if parsed.ConfigFile == nil {
			// ParseJsonConfigFileContent passes a nil sourceFile to the worker,
			// so the result carries no ConfigFile and
			// ParsedCommandLine.WildcardDirectories() cannot be used: it reads
			// p.ConfigFile.configFileSpecs (parsedcommandline.go:258-273). The
			// specs are computed all the same -- tsconfigparsing.go:1328-1362
			// computes them unconditionally and only :1375-1377 gates their
			// attachment -- so the overlay's in-package hook re-derives them
			// from the parse's own result and hands them to the pinned
			// getWildcardDirectories (wildcarddirectories.go:10), which takes
			// the specs directly. Nothing about the section is invented here:
			// the calculation is the pinned one.
			var ok bool
			wildcards, wildcardInclude, wildcardExclude, ok = tsoptions.Phase1WildcardDirectoriesWithoutConfigFile(parsed)
			if !ok {
				// Record what the pinned parse *did* answer, so the gap stays
				// evidenced rather than asserted.
				if encoded, marshalErr := json.Marshal(parsed.Raw); marshalErr == nil {
					unavailableRaw = string(encoded)
				}
				unavailableCompileOnSave = parsed.CompileOnSave != nil && *parsed.CompileOnSave
				unavailable = "the overlay's in-package accessor refused this parse result"
				return nil
			}
			wildcardSource = "tsoptions.Phase1WildcardDirectoriesWithoutConfigFile -> getWildcardDirectories"
		} else {
			wildcards = parsed.WildcardDirectories()
			wildcardSource = "tsoptions.ParsedCommandLine.WildcardDirectories"
		}
		// The frozen `wildcardDirectories` records the pinned calculation's own
		// key insertion order; the Go port returns a map, which discards it.
		// Recover it from the include specs that calculation was given. On the
		// raw-JSON path those specs are already in hand above; on the
		// jsonSourceFile path they are the ones the parse attached to the
		// config source file, which is what the pinned accessor read
		// (parsedcommandline.go:258-273).
		if parsed.ConfigFile != nil {
			wildcardInclude, wildcardExclude, _ = tsoptions.Phase1ValidatedSpecsFromConfigFile(parsed)
		}
		var orderOK bool
		wildcardOrder, orderOK = tsoptions.Phase1WildcardDirectoryOrder(
			parsed, wildcardInclude, wildcardExclude, wildcards)
		wildcardOrderSource = "include specs, via tsoptions.Phase1WildcardDirectoryOrder"
		if !orderOK {
			// Not a repair: the section still renders, in the sorted order it
			// used before, and the row says the recovery declined.
			wildcardOrderSource = "sorted keys: the include-spec walk did not account for every key"
		}
		rendered, renderErr = matchFilesRender(inputs, host, basePath, parsed, wildcards, wildcardOrder)
		return nil
	}()
	if panicked != nil {
		return "native_unavailable", nil, fmt.Sprintf(
			"the pinned %s entry point cannot render this baseline: %v", request.EntryPoint, panicked)
	}
	if unknown != "" {
		return "harness_failed", nil, fmt.Sprintf("request names unknown entry point %q", unknown)
	}
	if unavailable != "" {
		return "native_unavailable", map[string]any{
			"baseline":        request.Baseline,
			"api":             request.API,
			"entry_point":     request.EntryPoint,
			"raw":             unavailableRaw,
			"compile_on_save": unavailableCompileOnSave,
		}, fmt.Sprintf("%s cannot render this baseline: %s", request.EntryPoint, unavailable)
	}
	if renderErr != nil {
		return "harness_failed", nil, fmt.Sprintf("cannot render %s: %v", request.Baseline, renderErr)
	}

	observation := map[string]any{
		"baseline":    request.Baseline,
		"api":         request.API,
		"entry_point": request.EntryPoint,
		"inputs": map[string]any{
			"configFileName":            inputs.configFileName,
			"basePath":                  basePath,
			"useCaseSensitiveFileNames": inputs.caseSensitive,
			"files":                     len(files),
			"symlinks":                  len(symlinks),
		},
		"wildcard_source":       wildcardSource,
		"wildcard_order_source": wildcardOrderSource,
		"rendered":              rendered,
		"rendered_bytes":        len(rendered),
		"rendered_sha256":       matchFilesDigest(rendered),
		"expected_bytes":        len(expected),
		"expected_sha256":       matchFilesDigest(expected),
		"reproduced":            rendered == expected,
	}
	if rendered != expected {
		observation["first_difference"] = matchFilesFirstDifference(rendered, expected)
	}
	if wildcardInclude != nil || wildcardExclude != nil {
		// The specs the pinned wildcard calculation was given, so a differing
		// row records its input as well as its output.
		observation["wildcard_specs"] = map[string]any{
			"include": wildcardInclude,
			"exclude": wildcardExclude,
		}
	}
	return "observed", observation, ""
}

func TestPhase1FilesystemMatchFiles(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []matchFilesRequest `json:"requests"`
	}
	if err := stdjson.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Operation != matchFilesOperation {
			row["result"] = "native_unavailable"
			row["reason"] = "operation is not served by the matchFiles baseline probe"
			observations = append(observations, row)
			continue
		}
		result, payload, reason := matchFilesObserve(request)
		row["result"] = result
		switch result {
		case "observed":
			row["observation"] = payload
		case "native_unavailable":
			row["reason"] = reason
			if payload != nil {
				// Extra evidence, not an observation: a declined case records
				// what the pinned parse could answer, never a rendered section.
				row["detail"] = payload
			}
		default:
			row["error"] = reason
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
	data, err := stdjson.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
