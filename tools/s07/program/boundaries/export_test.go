package compiler

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

// This supplement reuses the ordinary loader probe's observation types. It
// changes only host construction, to expose custom library path boundaries.
func TestS07ProgramBoundaries(t *testing.T) {
	var requests []struct {
		s07Request
		DefaultLibraryPath string `json:"default_library_path"`
	}
	raw, err := os.ReadFile(os.Getenv("S07_BOUNDARY_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(raw, &requests); err != nil {
		t.Fatal(err)
	}
	rows := []s07Observation{}
	for _, request := range requests {
		files := map[string]any{}
		for name, encoded := range request.Files {
			contents, err := hex.DecodeString(encoded)
			if err != nil {
				t.Fatal(err)
			}
			files[name] = contents
		}
		libraryPath := request.DefaultLibraryPath
		if libraryPath == "" {
			libraryPath = bundled.LibPath()
		}
		fs := bundled.WrapFS(vfstest.FromMap(files, request.CaseSensitive))
		host := NewCompilerHost(request.Cwd, fs, libraryPath, nil, nil, nil)
		config := tsoptions.NewParsedCommandLine(&request.Options, request.Roots, nil, tspath.ComparePathsOptions{CurrentDirectory: request.Cwd, UseCaseSensitiveFileNames: request.CaseSensitive})
		opts := ProgramOptions{Host: host, Config: config, SkipModuleResolution: request.SkipModuleResolution}
		loaded := processAllProgramFiles(opts, true)
		program := &Program{opts: opts, processedFiles: loaded}
		row := s07Observation{ID: request.ID, Files: []s07File{}, Missing: loaded.missingFiles, Resolutions: []s07Resolution{}, TypeResolutions: []s07TypeResolution{}}
		for _, file := range program.GetSourceFiles() {
			hash := sha256.Sum256([]byte(file.Text()))
			observed := s07File{Name: file.FileName(), Path: string(file.Path()), SHA256: hex.EncodeToString(hash[:]), Bytes: len(file.Text()), Meta: loaded.sourceFileMetaDatas[file.Path()], Lib: loaded.libFiles[file.Path()] != nil, Imports: []string{}, External: ast.IsExternalModule(file)}
			for _, node := range file.Imports() {
				observed.Imports = append(observed.Imports, node.Text())
			}
			row.Files = append(row.Files, observed)
		}
		for _, resolutions := range loaded.resolvedModules {
			if len(resolutions) != 0 {
				t.Fatalf("%s unexpectedly performed module resolution", request.ID)
			}
		}
		for _, resolutions := range loaded.typeResolutionsInFile {
			if len(resolutions) != 0 {
				t.Fatalf("%s unexpectedly performed type resolution", request.ID)
			}
		}
		for _, diagnostic := range loaded.includeProcessor.getDiagnostics(program).GetDiagnostics() {
			row.Diagnostics = append(row.Diagnostics, s07Diag(diagnostic))
		}
		rows = append(rows, row)
	}
	output, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S07_BOUNDARY_OUTPUT"), append(output, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}
