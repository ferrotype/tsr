// Phase 1 operation tables: group tsoptions (tsoptions ports, computeFn
// through the bridge in bridges/tsoptions). Columns are registered in init;
// the Rust side is tools/phase1/mutation/driver/src/table/tsoptions.rs and
// the spec is data/phase1/tables/tsoptions.json.
package main

import (
	"encoding/json"
	"reflect"
	"slices"

	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions/tsoptionstest"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

// hexes projects strings as hex (a nil list stays nil).
func hexes(texts []string) any {
	if texts == nil {
		return nil
	}
	out := []any{}
	for _, text := range texts {
		out = append(out, Hex(text))
	}
	return out
}

// optionsOf parses a tsconfig text's compiler options as ParseConfig does
// (under /p, with no other files); nil text is nil options.
func optionsOf(text *string) *core.CompilerOptions {
	if text == nil {
		return nil
	}
	host := tsoptionstest.NewVFSParseConfigHost(map[string]string{}, "/p", true)
	source := tsoptions.NewTsconfigSourceFileFromFilePath("/p/tsconfig.json", tspath.ToPath("/p/tsconfig.json", "/p", true), *text)
	return tsoptions.ParseJsonSourceFileConfigFileContent(source, host, "/p", nil, nil, "/p/tsconfig.json", nil, nil).CompilerOptions()
}

type affectCase struct {
	Old  *string `json:"old"`
	New  *string `json:"new"`
	Same bool    `json:"same"`
}

// affectColumn runs one Affects predicate over option pairs (the same
// options where the case says so).
func affectColumn(affect func(old, new *core.CompilerOptions) bool) func(in struct {
	Cases []affectCase `json:"cases"`
}) any {
	return func(in struct {
		Cases []affectCase `json:"cases"`
	}) any {
		out := []any{}
		for _, c := range in.Cases {
			old := optionsOf(c.Old)
			new := optionsOf(c.New)
			if c.Same {
				new = old
			}
			out = append(out, affect(old, new))
		}
		return out
	}
}

// showValue projects a --showConfig option value: strings as hex, integers
// as scalars, lists and path maps element by element.
func showValue(value any) any {
	switch v := value.(type) {
	case bool:
		return v
	case string:
		return Hex(v)
	case []string:
		return hexes(v)
	case *int:
		return Scalar(*v)
	case *collections.OrderedMap[string, []string]:
		out := []any{}
		for key, values := range v.Entries() {
			out = append(out, []any{Hex(key), hexes(values)})
		}
		return out
	}
	if rv := reflect.ValueOf(value); rv.CanInt() {
		return Scalar(int(rv.Int()))
	}
	fatal("showValue: unsupported %T", value)
	return nil
}

// configColumn is a column over a parsed tsconfig (the config input) and its
// args; the value is f(parsed, args).
func configColumn[A any](id string, f func(parsed *tsoptions.ParsedCommandLine, args A) any) Column {
	return Column{
		ID:    id,
		Input: "config",
		Build: func(raw json.RawMessage) (func() any, error) {
			parsed, in, err := ParseConfig(raw)
			if err != nil {
				return nil, err
			}
			var args A
			if err := json.Unmarshal(in.Args, &args); err != nil {
				return nil, err
			}
			return func() any { return f(parsed, args) }, nil
		},
	}
}

func init() {
	Register("tsoptions",
		configColumn("tsoptions.ParsedCommandLine.LiteralFileNames", func(parsed *tsoptions.ParsedCommandLine, _ any) any {
			return hexes(parsed.LiteralFileNames())
		}),
		// With the args' file names (null for nil): [the copy's file names,
		// its literal file names, the original's file names].
		configColumn("tsoptions.ParsedCommandLine.WithFileNames", func(parsed *tsoptions.ParsedCommandLine, args []string) any {
			copied := parsed.WithFileNames(args)
			return []any{hexes(copied.FileNames()), hexes(copied.LiteralFileNames()), hexes(parsed.FileNames())}
		}),
		configColumn("tsoptions.ParsedCommandLine.GetOutputFileNames", func(parsed *tsoptions.ParsedCommandLine, _ any) any {
			return hexes(slices.Collect(parsed.GetOutputFileNames()))
		}),
		configColumn("tsoptions.ParsedCommandLine.PossiblyMatchesFileName", func(parsed *tsoptions.ParsedCommandLine, args []string) any {
			out := []any{}
			for _, name := range args {
				out = append(out, parsed.PossiblyMatchesFileName(name))
			}
			return out
		}),
		// Per probe file name, whether any glob matches, then the glob count
		// (Go ranges over the wildcard directory map in random order).
		configColumn("tsoptions.ParsedCommandLine.WildcardDirectoryGlobs", func(parsed *tsoptions.ParsedCommandLine, args []string) any {
			globs := parsed.WildcardDirectoryGlobs()
			out := []any{}
			for _, name := range args {
				matched := false
				for _, glob := range globs {
					matched = matched || glob.Match(name)
				}
				out = append(out, matched)
			}
			return append(out, Scalar(len(globs)))
		}),
		// ParseBuildCommandLine over the config input's files with the args
		// as the command line: its resolved project paths.
		Column{
			ID:    "tsoptions.ParsedBuildCommandLine.ResolvedProjectPaths",
			Input: "config",
			Build: func(raw json.RawMessage) (func() any, error) {
				var in ConfigInput
				if err := DecodeInput(raw, &in); err != nil {
					return nil, err
				}
				var args []string
				if err := json.Unmarshal(in.Args, &args); err != nil {
					return nil, err
				}
				host := tsoptionstest.NewVFSParseConfigHost(in.Files, in.CurrentDirectory, in.CaseSensitive)
				parsed := tsoptions.ParseBuildCommandLine(args, host)
				return func() any { return hexes(parsed.ResolvedProjectPaths()) }, nil
			},
		},
		typedValuesColumn("tsoptions.CompilerOptionsAffectEmit", affectColumn(tsoptions.CompilerOptionsAffectEmit)),
		typedValuesColumn("tsoptions.CompilerOptionsAffectDeclarationPath", affectColumn(tsoptions.CompilerOptionsAffectDeclarationPath)),
		typedValuesColumn("tsoptions.CompilerOptionsAffectSemanticDiagnostics", affectColumn(tsoptions.CompilerOptionsAffectSemanticDiagnostics)),
		// Over the semantic-diagnostics options of each config: [whether a
		// call stopping at stop_at stopped, [[name hex, zero, field index],
		// ...] of the fields it visited].
		typedValuesColumn("tsoptions.ForEachCompilerOptionValue", func(in struct {
			Configs []string `json:"configs"`
			StopAt  string   `json:"stop_at"`
		}) any {
			out := []any{}
			for _, text := range in.Configs {
				visited := []any{}
				stopped := tsoptions.ForEachCompilerOptionValue(optionsOf(&text),
					func(option *tsoptions.CommandLineOption) bool { return option.AffectsSemanticDiagnostics },
					func(option *tsoptions.CommandLineOption, value reflect.Value, i int) bool {
						visited = append(visited, []any{Hex(option.Name), value.IsZero(), Scalar(i)})
						return option.Name == in.StopAt
					})
				out = append(out, []any{stopped, visited})
			}
			return out
		}),
		// ConvertToTSConfig with the args' config file name: [[[option hex,
		// value], ...] in order, references [[path hex, circular]], files,
		// include, exclude (hex lists or null), compileOnSave (or null)].
		configColumn("tsoptions.ConvertToTSConfig", func(parsed *tsoptions.ParsedCommandLine, name string) any {
			config := tsoptions.ConvertToTSConfig(parsed, name)
			options := []any{}
			for key, value := range config.CompilerOptions.Entries() {
				options = append(options, []any{Hex(key), showValue(value)})
			}
			var references any
			if config.References != nil {
				list := []any{}
				for _, reference := range config.References {
					ref := reference.(*collections.OrderedMap[string, any])
					path, _ := ref.Get("path")
					circular, _ := ref.Get("circular")
					list = append(list, []any{Hex(path.(string)), circular == true})
				}
				references = list
			}
			var compileOnSave any
			if config.CompileOnSave != nil {
				compileOnSave = *config.CompileOnSave
			}
			return []any{options, references, hexes(config.Files), hexes(config.Include), hexes(config.Exclude), compileOnSave}
		}),
		// computeFn (through the bridge) over a module-kind and a bool
		// getter, per config: [emit module kind, allowJs].
		typedValuesColumn("tsoptions.computeFn", func(in struct {
			Configs []string `json:"configs"`
		}) any {
			moduleKind := tsoptions.Phase1ComputeFnInt((*core.CompilerOptions).GetEmitModuleKind)
			allowJs := tsoptions.Phase1ComputeFnBool((*core.CompilerOptions).GetAllowJS)
			out := []any{}
			for _, text := range in.Configs {
				options := optionsOf(&text)
				out = append(out, []any{Scalar(int(moduleKind(options).(core.ModuleKind))), allowJs(options)})
			}
			return out
		}),
		// ParseExtendedConfig over the config input's files, for each file
		// name of the args: its ExtendedFileNames.
		Column{
			ID:    "tsoptions.ExtendedConfigCacheEntry.ExtendedFileNames",
			Input: "config",
			Build: func(raw json.RawMessage) (func() any, error) {
				var in ConfigInput
				if err := DecodeInput(raw, &in); err != nil {
					return nil, err
				}
				var names []string
				if err := json.Unmarshal(in.Args, &names); err != nil {
					return nil, err
				}
				host := tsoptionstest.NewVFSParseConfigHost(in.Files, in.CurrentDirectory, in.CaseSensitive)
				entries := []*tsoptions.ExtendedConfigCacheEntry{}
				for _, name := range names {
					entries = append(entries, tsoptions.ParseExtendedConfig(name,
						tspath.ToPath(name, in.CurrentDirectory, in.CaseSensitive), nil, host, nil))
				}
				return func() any {
					out := []any{}
					for _, entry := range entries {
						// Callers read length and elements: nil and empty are one value.
						if names := entry.ExtendedFileNames(); len(names) > 0 {
							out = append(out, hexes(names))
						} else {
							out = append(out, nil)
						}
					}
					return out
				}, nil
			},
		},
		// ReloadFileNamesOfParsedCommandLine on the config's files plus the
		// args' new files: [the reloaded file names, its literal file names,
		// the original's file names], each hex.
		Column{
			ID:    "tsoptions.ParsedCommandLine.ReloadFileNamesOfParsedCommandLine",
			Input: "config",
			Build: func(raw json.RawMessage) (func() any, error) {
				parsed, in, err := ParseConfig(raw)
				if err != nil {
					return nil, err
				}
				var extra []string
				if err := json.Unmarshal(in.Args, &extra); err != nil {
					return nil, err
				}
				files := map[string]string{}
				for name, text := range in.Files {
					files[name] = text
				}
				for _, name := range extra {
					files[name] = ""
				}
				fs := tsoptionstest.NewVFSParseConfigHost(files, in.CurrentDirectory, in.CaseSensitive).FS()
				return func() any {
					reloaded := parsed.ReloadFileNamesOfParsedCommandLine(fs)
					return []any{hexes(reloaded.FileNames()), hexes(reloaded.LiteralFileNames()), hexes(parsed.FileNames())}
				}, nil
			},
		},
		typedValuesColumn("tsoptions.TargetToLibMap", func(in struct{}) any {
			entries := map[string]int{}
			for target, lib := range tsoptions.TargetToLibMap() {
				entries[lib] = int(target)
			}
			return sortedPairs(entries)
		}),
		typedValuesColumn("core.ResolveProjectReferencePath", func(in struct {
			Paths []string `json:"paths"`
		}) any {
			out := []any{}
			for _, path := range in.Paths {
				out = append(out, []any{Hex(core.ResolveConfigFileNameOfProjectReference(path)),
					Hex(core.ResolveProjectReferencePath(&core.ProjectReference{Path: path}))})
			}
			return out
		}),
		// tsoptions/parsedcommandline.go:ParsedCommandLine.
		// PossiblyMatchesDirectoryName on a parsed tsconfig and a list of
		// directory paths (args: ["path", ...]): one bool per path. The
		// wildcard directories are computed in setup, so the column holds only
		// the operation's own work.
		Column{
			ID:    "tsoptions.ParsedCommandLine.PossiblyMatchesDirectoryName",
			Input: "config",
			Build: func(raw json.RawMessage) (func() any, error) {
				parsed, in, err := ParseConfig(raw)
				if err != nil {
					return nil, err
				}
				var paths []string
				if err := json.Unmarshal(in.Args, &paths); err != nil {
					return nil, err
				}
				_ = parsed.WildcardDirectories()
				return func() any {
					out := []any{}
					for _, path := range paths {
						out = append(out, parsed.PossiblyMatchesDirectoryName(tspath.Path(path)))
					}
					return out
				}, nil
			},
		},
	)
}
