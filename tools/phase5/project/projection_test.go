// Carried, test-only data adapter. check.py appends the three pinned state
// writer methods, changing only their receiver and the snapshot/program types.
package fourslash

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"iter"
	"maps"
	"os"
	"reflect"
	"slices"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/lsp/lsproto"
	"github.com/microsoft/TypeScript/tsc/internal/project"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/projecttestutil"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs"
)

type wireID = json.RawMessage

func identity(pointer any) wireID { data, _ := json.Marshal(fmt.Sprintf("%p", pointer)); return data }

type fileData struct {
	ID   wireID      `json:"id"`
	Name string      `json:"fileName"`
	Path tspath.Path `json:"path"`
}
type projectData struct {
	Name    string     `json:"name"`
	Program wireID     `json:"program"`
	Files   []fileData `json:"files"`
}
type openData struct {
	Name     string   `json:"fileName"`
	Default  string   `json:"defaultProject"`
	Projects []string `json:"projects"`
}
type configData struct {
	Path     tspath.Path   `json:"path"`
	Name     string        `json:"fileName"`
	Projects []tspath.Path `json:"retainingProjects"`
	Files    []tspath.Path `json:"retainingOpenFiles"`
	Configs  []tspath.Path `json:"retainingConfigs"`
}
type namesData struct {
	Path      tspath.Path       `json:"path"`
	Nearest   string            `json:"nearest"`
	Ancestors map[string]string `json:"ancestors"`
}
type stateData struct {
	Version  int           `json:"version"`
	Projects []projectData `json:"projects"`
	Open     []openData    `json:"openFiles"`
	Registry wireID        `json:"registry"`
	Configs  []configData  `json:"configs"`
	Names    []namesData   `json:"configFileNames"`
}

func projectState(snapshot *project.Snapshot, open map[string]struct{}, fs vfs.FS) stateData {
	result := stateData{Version: 1, Registry: identity(snapshot.ProjectCollection.ConfigFileRegistry())}
	for _, p := range snapshot.ProjectCollection.Projects() {
		row := projectData{Name: p.Name(), Program: identity(p.GetProgram())}
		if program := p.GetProgram(); program != nil {
			for _, f := range program.GetSourceFiles() {
				row.Files = append(row.Files, fileData{identity(f), f.FileName(), f.Path()})
			}
		}
		result.Projects = append(result.Projects, row)
	}
	for file := range open {
		row := openData{Name: file}
		path := tspath.ToPath(file, "/", fs.UseCaseSensitiveFileNames())
		if p := snapshot.ProjectCollection.GetDefaultProject(path); p != nil {
			row.Default = p.Name()
		}
		for _, p := range snapshot.ProjectCollection.Projects() {
			if program := p.GetProgram(); program != nil && program.GetSourceFileByPath(path) != nil {
				row.Projects = append(row.Projects, p.Name())
			}
		}
		slices.Sort(row.Projects)
		result.Open = append(result.Open, row)
	}
	registry := snapshot.ProjectCollection.ConfigFileRegistry()
	registry.ForEachTestConfigEntry(func(path tspath.Path, e *project.TestConfigEntry) {
		result.Configs = append(result.Configs, configData{path, e.FileName, slices.Collect(e.RetainingProjects), slices.Collect(e.RetainingOpenFiles), slices.Collect(e.RetainingConfigs)})
	})
	registry.ForEachTestConfigFileNamesEntry(func(path tspath.Path, e *project.TestConfigFileNamesEntry) {
		result.Names = append(result.Names, namesData{path, e.NearestConfigFileName, e.Ancestors})
	})
	return result
}

type wireFile struct{ data fileData }

func (f *wireFile) FileName() string  { return f.data.Name }
func (f *wireFile) Path() tspath.Path { return f.data.Path }

type wireProgram struct{ files []*wireFile }

func (p *wireProgram) GetSourceFiles() []*wireFile { return p.files }
func (p *wireProgram) GetSourceFileByPath(path tspath.Path) *wireFile {
	for _, f := range p.files {
		if f.Path() == path {
			return f
		}
	}
	return nil
}

type wireProject struct {
	name    string
	program *wireProgram
}

func (p *wireProject) Name() string             { return p.name }
func (p *wireProject) GetProgram() *wireProgram { return p.program }

type wireRegistry struct {
	configs map[tspath.Path]configData
	names   map[tspath.Path]namesData
}

func (r *wireRegistry) GetTestConfigEntry(path tspath.Path) *project.TestConfigEntry {
	if r != nil {
		if e, ok := r.configs[path]; ok {
			return &project.TestConfigEntry{FileName: e.Name, RetainingProjects: slices.Values(e.Projects), RetainingOpenFiles: slices.Values(e.Files), RetainingConfigs: slices.Values(e.Configs)}
		}
	}
	return nil
}
func (r *wireRegistry) GetTestConfigFileNamesEntry(path tspath.Path) *project.TestConfigFileNamesEntry {
	if r != nil {
		if e, ok := r.names[path]; ok {
			return &project.TestConfigFileNamesEntry{NearestConfigFileName: e.Nearest, Ancestors: e.Ancestors}
		}
	}
	return nil
}
func (r *wireRegistry) ForEachTestConfigEntry(fn func(tspath.Path, *project.TestConfigEntry)) {
	if r != nil {
		for path := range r.configs {
			fn(path, r.GetTestConfigEntry(path))
		}
	}
}
func (r *wireRegistry) ForEachTestConfigFileNamesEntry(fn func(tspath.Path, *project.TestConfigFileNamesEntry)) {
	if r != nil {
		for path := range r.names {
			fn(path, r.GetTestConfigFileNamesEntry(path))
		}
	}
}

type wireCollection struct {
	projects []*wireProject
	defaults map[tspath.Path]*wireProject
	registry *wireRegistry
}

func (c *wireCollection) Projects() []*wireProject                        { return c.projects }
func (c *wireCollection) GetDefaultProject(path tspath.Path) *wireProject { return c.defaults[path] }
func (c *wireCollection) ConfigFileRegistry() *wireRegistry               { return c.registry }

type wireSnapshot struct{ ProjectCollection *wireCollection }
type projectionBaseline struct {
	serializedProjects           map[string]*wireProgram
	serializedOpenFiles          map[string]*openFileInfo
	serializedConfigFileRegistry *wireRegistry
}
type projectionWriter struct {
	stateBaseline *projectionBaseline
	openFiles     map[string]struct{}
	vfs           vfs.FS
}
type projectionDecoder struct {
	programs   map[string]*wireProgram
	files      map[string]*wireFile
	registries map[string]*wireRegistry
}

func newProjectionDecoder() *projectionDecoder {
	return &projectionDecoder{map[string]*wireProgram{}, map[string]*wireFile{}, map[string]*wireRegistry{}}
}
func (d *projectionDecoder) decode(t *testing.T, data stateData, fs vfs.FS) *wireSnapshot {
	t.Helper()
	if data.Version != 1 {
		t.Fatal("unsupported projection")
	}
	c := &wireCollection{defaults: map[tspath.Path]*wireProject{}}
	for _, p := range data.Projects {
		var files []*wireFile
		for _, f := range p.Files {
			key := string(f.ID)
			if key == "" {
				t.Fatal("missing source identity")
			}
			old := d.files[key]
			if old == nil {
				old = &wireFile{f}
				d.files[key] = old
			} else if !reflect.DeepEqual(old.data, f) {
				t.Fatal("source identity reused for different data")
			}
			files = append(files, old)
		}
		key := string(p.Program)
		var program *wireProgram
		if key != "null" && key != `"0x0"` {
			program = d.programs[key]
			if program == nil {
				program = &wireProgram{files}
				d.programs[key] = program
			} else if !slices.Equal(program.files, files) {
				t.Fatal("program identity reused for different files")
			}
		} else if len(files) != 0 {
			t.Fatal("delayed project has source files")
		}
		c.projects = append(c.projects, &wireProject{p.Name, program})
	}
	for _, f := range data.Open {
		path := tspath.ToPath(f.Name, "/", fs.UseCaseSensitiveFileNames())
		for _, p := range c.projects {
			if p.Name() == f.Default {
				c.defaults[path] = p
			}
		}
	}
	registry := &wireRegistry{configs: map[tspath.Path]configData{}, names: map[tspath.Path]namesData{}}
	for _, e := range data.Configs {
		slices.Sort(e.Projects)
		slices.Sort(e.Files)
		slices.Sort(e.Configs)
		registry.configs[e.Path] = e
	}
	for _, e := range data.Names {
		registry.names[e.Path] = e
	}
	key := string(data.Registry)
	if old := d.registries[key]; old != nil {
		if !reflect.DeepEqual(old, registry) {
			t.Fatal("registry identity reused for different data")
		}
		registry = old
	} else {
		d.registries[key] = registry
	}
	c.registry = registry
	return &wireSnapshot{c}
}

type action struct {
	Kind, File, Text string
	Version          int32
	Options          *core.CompilerOptions
}
type scenario struct {
	Files   map[string]any
	Actions []action
}
type result struct {
	States    []stateData `json:"states"`
	Baselines []string    `json:"baselines"`
}

func readJSON(t *testing.T, path string, value any) {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if err = json.Unmarshal(data, value); err != nil {
		t.Fatal(err)
	}
}
func writeJSON(t *testing.T, path string, value any) {
	t.Helper()
	data, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(path, data, 0600); err != nil {
		t.Fatal(err)
	}
}
func renderProjection(t *testing.T, writer *projectionWriter, decoder *projectionDecoder, data stateData) string {
	writer.openFiles = map[string]struct{}{}
	for _, f := range data.Open {
		writer.openFiles[f.Name] = struct{}{}
	}
	snapshot := decoder.decode(t, data, writer.vfs)
	var output strings.Builder
	writer.printProjectsDiff(t, snapshot, &output)
	writer.printOpenFilesDiff(t, snapshot, &output)
	writer.printConfigFileRegistryDiff(t, snapshot, &output)
	return output.String()
}
func TestRustProjectStateNative(t *testing.T) {
	var fixture scenario
	readJSON(t, os.Getenv("TSR_PROJECT_CASES"), &fixture)
	init, _ := projecttestutil.GetSessionInitOptions(fixture.Files, &project.SessionOptions{CurrentDirectory: "/", DefaultLibraryPath: "/", PositionEncoding: lsproto.PositionEncodingKindUTF16}, &projecttestutil.TypingsInstallerOptions{})
	session := project.NewSession(init)
	defer session.Close()
	native := &FourslashTest{stateBaseline: &stateBaseline{}, openFiles: map[string]struct{}{}, vfs: init.FS}
	projected := &projectionWriter{stateBaseline: &projectionBaseline{}, vfs: init.FS}
	decoder := newProjectionDecoder()
	output := result{}
	for _, step := range fixture.Actions {
		uri := lsproto.DocumentUri("file://" + step.File)
		switch step.Kind {
		case "open":
			session.DidOpenFile(context.Background(), uri, step.Version, step.Text, lsproto.LanguageKindTypeScript)
			native.openFiles[step.File] = struct{}{}
		case "change":
			session.DidChangeFile(context.Background(), uri, step.Version, []lsproto.TextDocumentContentChangePartialOrWholeDocument{{WholeDocument: &lsproto.TextDocumentContentChangeWholeDocument{Text: step.Text}}})
		case "close":
			session.DidCloseFile(context.Background(), uri)
			delete(native.openFiles, step.File)
		case "options":
			session.DidChangeCompilerOptionsForInferredProjects(context.Background(), step.Options)
		case "state":
			if step.File != "" {
				if _, err := session.GetLanguageService(context.Background(), uri); err != nil {
					t.Fatal(err)
				}
			}
			snapshot := session.Snapshot()
			var original strings.Builder
			native.printProjectsDiff(t, snapshot, &original)
			native.printOpenFilesDiff(t, snapshot, &original)
			native.printConfigFileRegistryDiff(t, snapshot, &original)
			projectedData := projectState(snapshot, native.openFiles, init.FS)
			// Exercise the actual JSON boundary rather than sharing pointer wrappers.
			bytes, _ := json.Marshal(projectedData)
			var roundtrip stateData
			if err := json.Unmarshal(bytes, &roundtrip); err != nil {
				t.Fatal(err)
			}
			if adapted := renderProjection(t, projected, decoder, roundtrip); adapted != original.String() {
				t.Fatalf("projected native writer differs:\n%s\nORIGINAL:\n%s", adapted, original.String())
			}
			output.States = append(output.States, roundtrip)
			output.Baselines = append(output.Baselines, original.String())
		default:
			t.Fatalf("unknown action %q", step.Kind)
		}
	}
	writeJSON(t, os.Getenv("TSR_PROJECT_OUTPUT"), output)
}
func TestRustProjectStateRender(t *testing.T) {
	if os.Getenv("TSR_PROJECT_RENDER") == "" {
		t.Skip("render mode only")
	}
	var data []stateData
	readJSON(t, os.Getenv("TSR_PROJECT_RENDER"), &data)
	init, _ := projecttestutil.GetSessionInitOptions(map[string]any{}, &project.SessionOptions{CurrentDirectory: "/"}, &projecttestutil.TypingsInstallerOptions{})
	writer := &projectionWriter{stateBaseline: &projectionBaseline{}, vfs: init.FS}
	decoder := newProjectionDecoder()
	var output []string
	for _, state := range data {
		output = append(output, renderProjection(t, writer, decoder, state))
	}
	writeJSON(t, os.Getenv("TSR_PROJECT_OUTPUT"), output)
}
