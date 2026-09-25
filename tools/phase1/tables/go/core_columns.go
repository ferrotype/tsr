// Phase 1 operation tables: group core (core helpers, collections, link
// store, stack). Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/core.rs and the spec is
// data/phase1/tables/core.json.
package main

import (
	"encoding/json"

	"github.com/microsoft/TypeScript/tsc/internal/core"
)

func init() {
	Register("core",
		// core/core.go:Splice[int64] on one slice and a grid of (start,
		// deleteCount, items). Its only callers (printer/emitcontext.go:
		// 314-333) assign the result back, so the value, not the aliasing,
		// is the contract.
		Column{
			ID:    "core.Splice",
			Input: "values",
			Build: func(raw json.RawMessage) (func() any, error) {
				var in struct {
					S     []int64 `json:"s"`
					Cases []struct {
						Start int     `json:"start"`
						Count int     `json:"count"`
						Items []int64 `json:"items"`
					} `json:"cases"`
				}
				if err := DecodeInput(raw, &in); err != nil {
					return nil, err
				}
				inputs := make([][]int64, len(in.Cases))
				for i := range in.Cases {
					inputs[i] = append([]int64(nil), in.S...)
				}
				return func() any {
					out := []any{}
					for i, c := range in.Cases {
						result := core.Splice(inputs[i], c.Start, c.Count, c.Items...)
						values := make([]any, len(result))
						for j, value := range result {
							values[j] = value
						}
						out = append(out, values)
					}
					return out
				}, nil
			},
		},
	)
}
