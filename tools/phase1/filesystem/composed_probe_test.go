package execute

// Access-only overlay: the production watchCompilerHost and source-file cache
// are reached in their own package. The scheduler itself is not executed.
import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/binder"
	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/cachedvfs"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/osvfs"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/trackingvfs"
)

type phase1ComposedRequest struct {
	Case         string            `json:"case"`
	Operation    string            `json:"operation"`
	Subject      string            `json:"subject"`
	Config       string            `json:"config"`
	InitialFiles map[string]string `json:"initial_files"`
	Mutations    []struct {
		Op   string `json:"op"`
		Path string `json:"path"`
		Text string `json:"text"`
	} `json:"mutations"`
	RetainedFile  string   `json:"retained_file"`
	UnchangedFile string   `json:"unchanged_file"`
	MetadataPaths []string `json:"metadata_paths"`
}
type phase1ComposedRoot struct{ name, real string }

func (r phase1ComposedRoot) join(relative string) string {
	if relative == "" || strings.ContainsAny(relative, "\\:") {
		panic("invalid fixture-relative path")
	}
	for _, part := range strings.Split(relative, "/") {
		if part == "" || part == "." || part == ".." {
			panic("invalid fixture-relative path")
		}
	}
	return r.name + "/" + relative
}
func (r phase1ComposedRoot) normalize(name string) string {
	name = tspath.NormalizePath(name)
	if r.real != r.name && (name == r.real || strings.HasPrefix(name, r.real+"/")) {
		return "<realroot>" + name[len(r.real):]
	}
	if name == r.name || strings.HasPrefix(name, r.name+"/") {
		return "<root>" + name[len(r.name):]
	}
	return name
}
func (r phase1ComposedRoot) write(t *testing.T, path, text string, generation int64) {
	t.Helper()
	path = r.join(path)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(text), 0o644); err != nil {
		t.Fatal(err)
	}
	// The native watch host observes mtime. Avoid filesystem timestamp granularity
	// or equal-size rewrites making this create/edit fixture accidentally unchanged.
	stamp := time.Unix(946684800+generation, 0)
	if err := os.Chtimes(path, stamp, stamp); err != nil {
		t.Fatal(err)
	}
}
func phase1ComposedBound(root phase1ComposedRoot, file *ast.SourceFile) any {
	binder.BindSourceFile(file)
	names := []string{}
	for name := range file.Locals {
		names = append(names, name)
	}
	slices.Sort(names)
	symbols := []any{}
	for _, name := range names {
		symbol := file.Locals[name]
		symbols = append(symbols, []any{name, symbol.Name, uint32(symbol.Flags)})
	}
	return map[string]any{"file": root.normalize(file.FileName()), "text": file.Text(), "symbols": symbols}
}
func phase1ComposedMetadata(root phase1ComposedRoot, paths []string, cached *cachedvfs.FS) []any {
	rows := []any{}
	for _, path := range paths {
		name := root.join(path)
		rows = append(rows, []any{root.normalize(name), cached.FileExists(name)})
	}
	return rows
}
func phase1ComposedBuild(t *testing.T, root phase1ComposedRoot, request phase1ComposedRequest, cache *collections.SyncMap[tspath.Path, *cachedSourceFile]) (*compiler.Program, *cachedvfs.FS, any) {
	t.Helper()
	// Config discovery uses the live OS host, then the watcher explicitly seeds
	// its wildcard directories and config path into the fresh tracking wrapper.
	configHost := compiler.NewCompilerHost(root.name, osvfs.FS(), root.join("lib"), nil, nil, nil)
	configName := root.join("tsconfig.json")
	configText, ok := osvfs.FS().ReadFile(configName)
	if !ok {
		t.Fatal("missing physical config")
	}
	source := tsoptions.NewTsconfigSourceFileFromFilePath(configName, tspath.ToPath(configName, root.name, osvfs.FS().UseCaseSensitiveFileNames()), configText)
	config := tsoptions.ParseJsonSourceFileConfigFileContent(source, configHost, root.name, nil, nil, configName, nil, nil)
	cached := cachedvfs.From(osvfs.FS())
	tracking := &trackingvfs.FS{Inner: cached}
	wildcard := []string{}
	for dir := range config.WildcardDirectories() {
		tracking.SeenFiles.Add(dir)
		wildcard = append(wildcard, root.normalize(dir))
	}
	slices.Sort(wildcard)
	tracking.SeenFiles.Add(root.join("tsconfig.json"))
	host := &watchCompilerHost{CompilerHost: compiler.NewCompilerHost(root.name, tracking, root.join("lib"), nil, nil, nil), cache: cache}
	program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host, SingleThreaded: core.TSTrue})
	files := slices.Clone(program.GetSourceFiles())
	slices.SortFunc(files, func(a, b *ast.SourceFile) int { return strings.Compare(a.FileName(), b.FileName()) })
	observedFiles := []any{}
	for _, file := range files {
		observedFiles = append(observedFiles, phase1ComposedBound(root, file))
	}
	syntactic := []any{}
	for _, d := range program.GetSyntacticDiagnostics(context.Background(), nil) {
		name := ""
		if d.File() != nil {
			name = root.normalize(d.File().FileName())
		}
		args := d.MessageArgs()
		if args == nil {
			args = []string{}
		}
		syntactic = append(syntactic, map[string]any{"file": name, "pos": d.Pos(), "end": d.End(), "code": d.Code(), "category": int(d.Category()), "key": string(d.MessageKey()), "args": args})
	}
	metadata := phase1ComposedMetadata(root, request.MetadataPaths, cached)
	seen := tracking.SeenFiles.ToSlice()
	for i := range seen {
		seen[i] = root.normalize(seen[i])
	}
	slices.Sort(seen)
	cached.DisableAndClearCache()
	return program, cached, map[string]any{"files": observedFiles, "syntactic": syntactic, "seen": seen, "wildcard_directories": wildcard, "metadata": metadata}
}
func phase1ComposedObserve(t *testing.T, request phase1ComposedRequest) any {
	name := tspath.NormalizePath(t.TempDir())
	root := phase1ComposedRoot{name: name, real: osvfs.FS().Realpath(name)}
	root.write(t, "tsconfig.json", request.Config, 0)
	for path, text := range request.InitialFiles {
		root.write(t, path, text, 0)
	}
	cache := &collections.SyncMap[tspath.Path, *cachedSourceFile]{}
	first, firstCache, firstRow := phase1ComposedBuild(t, root, request, cache)
	retained := first.GetSourceFile(root.join(request.RetainedFile))
	unchanged := first.GetSourceFile(root.join(request.UnchangedFile))
	if retained == nil || unchanged == nil {
		t.Fatal("missing first retained/unchanged file")
	}
	for i, mutation := range request.Mutations {
		switch mutation.Op {
		case "write":
			root.write(t, mutation.Path, mutation.Text, int64(i+1))
		case "remove":
			if err := os.Remove(root.join(mutation.Path)); err != nil {
				t.Fatal(err)
			}
		default:
			t.Fatalf("unknown composed mutation %q", mutation.Op)
		}
	}
	afterClear := phase1ComposedMetadata(root, request.MetadataPaths, firstCache)
	second, _, secondRow := phase1ComposedBuild(t, root, request, cache)
	reused := second.GetSourceFile(root.join(request.UnchangedFile)) == unchanged
	replaced := second.GetSourceFile(root.join(request.RetainedFile)) != retained
	first, second, cache = nil, nil, nil
	runtime.GC()
	return map[string]any{"ordered": []any{firstRow, secondRow}, "after_disable_and_clear": afterClear, "unchanged_reused": reused, "edited_replaced": replaced, "retained_after_programs_dropped": phase1ComposedBound(root, retained)}
}
func TestPhase1FilesystemComposed(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []phase1ComposedRequest `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}
	rows := []any{}
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Subject != "composed.ProgramRebuild" {
			row["result"] = "native_unavailable"
			row["reason"] = fmt.Sprintf("unsupported composed subject %q", request.Subject)
		} else {
			row["result"] = "observed"
			row["observation"] = phase1ComposedObserve(t, request)
		}
		rows = append(rows, row)
	}
	hash := sha256.Sum256(input)
	output := map[string]any{"request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "version": 1, "observations": rows}
	data, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
