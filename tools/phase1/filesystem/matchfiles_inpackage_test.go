package tsoptions

// Phase 1 F2a, test-only accessor for the carried `config/matchFiles`
// renderer. It is overlaid into the pinned `tsoptions` package as
// `phase1_probe_inpackage_export_test.go`, next to the renderer probe, which is
// overlaid as `phase1_probe_export_test.go` into `tsoptions_test`. Nothing
// under `upstream/` is modified: both files are overlay-only test sources.
//
// Why it exists. `ParsedCommandLine.WildcardDirectories()`
// (parsedcommandline.go:258-273) reads `p.ConfigFile.configFileSpecs`, and the
// raw-JSON entry point `ParseJsonConfigFileContent` (tsconfigparsing.go:872-880)
// passes a nil `sourceFile` to the worker, so `ConfigFile` is nil and the
// accessor dereferences nil. The specs themselves are not missing: the worker
// computes `validatedIncludeSpecs` and `validatedExcludeSpecs` unconditionally
// at tsconfigparsing.go:1328-1362, and only their *attachment* is gated, at
// :1375-1377 `if sourceFile != nil { sourceFile.configFileSpecs = ... }`. The
// computed value is then discarded, so there is nothing left on the returned
// `*ParsedCommandLine` to read: it cannot be captured after the fact without
// editing the pin, which is not allowed.
//
// What this does instead. It re-derives the two validated spec slices from the
// pinned parse's *own result* -- `p.Raw` and `p.ParsedConfig.CompilerOptions`
// are the very values the worker used (tsconfigparsing.go:1518-1523) and
// `p.comparePathsOptions.CurrentDirectory` is its `basePathForFileNames`
// (:1528-1531) -- by calling the same pinned unexported functions in the same
// order: `parseJsonToStringKey`, `validateSpecs` and
// `getSubstitutedStringArrayWithConfigDirTemplate`. It then hands those specs
// to the pinned `getWildcardDirectories` (wildcarddirectories.go:10), which
// takes the specs directly and never needs a `ConfigFile`.
//
// This is a re-derivation, not a capture, and it is reported as one. The only
// logic restated here is the control flow between tsconfigparsing.go:1306 and
// :1362 that selects and validates the specs; every leaf computation,
// including the wildcard calculation itself, is the pinned function. The
// restated `getPropFromRaw` closure omits only its diagnostics: those were
// already produced by the real parse and are carried on
// `ParsedCommandLine.Errors`, which the renderer prints, so re-raising them
// here would double them.

import (
	"reflect"

	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfsmatch"
)

// phase1ValidatedSpecs restates tsconfigparsing.go:1306-1349 for the
// `sourceFile == nil` path, reading its inputs off the parse's own result.
func phase1ValidatedSpecs(p *ParsedCommandLine) (include []string, exclude []string) {
	rawConfig := parseJsonToStringKey(p.Raw)

	// tsconfigparsing.go:1265-1282, for the `sourceFile == nil` case, minus the
	// diagnostics the real parse already reported.
	getPropFromRaw := func(prop string) propOfRaw {
		value, exists := rawConfig.Get(prop)
		if exists && value != nil {
			if reflect.TypeOf(value).Kind() == reflect.Slice {
				result := rawConfig.GetOrZero(prop)
				return propOfRaw{sliceValue: result.([]any)}
			}
			return propOfRaw{sliceValue: nil, wrongValue: "not-array"}
		}
		return propOfRaw{sliceValue: nil, wrongValue: "no-prop"}
	}

	fileSpecs := getPropFromRaw("files")
	includeSpecs := getPropFromRaw("include")
	excludeSpecs := getPropFromRaw("exclude")

	// tsconfigparsing.go:1308-1323.
	if excludeSpecs.wrongValue == "no-prop" && p.ParsedConfig != nil && p.ParsedConfig.CompilerOptions != nil {
		outDir := p.ParsedConfig.CompilerOptions.OutDir
		declarationDir := p.ParsedConfig.CompilerOptions.DeclarationDir
		if outDir != "" || declarationDir != "" {
			var values []any
			if outDir != "" {
				values = append(values, outDir)
			}
			if declarationDir != "" {
				values = append(values, declarationDir)
			}
			excludeSpecs = propOfRaw{sliceValue: values}
		}
	}

	// tsconfigparsing.go:1324-1327.
	if fileSpecs.sliceValue == nil && includeSpecs.sliceValue == nil {
		includeSpecs = propOfRaw{sliceValue: []any{defaultIncludeSpec}}
	}

	basePathForFileNames := p.comparePathsOptions.CurrentDirectory

	// tsconfigparsing.go:1336-1349. `validateSpecs` takes the tsconfig source
	// file only to locate its diagnostics; on this path the worker passed
	// `tsconfigToSourceFile(nil)`, which is nil (tsconfigparsing.go:289-294).
	if includeSpecs.sliceValue != nil {
		beforeSubstitution, _ := validateSpecs(includeSpecs.sliceValue, true /*disallowTrailingRecursion*/, nil, "include")
		if include = getSubstitutedStringArrayWithConfigDirTemplate(beforeSubstitution, basePathForFileNames); include == nil {
			include = beforeSubstitution
		}
	}
	if excludeSpecs.sliceValue != nil {
		exclude, _ = validateSpecs(excludeSpecs.sliceValue, false /*disallowTrailingRecursion*/, nil, "exclude")
		if withSubstitution := getSubstitutedStringArrayWithConfigDirTemplate(exclude, basePathForFileNames); withSubstitution != nil {
			exclude = withSubstitution
		}
	}
	return include, exclude
}

// Phase1WildcardDirectoriesWithoutConfigFile hands the re-derived specs to the
// pinned `getWildcardDirectories` and returns what it answered, together with
// the specs it was given, so a differing row can record the exact input as well
// as the output. It refuses any other input: when `ConfigFile` is non-nil the
// caller must use the pinned `WildcardDirectories()` accessor, so that path is
// never re-derived.
func Phase1WildcardDirectoriesWithoutConfigFile(p *ParsedCommandLine) (directories map[string]bool, include []string, exclude []string, ok bool) {
	if p == nil || p.ConfigFile != nil {
		return nil, nil, nil, false
	}
	include, exclude = phase1ValidatedSpecs(p)
	return getWildcardDirectories(include, exclude, p.comparePathsOptions), include, exclude, true
}

// Phase1ValidatedSpecsFromConfigFile is the sibling of the re-derivation above
// for the entry point that *does* attach a ConfigFile. It reads the specs the
// pinned parse itself recorded -- `configFileSpecs.validatedIncludeSpecs` and
// `validatedExcludeSpecs`, attached at tsconfigparsing.go:1375-1377 -- which
// are the very slices `ParsedCommandLine.WildcardDirectories()` passes to
// `getWildcardDirectories` (parsedcommandline.go:258-273). Nothing is derived
// here: this is a read of the pinned parse's own field, and it exists only
// because the field is unexported.
func Phase1ValidatedSpecsFromConfigFile(p *ParsedCommandLine) (include []string, exclude []string, ok bool) {
	if p == nil || p.ConfigFile == nil || p.ConfigFile.configFileSpecs == nil {
		return nil, nil, false
	}
	specs := p.ConfigFile.configFileSpecs
	return specs.validatedIncludeSpecs, specs.validatedExcludeSpecs, true
}

// Phase1WildcardDirectoryOrder recovers the order in which the pinned
// `getWildcardDirectories` inserted the keys of the map it returned.
//
// Why the order has to be recovered at all. The pinned function is a
// line-for-line port of commandLineParser.ts:4113-4160, where
// `wildcardDirectories` is a JS object literal filled by `for (const file of
// include)`, so its key order *is* that loop's insertion order and the frozen
// baselines record it. The Go port fills a `map[string]bool`
// (wildcarddirectories.go:29) and returns it, and a Go map has no order, so the
// information the baselines carry is destroyed on the way out. It is not lost,
// though: it is a function of the include specs the calculation was given, so
// it can be recovered from them.
//
// What this does and does not do. It does not recompute which directories are
// watched or how: `directories` is the pinned function's own answer and is
// never added to, removed from or corrected here. The only thing restated is
// the *iteration* of wildcarddirectories.go:35-39 -- normalize each include
// spec against the base path, skip the ones the exclude matcher matches, ask
// which directory it names -- and every step of that is the pinned call
// (`tspath.NormalizePath`, `tspath.CombinePaths`, `vfsmatch.NewSpecMatcher`,
// `getWildcardDirectoryFromSpec`). Each spec's directory is emitted the first
// time it appears and only if the returned map still holds it, which is what
// makes this a recovery of the pinned key order rather than a second
// calculation: a spec whose directory the pinned function dropped (a duplicate
// canonical key resolving to an earlier path at :51-57, or a subpath deleted
// under a recursive watch at :70-78) contributes nothing.
//
// `ok` is false when the walk does not account for every key exactly once. The
// one shape that would produce that is a key the pinned function deleted and
// then re-inserted, which in the TypeScript object moves it to the end while
// this walk would leave it at its first position; no frozen baseline exercises
// it, and the caller is told rather than quietly given a guess.
func Phase1WildcardDirectoryOrder(p *ParsedCommandLine, include []string, exclude []string, directories map[string]bool) (order []string, ok bool) {
	if p == nil || len(directories) == 0 {
		return nil, len(directories) == 0
	}
	comparePathsOptions := p.comparePathsOptions
	excludeMatcher := vfsmatch.NewSpecMatcher(
		exclude,
		comparePathsOptions.CurrentDirectory,
		vfsmatch.UsageExclude,
		comparePathsOptions.UseCaseSensitiveFileNames,
	)
	order = make([]string, 0, len(directories))
	emitted := make(map[string]struct{}, len(directories))
	for _, file := range include {
		spec := tspath.NormalizePath(tspath.CombinePaths(comparePathsOptions.CurrentDirectory, file))
		if excludeMatcher != nil && excludeMatcher.MatchString(spec) {
			continue
		}
		match := getWildcardDirectoryFromSpec(spec, comparePathsOptions.UseCaseSensitiveFileNames)
		if match == nil {
			continue
		}
		if _, kept := directories[match.Path]; !kept {
			continue
		}
		if _, already := emitted[match.Path]; already {
			continue
		}
		emitted[match.Path] = struct{}{}
		order = append(order, match.Path)
	}
	if len(order) != len(directories) {
		return nil, false
	}
	return order, true
}
