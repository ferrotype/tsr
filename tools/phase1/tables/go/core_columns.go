// Phase 1 operation tables: group core (core helpers, collections, link
// store, stack). Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/core.rs and the spec is
// data/phase1/tables/core.json.
package main

import (
	"bytes"
	"encoding/json"
	"iter"
	"slices"
	"sort"
	"strings"

	"github.com/microsoft/TypeScript/tsc/internal/core"
)

// typedValuesColumn is a values column whose input decodes into I; the value
// is f(input).
func typedValuesColumn[I any](id string, f func(in I) any) Column {
	return Column{
		ID:    id,
		Input: "values",
		Build: func(raw json.RawMessage) (func() any, error) {
			var in I
			if err := DecodeInput(raw, &in); err != nil {
				return nil, err
			}
			return func() any { return f(in) }, nil
		},
	}
}

func ints(values []int) []any {
	out := []any{}
	for _, value := range values {
		out = append(out, Scalar(value))
	}
	return out
}

// sortedPairs projects a string-keyed int map as [[key hex, value], ...] by key.
func sortedPairs(m map[string]int) []any {
	return sortedTable(m, func(value int) any { return Scalar(value) }).([]any)
}

type mapCase struct {
	M1  map[string]int `json:"m1"`
	M2  map[string]int `json:"m2"`
	Dst map[string]int `json:"dst"`
	Nil bool           `json:"nil"`
	// Which DiffMaps callbacks are given.
	Added   bool `json:"added"`
	Removed bool `json:"removed"`
	Changed bool `json:"changed"`
}

// diffEvents runs a DiffMaps-shaped function with recording callbacks and
// returns the events sorted by their canonical encoding (Go ranges over maps
// in random order, so the order is no contract).
func diffEvents(c mapCase, run func(onAdded func(string, int), onRemoved func(string, int), onChanged func(string, int, int))) []any {
	events := [][]any{}
	var onAdded func(string, int)
	var onRemoved func(string, int)
	var onChanged func(string, int, int)
	if c.Added {
		onAdded = func(k string, v int) { events = append(events, []any{"added", Hex(k), Scalar(v)}) }
	}
	if c.Removed {
		onRemoved = func(k string, v int) { events = append(events, []any{"removed", Hex(k), Scalar(v)}) }
	}
	if c.Changed {
		onChanged = func(k string, v1, v2 int) { events = append(events, []any{"changed", Hex(k), Scalar(v1), Scalar(v2)}) }
	}
	run(onAdded, onRemoved, onChanged)
	key := func(event []any) string {
		var buffer bytes.Buffer
		Canonical(&buffer, event)
		return buffer.String()
	}
	sort.Slice(events, func(i, j int) bool { return key(events[i]) < key(events[j]) })
	out := []any{}
	for _, event := range events {
		out = append(out, event)
	}
	return out
}

// predicates are the named predicates the Or column combines.
var predicates = map[string]func(int) bool{
	"even":     func(v int) bool { return v%2 == 0 },
	"negative": func(v int) bool { return v < 0 },
	"big":      func(v int) bool { return v > 100 },
}

func init() {
	Register("core",
		typedValuesColumn("core.CheckEachDefined", func(in struct {
			Lists [][]int `json:"lists"`
		}) any {
			out := []any{}
			for _, list := range in.Lists {
				pointers := make([]*int, len(list))
				for i := range list {
					pointers[i] = &list[i]
				}
				result := core.CheckEachDefined(pointers, "undefined element")
				values := []int{}
				for _, pointer := range result {
					values = append(values, *pointer)
				}
				out = append(out, ints(values))
			}
			return out
		}),
		typedValuesColumn("core.CopyMapInto", func(in struct {
			Cases []mapCase `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				dst := c.Dst
				if c.Nil {
					dst = nil
				}
				out = append(out, sortedPairs(core.CopyMapInto(dst, c.M2)))
			}
			return out
		}),
		typedValuesColumn("core.DiffMaps", func(in struct {
			Cases []mapCase `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				out = append(out, diffEvents(c, func(a func(string, int), r func(string, int), ch func(string, int, int)) {
					core.DiffMaps(c.M1, c.M2, a, r, ch)
				}))
			}
			return out
		}),
		// With equality modulo 10.
		typedValuesColumn("core.DiffMapsFunc", func(in struct {
			Cases []mapCase `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				out = append(out, diffEvents(c, func(a func(string, int), r func(string, int), ch func(string, int, int)) {
					core.DiffMapsFunc(c.M1, c.M2, func(v1, v2 int) bool { return v1%10 == v2%10 }, a, r, ch)
				}))
			}
			return out
		}),
		typedValuesColumn("core.ElementOrNil", func(in struct {
			Cases []struct {
				Slice []int `json:"slice"`
				Index int   `json:"index"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				out = append(out, Scalar(core.ElementOrNil(c.Slice, c.Index)))
			}
			return out
		}),
		// With the predicate "greater than the threshold".
		typedValuesColumn("core.FindLastIndex", func(in struct {
			Cases []struct {
				Slice     []int `json:"slice"`
				Threshold int   `json:"threshold"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				out = append(out, Scalar(core.FindLastIndex(c.Slice, func(v int) bool { return v > c.Threshold })))
			}
			return out
		}),
		// With the mapping "minus the offset".
		typedValuesColumn("core.FirstNonNil", func(in struct {
			Cases []struct {
				Slice  []int `json:"slice"`
				Offset int   `json:"offset"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				out = append(out, Scalar(core.FirstNonNil(c.Slice, func(v int) int { return v - c.Offset })))
			}
			return out
		}),
		typedValuesColumn("core.FirstNonZero", func(in struct {
			Lists [][]int `json:"lists"`
		}) any {
			out := []any{}
			for _, list := range in.Lists {
				out = append(out, Scalar(core.FirstNonZero(list...)))
			}
			return out
		}),
		// A nil sequence where the case has none.
		typedValuesColumn("core.FirstOrNilSeq", func(in struct {
			Cases []struct {
				Seq []int `json:"seq"`
				Nil bool  `json:"nil"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				var seq iter.Seq[int]
				if !c.Nil {
					seq = slices.Values(c.Seq)
				}
				out = append(out, Scalar(core.FirstOrNilSeq(seq)))
			}
			return out
		}),
		// Over string candidates compared with strings.Compare; the empty
		// string where there is no suggestion.
		typedValuesColumn("core.GetSpellingSuggestionWithMaxCandidateCount", func(in struct {
			Cases []struct {
				Name       string   `json:"name"`
				Candidates []string `json:"candidates"`
				Max        int      `json:"max"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				out = append(out, Hex(core.GetSpellingSuggestionWithMaxCandidateCount(c.Name, slices.Values(c.Candidates),
					func(s string) string { return s }, strings.Compare, c.Max)))
			}
			return out
		}),
		typedValuesColumn("core.IndexAfter", func(in struct {
			Cases []struct {
				S       string `json:"s"`
				Pattern string `json:"pattern"`
				Start   int    `json:"start"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				out = append(out, Scalar(core.IndexAfter(c.S, c.Pattern, c.Start)))
			}
			return out
		}),
		typedValuesColumn("core.MapNonNil", func(in struct {
			Cases []struct {
				Slice  []int `json:"slice"`
				Offset int   `json:"offset"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				out = append(out, ints(core.MapNonNil(c.Slice, func(v int) int { return v - c.Offset })))
			}
			return out
		}),
		// With cmp comparing the values modulo 10.
		typedValuesColumn("core.MinAllFunc", func(in struct {
			Lists [][]int `json:"lists"`
		}) any {
			out := []any{}
			for _, list := range in.Lists {
				out = append(out, ints(core.MinAllFunc(list, func(a, b int) int { return a%10 - b%10 })))
			}
			return out
		}),
		// Or of the named predicates, applied to each input.
		typedValuesColumn("core.Or", func(in struct {
			Cases []struct {
				Predicates []string `json:"predicates"`
				Inputs     []int    `json:"inputs"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				funcs := []func(int) bool{}
				for _, name := range c.Predicates {
					funcs = append(funcs, predicates[name])
				}
				or := core.Or(funcs...)
				results := []any{}
				for _, input := range c.Inputs {
					results = append(results, or(input))
				}
				out = append(out, results)
			}
			return out
		}),
		typedValuesColumn("core.ReplaceElement", func(in struct {
			Cases []struct {
				Slice []int `json:"slice"`
				I     int   `json:"i"`
				T     int   `json:"t"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				out = append(out, ints(core.ReplaceElement(c.Slice, c.I, c.T)))
			}
			return out
		}),
		// With f adding the index to multiples of the divisor: [result, whether
		// it is the input slice itself].
		typedValuesColumn("core.SameMapIndex", func(in struct {
			Cases []struct {
				Slice   []int `json:"slice"`
				Divisor int   `json:"divisor"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				result := core.SameMapIndex(c.Slice, func(v int, i int) int {
					if v%c.Divisor == 0 {
						return v + i
					}
					return v
				})
				same := len(result) > 0 && len(c.Slice) > 0 && &result[0] == &c.Slice[0]
				out = append(out, []any{ints(result), same})
			}
			return out
		}),
		typedValuesColumn("core.ShouldRewriteModuleSpecifier", func(in struct {
			Cases []struct {
				Specifier string `json:"specifier"`
				Rewrite   int    `json:"rewrite"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				options := &core.CompilerOptions{RewriteRelativeImportExtensions: core.Tristate(c.Rewrite)}
				out = append(out, core.ShouldRewriteModuleSpecifier(c.Specifier, options))
			}
			return out
		}),
		typedValuesColumn("core.UnorderedEqual", func(in struct {
			Pairs [][2][]int `json:"pairs"`
		}) any {
			out := []any{}
			for _, pair := range in.Pairs {
				out = append(out, core.UnorderedEqual(pair[0], pair[1]))
			}
			return out
		}),
		// Operations on one LinkStore[string, int]: get (and set the value),
		// has, try_get; the value is each operation's result.
		typedValuesColumn("core.LinkStore", func(in struct {
			Ops []struct {
				Op    string `json:"op"`
				Key   string `json:"key"`
				Value int    `json:"value"`
			} `json:"ops"`
		}) any {
			var store core.LinkStore[string, int]
			out := []any{}
			for _, op := range in.Ops {
				switch op.Op {
				case "get":
					value := store.Get(op.Key)
					before := *value
					*value = op.Value
					out = append(out, Scalar(before))
				case "has":
					out = append(out, store.Has(op.Key))
				case "try_get":
					if value := store.TryGet(op.Key); value != nil {
						out = append(out, Scalar(*value))
					} else {
						out = append(out, nil)
					}
				}
			}
			return out
		}),
		typedValuesColumn("core.PagedLinkStore", func(in struct {
			Ops []struct {
				Op    string `json:"op"`
				Key   uint64 `json:"key"`
				Value int    `json:"value"`
			} `json:"ops"`
		}) any {
			var store core.PagedLinkStore[int]
			out := []any{}
			for _, op := range in.Ops {
				switch op.Op {
				case "get":
					value := store.Get(op.Key)
					before := *value
					*value = op.Value
					out = append(out, Scalar(before))
				case "has":
					out = append(out, store.Has(op.Key))
				case "try_get":
					if value := store.TryGet(op.Key); value != nil {
						out = append(out, Scalar(*value))
					} else {
						out = append(out, nil)
					}
				}
			}
			return out
		}),
		typedValuesColumn("core.NonRelativeModuleNameForTypingCache", func(in struct {
			Names []string `json:"names"`
		}) any {
			out := []any{}
			for _, name := range in.Names {
				out = append(out, Hex(core.NonRelativeModuleNameForTypingCache(name)))
			}
			return out
		}),
		// Operations on one Stack[int]: push, pop, peek, len (pop and peek
		// only on a nonempty stack).
		typedValuesColumn("core.Stack", func(in struct {
			Ops []struct {
				Op    string `json:"op"`
				Value int    `json:"value"`
			} `json:"ops"`
		}) any {
			var stack core.Stack[int]
			out := []any{}
			for _, op := range in.Ops {
				switch op.Op {
				case "push":
					stack.Push(op.Value)
					out = append(out, nil)
				case "pop":
					out = append(out, Scalar(stack.Pop()))
				case "peek":
					out = append(out, Scalar(stack.Peek()))
				case "len":
					out = append(out, Scalar(stack.Len()))
				}
			}
			return out
		}),
		// Edits of one text: [ApplyBulkEdits hex, [ApplyTo hex per edit]].
		typedValuesColumn("core.ApplyBulkEdits", func(in struct {
			Cases []struct {
				Text  string `json:"text"`
				Edits []struct {
					Pos  int    `json:"pos"`
					End  int    `json:"end"`
					Text string `json:"text"`
				} `json:"edits"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				edits := []core.TextChange{}
				applied := []any{}
				for _, edit := range c.Edits {
					change := core.TextChange{TextRange: core.NewTextRange(edit.Pos, edit.End), NewText: edit.Text}
					edits = append(edits, change)
					applied = append(applied, Hex(change.ApplyTo(c.Text)))
				}
				out = append(out, []any{Hex(core.ApplyBulkEdits(c.Text, edits)), applied})
			}
			return out
		}),
		// [a equals b, a equals itself, a equals nil, nil equals nil] per pair.
		typedValuesColumn("core.TypeAcquisition.Equals", func(in struct {
			Pairs [][2]struct {
				Enable  int      `json:"enable"`
				Include []string `json:"include"`
				Exclude []string `json:"exclude"`
				Disable int      `json:"disable"`
				NilList bool     `json:"nil_include"`
			} `json:"pairs"`
		}) any {
			out := []any{}
			for _, pair := range in.Pairs {
				build := func(i int) *core.TypeAcquisition {
					ta := &core.TypeAcquisition{Enable: core.Tristate(pair[i].Enable), Include: pair[i].Include,
						Exclude: pair[i].Exclude, DisableFilenameBasedTypeAcquisition: core.Tristate(pair[i].Disable)}
					if pair[i].NilList {
						ta.Include = nil
					}
					return ta
				}
				a, b := build(0), build(1)
				var none *core.TypeAcquisition
				out = append(out, []any{a.Equals(b), a.Equals(a), a.Equals(none), none.Equals(none)})
			}
			return out
		}),
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
