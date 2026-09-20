package collections_test

// Access only: replays an ordered action trace against the pinned collection
// types and records what each action observed.
//
// It is an external test file, like every pinned test file in this directory.
// Every operation it exercises is exported, so in-package access would buy
// nothing, and staying outside keeps the package-scope names this file
// declares (the action type, the replay functions, the small render helpers)
// out of `package collections`, where a later pin could collide with them.
//
// The trace shape is the contract the Rust driver answers: one ordered result
// per action, under `ordered`, so member order survives canonicalisation.
//
// It covers every type in the package: OrderedMap and OrderedSet, whose
// subject is insertion order; Set and MultiMap, whose subject is membership,
// per-method nil tolerance and which results alias the container's own
// storage; CopyOnWriteMap/CopyOnWriteSet, whose subject is what a scope
// inherits, clones and discards; and SyncMap/SyncSet, whose subject includes
// the present-with-nil contract the pinned syncmap_test pins.
//
// Two rules keep a hash-backed observation reproducible. Anything derived from
// a Go map's iteration order is sorted before it is recorded, because that
// order is randomized and is not the contract. A Range stopped by its callback
// records only how many entries it visited, never which ones: sync.Map
// specifies the count, not the selection, so recording the keys would record
// the scheduler.
//
// A third rule keeps a recorded panic portable. A panic the pinned source
// raises itself is recorded with its literal text, because that text is the
// contract (set.go's "cannot modify nil Set"). A panic the Go runtime raises
// is reduced to a class instead: its wording names dynamic types and can move
// with the toolchain, and no Rust port could ever reproduce the sentence.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"slices"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/collections"
)

type action struct {
	Op    string `json:"op"`
	Key   string `json:"key"`
	Value string `json:"value"`
	Index int    `json:"index"`
	Size  int    `json:"size"`
	Stop  int    `json:"stop"`
	// Target names the container an action applies to for the subjects that
	// need more than one (Set's binary operations); empty means the first.
	Target string `json:"target"`
	// Other names the second operand of a binary operation; empty means "the
	// other of s and t".
	Other string `json:"other"`
	// Items carries a constructor's input list (NewSetFromItems, GroupBy).
	Items []string `json:"items"`
	// KeyOf names GroupBy's grouping rule. It travels in the request so the
	// trace says what the key is: a rule that lived only in this file would
	// leave a Rust port reading the request with no way to know it.
	KeyOf string `json:"key_of"`
}

// decodeAction unmarshals a request's actions once its subject has matched.
func decodeAction(raw json.RawMessage) []action {
	if len(raw) == 0 {
		return nil
	}
	var out []action
	if err := json.Unmarshal(raw, &out); err != nil {
		return nil
	}
	return out
}

type leafRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared schedule and another
	// group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
}

// entries renders an ordered map as an entry array, never as a JSON object:
// an object's member order does not survive canonical comparison.
func entries(m *collections.OrderedMap[string, string]) []any {
	out := []any{}
	for key, value := range m.Entries() {
		out = append(out, []any{key, value})
	}
	return out
}

func keysOf(m *collections.OrderedMap[string, string]) []any {
	out := []any{}
	for key := range m.Keys() {
		out = append(out, key)
	}
	return out
}

// guarded runs f and converts a panic into a recorded class, so a case whose
// subject is the panic boundary observes it instead of failing the probe.
func guarded(f func() any) (result any, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = nil
			panicked = classify(r)
		}
	}()
	return f(), ""
}

func classify(r any) string {
	text := ""
	if err, ok := r.(error); ok {
		text = err.Error()
	} else if s, ok := r.(string); ok {
		text = s
	} else {
		text = "non-error panic"
	}
	switch {
	case contains(text, "index out of range"), contains(text, "slice bounds out of range"):
		return "index_out_of_range"
	case contains(text, "nil pointer dereference"), contains(text, "invalid memory address"):
		return "nil_pointer_dereference"
	case contains(text, "interface conversion"):
		// A failed type assertion. The runtime spells out both dynamic types
		// ("interface is nil, not interface {}"), which is toolchain wording
		// rather than pinned source, so only the shape is recorded.
		// SyncMap.ToMap reaches this by asserting a stored nil.
		if contains(text, "interface is nil") {
			return "nil_interface_conversion"
		}
		return "interface_conversion"
	default:
		// Reached by a panic the pinned source raises itself, whose literal
		// text is the contract: set.go's "cannot modify nil Set".
		return "other:" + text
	}
}

func contains(s, sub string) bool {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return true
		}
	}
	return false
}

func replayOrderedMap(request leafRequest) []any {
	var m *collections.OrderedMap[string, string]
	var clone *collections.OrderedMap[string, string]
	ordered := []any{}
	for _, a := range decodeAction(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new":
			m = &collections.OrderedMap[string, string]{}
		case "new_nil":
			m = nil
		case "new_size_hint":
			m = collections.NewOrderedMapWithSizeHint[string, string](a.Size)
		case "set":
			value, panicked := guarded(func() any { m.Set(a.Key, a.Value); return nil })
			_ = value
			row["panic"] = panicked
		case "delete":
			value, panicked := guarded(func() any {
				got, ok := m.Delete(a.Key)
				return []any{got, ok}
			})
			row["result"], row["panic"] = value, panicked
		case "get":
			value, panicked := guarded(func() any {
				got, ok := m.Get(a.Key)
				return []any{got, ok}
			})
			row["result"], row["panic"] = value, panicked
		case "has":
			value, panicked := guarded(func() any { return m.Has(a.Key) })
			row["result"], row["panic"] = value, panicked
		case "size":
			value, panicked := guarded(func() any { return m.Size() })
			row["result"], row["panic"] = value, panicked
		case "keys":
			value, panicked := guarded(func() any { return keysOf(m) })
			row["result"], row["panic"] = value, panicked
		case "entries":
			value, panicked := guarded(func() any { return entries(m) })
			row["result"], row["panic"] = value, panicked
		case "clear":
			_, panicked := guarded(func() any { m.Clear(); return nil })
			row["panic"] = panicked
		case "clone":
			value, panicked := guarded(func() any {
				clone = m.Clone()
				return nil
			})
			_ = value
			row["panic"] = panicked
		case "clone_entries":
			value, panicked := guarded(func() any { return entries(clone) })
			row["result"], row["panic"] = value, panicked
		case "clone_set":
			_, panicked := guarded(func() any { clone.Set(a.Key, a.Value); return nil })
			row["panic"] = panicked
		case "early_stop_keys":
			// Break out after `stop` keys: the pinned iterator is a range-over-func,
			// so stopping early is an observable contract, not an implementation detail.
			value, panicked := guarded(func() any {
				seen := []any{}
				count := 0
				for key := range m.Keys() {
					if count >= a.Stop {
						break
					}
					seen = append(seen, key)
					count++
				}
				return seen
			})
			row["result"], row["panic"] = value, panicked
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

// sortedStrings renders values whose source is a Go map. Map iteration order
// is randomized, so an observation that leaked it would not be reproducible.
// For the hash-backed types the subject is presence, not order; the ordered
// types never reach this helper.
func sortedStrings(values []string) []any {
	sorted := slices.Clone(values)
	slices.Sort(sorted)
	out := make([]any, 0, len(sorted))
	for _, value := range sorted {
		out = append(out, value)
	}
	return out
}

// listOf keeps a slice's own order, which for MultiMap values is the append
// order and therefore part of the contract.
func listOf(values []string) []any {
	out := make([]any, 0, len(values))
	for _, value := range values {
		out = append(out, value)
	}
	return out
}

func replayOrderedSet(request leafRequest) []any {
	var s *collections.OrderedSet[string]
	var clone *collections.OrderedSet[string]
	values := func(target *collections.OrderedSet[string]) any {
		out := []any{}
		for value := range target.Values() {
			out = append(out, value)
		}
		return out
	}
	ordered := []any{}
	for _, a := range decodeAction(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new":
			s = &collections.OrderedSet[string]{}
		case "new_nil":
			s = nil
		case "new_size_hint":
			s = collections.NewOrderedSetWithSizeHint[string](a.Size)
		case "add":
			_, panicked := guarded(func() any { s.Add(a.Key); return nil })
			row["panic"] = panicked
		case "has":
			row["result"], row["panic"] = guarded(func() any { return s.Has(a.Key) })
		case "delete":
			row["result"], row["panic"] = guarded(func() any { return s.Delete(a.Key) })
		case "size":
			row["result"], row["panic"] = guarded(func() any { return s.Size() })
		case "values":
			row["result"], row["panic"] = guarded(func() any { return values(s) })
		case "clear":
			_, panicked := guarded(func() any { s.Clear(); return nil })
			row["panic"] = panicked
		case "clone":
			_, panicked := guarded(func() any { clone = s.Clone(); return nil })
			row["panic"] = panicked
		case "clone_values":
			row["result"], row["panic"] = guarded(func() any { return values(clone) })
		case "clone_size":
			row["result"], row["panic"] = guarded(func() any { return clone.Size() })
		case "clone_add":
			_, panicked := guarded(func() any { clone.Add(a.Key); return nil })
			row["panic"] = panicked
		case "early_stop_values":
			// Values is a range-over-func over the embedded map's keys, so
			// breaking out of it early is an observable contract.
			row["result"], row["panic"] = guarded(func() any {
				seen := []any{}
				for value := range s.Values() {
					if len(seen) >= a.Stop {
						break
					}
					seen = append(seen, value)
				}
				return seen
			})
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

// setSlots holds the three Set values a trace can name. Set's binary
// operations need a second receiver, and Clone/UnionedWith need somewhere to
// put their result, which is always `u`.
type setSlots struct {
	s, t, u *collections.Set[string]
}

func (slots *setSlots) get(name string) *collections.Set[string] {
	switch name {
	case "t":
		return slots.t
	case "u":
		return slots.u
	default:
		return slots.s
	}
}

func (slots *setSlots) put(name string, value *collections.Set[string]) {
	switch name {
	case "t":
		slots.t = value
	case "u":
		slots.u = value
	default:
		slots.s = value
	}
}

// operand resolves the second argument of a binary Set operation.
func (slots *setSlots) operand(target, other string) *collections.Set[string] {
	if other == "" {
		if target == "t" {
			other = "s"
		} else {
			other = "t"
		}
	}
	return slots.get(other)
}

// keySetResult records a Set's membership and whether the map it handed back
// was nil, which distinguishes a nil receiver and a zero value from a set
// whose backing map has been allocated.
func keySetResult(keys map[string]struct{}) any {
	names := make([]string, 0, len(keys))
	for key := range keys {
		names = append(names, key)
	}
	return []any{sortedStrings(names), keys == nil}
}

func replaySet(request leafRequest) []any {
	slots := &setSlots{}
	var captured map[string]struct{}
	ordered := []any{}
	for _, a := range decodeAction(request.Actions) {
		row := map[string]any{"op": a.Op, "target": a.Target}
		switch a.Op {
		case "new":
			slots.put(a.Target, &collections.Set[string]{})
		case "new_nil":
			slots.put(a.Target, nil)
		case "new_size_hint":
			slots.put(a.Target, collections.NewSetWithSizeHint[string](a.Size))
		case "new_from_items":
			slots.put(a.Target, collections.NewSetFromItems(a.Items...))
		case "add":
			_, panicked := guarded(func() any { slots.get(a.Target).Add(a.Key); return nil })
			row["panic"] = panicked
		case "add_if_absent":
			row["result"], row["panic"] = guarded(func() any {
				return slots.get(a.Target).AddIfAbsent(a.Key)
			})
		case "delete":
			_, panicked := guarded(func() any { slots.get(a.Target).Delete(a.Key); return nil })
			row["panic"] = panicked
		case "has":
			row["result"], row["panic"] = guarded(func() any { return slots.get(a.Target).Has(a.Key) })
		case "len":
			row["result"], row["panic"] = guarded(func() any { return slots.get(a.Target).Len() })
		case "keys":
			row["result"], row["panic"] = guarded(func() any {
				return keySetResult(slots.get(a.Target).Keys())
			})
		case "keys_capture":
			// Keys returns the live backing map. Holding on to it is how a
			// trace witnesses whether later mutations alias through it.
			row["result"], row["panic"] = guarded(func() any {
				captured = slots.get(a.Target).Keys()
				return keySetResult(captured)
			})
		case "keys_replay":
			row["result"], row["panic"] = guarded(func() any { return keySetResult(captured) })
		case "clear":
			_, panicked := guarded(func() any { slots.get(a.Target).Clear(); return nil })
			row["panic"] = panicked
		case "clone":
			row["result"], row["panic"] = guarded(func() any {
				clone := slots.get(a.Target).Clone()
				slots.u = clone
				return clone == nil
			})
		case "union":
			_, panicked := guarded(func() any {
				slots.get(a.Target).Union(slots.operand(a.Target, a.Other))
				return nil
			})
			row["panic"] = panicked
		case "unioned_with":
			row["result"], row["panic"] = guarded(func() any {
				united := slots.get(a.Target).UnionedWith(slots.operand(a.Target, a.Other))
				slots.u = united
				return united == nil
			})
		case "equals":
			row["result"], row["panic"] = guarded(func() any {
				return slots.get(a.Target).Equals(slots.operand(a.Target, a.Other))
			})
		case "is_subset_of":
			row["result"], row["panic"] = guarded(func() any {
				return slots.get(a.Target).IsSubsetOf(slots.operand(a.Target, a.Other))
			})
		case "intersects":
			row["result"], row["panic"] = guarded(func() any {
				return slots.get(a.Target).Intersects(slots.operand(a.Target, a.Other))
			})
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

// multiMapGetResult keeps the value slice's own order and records whether it
// was nil, which is how an absent key differs from an emptied one.
func multiMapGetResult(values []string) any {
	return []any{listOf(values), values == nil}
}

func replayMultiMap(request leafRequest) []any {
	var m *collections.MultiMap[string, string]
	var captured []string
	ordered := []any{}
	for _, a := range decodeAction(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new":
			m = &collections.MultiMap[string, string]{}
		case "new_size_hint":
			m = collections.NewMultiMapWithSizeHint[string, string](a.Size)
		case "group_by":
			// An unknown rule is recorded rather than silently defaulted: a
			// trace that meant something else must not be answered by this one.
			switch a.KeyOf {
			case "first_byte":
				m = collections.GroupBy[string, string](a.Items, func(item string) string {
					if item == "" {
						return ""
					}
					return item[:1]
				})
			default:
				row["unsupported_action"] = "group_by needs a known key_of, got " + a.KeyOf
			}
		case "add":
			_, panicked := guarded(func() any { m.Add(a.Key, a.Value); return nil })
			row["panic"] = panicked
		case "has":
			row["result"], row["panic"] = guarded(func() any { return m.Has(a.Key) })
		case "get":
			row["result"], row["panic"] = guarded(func() any { return multiMapGetResult(m.Get(a.Key)) })
		case "get_capture":
			row["result"], row["panic"] = guarded(func() any {
				captured = m.Get(a.Key)
				return multiMapGetResult(captured)
			})
		case "get_replay":
			row["result"], row["panic"] = guarded(func() any { return multiMapGetResult(captured) })
		case "remove":
			_, panicked := guarded(func() any { m.Remove(a.Key, a.Value); return nil })
			row["panic"] = panicked
		case "remove_all":
			_, panicked := guarded(func() any { m.RemoveAll(a.Key); return nil })
			row["panic"] = panicked
		case "len":
			row["result"], row["panic"] = guarded(func() any { return m.Len() })
		case "keys":
			row["result"], row["panic"] = guarded(func() any {
				names := []string{}
				for key := range m.Keys() {
					names = append(names, key)
				}
				return sortedStrings(names)
			})
		case "values":
			row["result"], row["panic"] = guarded(func() any {
				groups := [][]string{}
				for group := range m.Values() {
					groups = append(groups, group)
				}
				// The groups arrive in map order; only their contents and each
				// group's own order are specified.
				slices.SortFunc(groups, func(left, right []string) int {
					return slices.Compare(left, right)
				})
				out := []any{}
				for _, group := range groups {
					out = append(out, listOf(group))
				}
				return out
			})
		case "clear":
			_, panicked := guarded(func() any { m.Clear(); return nil })
			row["panic"] = panicked
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func replayCopyOnWriteMap(request leafRequest) []any {
	var c *collections.CopyOnWriteMap[string, string]
	var scopes []func()
	ordered := []any{}
	for _, a := range decodeAction(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new":
			c = &collections.CopyOnWriteMap[string, string]{}
			scopes = nil
		case "get":
			row["result"], row["panic"] = guarded(func() any {
				value, ok := c.Get(a.Key)
				return []any{value, ok}
			})
		case "has":
			row["result"], row["panic"] = guarded(func() any { return c.Has(a.Key) })
		case "set":
			_, panicked := guarded(func() any { c.Set(a.Key, a.Value); return nil })
			row["panic"] = panicked
		case "enter_scope":
			_, panicked := guarded(func() any {
				scopes = append(scopes, c.EnterScope())
				return nil
			})
			row["panic"] = panicked
			row["depth"] = len(scopes)
		case "exit_scope":
			if len(scopes) == 0 {
				row["unsupported_action"] = "exit_scope without a matching enter_scope"
				break
			}
			restore := scopes[len(scopes)-1]
			scopes = scopes[:len(scopes)-1]
			_, panicked := guarded(func() any { restore(); return nil })
			row["panic"] = panicked
			row["depth"] = len(scopes)
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func replayCopyOnWriteSet(request leafRequest) []any {
	var c *collections.CopyOnWriteSet[string]
	var scopes []func()
	ordered := []any{}
	for _, a := range decodeAction(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new":
			c = &collections.CopyOnWriteSet[string]{}
			scopes = nil
		case "has":
			row["result"], row["panic"] = guarded(func() any { return c.Has(a.Key) })
		case "add":
			_, panicked := guarded(func() any { c.Add(a.Key); return nil })
			row["panic"] = panicked
		case "enter_scope":
			_, panicked := guarded(func() any {
				scopes = append(scopes, c.EnterScope())
				return nil
			})
			row["panic"] = panicked
			row["depth"] = len(scopes)
		case "exit_scope":
			if len(scopes) == 0 {
				row["unsupported_action"] = "exit_scope without a matching enter_scope"
				break
			}
			restore := scopes[len(scopes)-1]
			scopes = scopes[:len(scopes)-1]
			_, panicked := guarded(func() any { restore(); return nil })
			row["panic"] = panicked
			row["depth"] = len(scopes)
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

// syncMapEntries renders a sync-backed map as a sorted entry array. sync.Map
// specifies no visit order, so anything derived from Range must be sorted or
// reduced to a count before it is recorded.
func syncMapEntries(pairs map[string]any) any {
	names := make([]string, 0, len(pairs))
	for key := range pairs {
		names = append(names, key)
	}
	slices.Sort(names)
	out := make([]any, 0, len(names))
	for _, key := range names {
		out = append(out, []any{key, pairs[key]})
	}
	return out
}

// The value type is `any` so a trace can store an untyped nil: that is the
// only way to reach the `val == nil` branch of Load and the `actualAny == nil`
// branch of LoadOrStore, which is what the pinned syncmap_test pins.
func replaySyncMap(request leafRequest) []any {
	var m *collections.SyncMap[string, any]
	var clone *collections.SyncMap[string, any]
	ordered := []any{}
	for _, a := range decodeAction(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new":
			m = &collections.SyncMap[string, any]{}
			clone = nil
		case "store":
			_, panicked := guarded(func() any { m.Store(a.Key, a.Value); return nil })
			row["panic"] = panicked
		case "store_nil":
			_, panicked := guarded(func() any { m.Store(a.Key, nil); return nil })
			row["panic"] = panicked
		case "load":
			row["result"], row["panic"] = guarded(func() any {
				value, ok := m.Load(a.Key)
				return []any{value, ok}
			})
		case "load_or_store":
			row["result"], row["panic"] = guarded(func() any {
				actual, loaded := m.LoadOrStore(a.Key, a.Value)
				return []any{actual, loaded}
			})
		case "load_or_store_nil":
			row["result"], row["panic"] = guarded(func() any {
				actual, loaded := m.LoadOrStore(a.Key, nil)
				return []any{actual, loaded}
			})
		case "delete":
			_, panicked := guarded(func() any { m.Delete(a.Key); return nil })
			row["panic"] = panicked
		case "clear":
			_, panicked := guarded(func() any { m.Clear(); return nil })
			row["panic"] = panicked
		case "size":
			row["result"], row["panic"] = guarded(func() any { return m.Size() })
		case "keys":
			row["result"], row["panic"] = guarded(func() any {
				names := []string{}
				for key := range m.Keys() {
					names = append(names, key)
				}
				return sortedStrings(names)
			})
		case "to_map":
			row["result"], row["panic"] = guarded(func() any { return syncMapEntries(m.ToMap()) })
		case "range":
			row["result"], row["panic"] = guarded(func() any {
				seen := map[string]any{}
				m.Range(func(key string, value any) bool {
					seen[key] = value
					return true
				})
				return syncMapEntries(seen)
			})
		case "range_stop":
			// Only the number of visits is specified. sync.Map does not
			// specify which entries a stopped Range saw, so recording the keys
			// would record the scheduler instead of the contract.
			row["result"], row["panic"] = guarded(func() any {
				count := 0
				m.Range(func(_ string, _ any) bool {
					count++
					return count < a.Stop
				})
				return count
			})
		case "clone":
			_, panicked := guarded(func() any { clone = m.Clone(); return nil })
			row["panic"] = panicked
		case "clone_size":
			row["result"], row["panic"] = guarded(func() any { return clone.Size() })
		case "clone_keys":
			row["result"], row["panic"] = guarded(func() any {
				names := []string{}
				for key := range clone.Keys() {
					names = append(names, key)
				}
				return sortedStrings(names)
			})
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func replaySyncSet(request leafRequest) []any {
	var s *collections.SyncSet[string]
	ordered := []any{}
	for _, a := range decodeAction(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new":
			s = &collections.SyncSet[string]{}
		case "add":
			_, panicked := guarded(func() any { s.Add(a.Key); return nil })
			row["panic"] = panicked
		case "add_if_absent":
			row["result"], row["panic"] = guarded(func() any { return s.AddIfAbsent(a.Key) })
		case "has":
			row["result"], row["panic"] = guarded(func() any { return s.Has(a.Key) })
		case "delete":
			_, panicked := guarded(func() any { s.Delete(a.Key); return nil })
			row["panic"] = panicked
		case "size":
			row["result"], row["panic"] = guarded(func() any { return s.Size() })
		case "is_empty":
			row["result"], row["panic"] = guarded(func() any { return s.IsEmpty() })
		case "to_slice":
			row["result"], row["panic"] = guarded(func() any { return sortedStrings(s.ToSlice()) })
		case "keys":
			row["result"], row["panic"] = guarded(func() any {
				names := []string{}
				for key := range s.Keys() {
					names = append(names, key)
				}
				return sortedStrings(names)
			})
		case "range_stop":
			row["result"], row["panic"] = guarded(func() any {
				count := 0
				s.Range(func(_ string) bool {
					count++
					return count < a.Stop
				})
				return count
			})
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

// replayFor maps a request's declared subject to the trace player that serves
// it. A subject with no player is not this probe's: the schedule is shared
// with the other leaf groups, and every request still gets a row.
func replayFor(subject string) func(leafRequest) []any {
	switch subject {
	case "OrderedMap":
		return replayOrderedMap
	case "OrderedSet":
		return replayOrderedSet
	case "Set":
		return replaySet
	case "MultiMap":
		return replayMultiMap
	case "CopyOnWriteMap":
		return replayCopyOnWriteMap
	case "CopyOnWriteSet":
		return replayCopyOnWriteSet
	case "SyncMap":
		return replaySyncMap
	case "SyncSet":
		return replaySyncSet
	}
	return nil
}

func TestPhase1LeavesCollections(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []leafRequest `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if replay := replayFor(request.Subject); replay != nil {
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": replay(request)}
		} else {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the collections probe"
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
	data, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
