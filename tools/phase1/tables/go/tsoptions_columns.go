// Phase 1 operation tables: group tsoptions (tsoptions ports, computeFn
// through the bridge in bridges/tsoptions). Columns are registered in init;
// the Rust side is tools/phase1/mutation/driver/src/table/tsoptions.rs and
// the spec is data/phase1/tables/tsoptions.json.
package main

import (
	"encoding/json"
	"reflect"
	"slices"

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
