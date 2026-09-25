// Phase 1 operation tables: group tsoptions (tsoptions ports, computeFn
// through the bridge in bridges/tsoptions). Columns are registered in init;
// the Rust side is tools/phase1/mutation/driver/src/table/tsoptions.rs and
// the spec is data/phase1/tables/tsoptions.json.
package main

import (
	"encoding/json"

	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

func init() {
	Register("tsoptions",
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
