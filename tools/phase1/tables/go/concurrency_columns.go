// Phase 1 operation tables: group concurrency (core concurrency, request context, BFS).
// Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/concurrency.rs and the spec is
// data/phase1/tables/concurrency.json.
package main

import (
	"context"
	"errors"
	"slices"
	"sync"
	"sync/atomic"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/core"
)

// bfsGraph is a search over string nodes: the edges, the start, the nodes
// visit reports as results and the ones it also stops at.
type bfsGraph struct {
	Edges   map[string][]string `json:"edges"`
	Start   string              `json:"start"`
	Results []string            `json:"results"`
	Stops   []string            `json:"stops"`
	// Ex only: keys visited before the search, the length of the key prefix
	// (0 for the whole node), and per level the keys PreprocessLevel deletes,
	// the key it asks Has about and how many nodes Range reads.
	Visited []string     `json:"visited"`
	KeyLen  int          `json:"key_len"`
	Levels  []levelPrune `json:"levels"`
}

type levelPrune struct {
	Delete []string `json:"delete"`
	Has    string   `json:"has"`
	Range  int      `json:"range"`
}

func (g bfsGraph) visit(node string) (bool, bool) {
	stop := slices.Contains(g.Stops, node)
	return stop || slices.Contains(g.Results, node), stop
}

func (g bfsGraph) key(node string) string {
	if g.KeyLen > 0 && len(node) > g.KeyLen {
		return node[:g.KeyLen]
	}
	return node
}

func bfsValue(result core.BreadthFirstSearchResult[string]) any {
	path := []any{}
	for _, node := range result.Path {
		path = append(path, Hex(node))
	}
	return []any{result.Stopped, path}
}

func init() {
	Register("concurrency",
		// Operations on one context, each With deriving the next: the value
		// is each Get's result.
		typedValuesColumn("core.RequestContext", func(in struct {
			Ops []struct {
				Op       string `json:"op"`
				ID       string `json:"id"`
				Lifetime int    `json:"lifetime"`
			} `json:"ops"`
		}) any {
			ctx := context.Background()
			out := []any{}
			for _, op := range in.Ops {
				switch op.Op {
				case "with_request_id":
					ctx = core.WithRequestID(ctx, op.ID)
				case "with_checker_lifetime":
					ctx = core.WithCheckerLifetime(ctx, core.CheckerLifetime(op.Lifetime))
				case "get_request_id":
					out = append(out, Hex(core.GetRequestID(ctx)))
				case "get_checker_lifetime":
					out = append(out, Scalar(int(core.GetCheckerLifetime(ctx))))
				}
			}
			return out
		}),
		// [stopped, path hex] per graph.
		typedValuesColumn("core.BreadthFirstSearchParallel", func(in struct {
			Graphs []bfsGraph `json:"graphs"`
		}) any {
			out := []any{}
			for _, g := range in.Graphs {
				out = append(out, bfsValue(core.BreadthFirstSearchParallel(g.Start,
					func(node string) []string { return g.Edges[node] }, g.visit)))
			}
			return out
		}),
		// [stopped, path hex, [per level: [Range's nodes hex, Has]]] per
		// graph, keyed by the node's prefix, with a preseeded visited set and
		// a PreprocessLevel that records and prunes each level.
		typedValuesColumn("core.BreadthFirstSearchParallelEx", func(in struct {
			Graphs []bfsGraph `json:"graphs"`
		}) any {
			out := []any{}
			for _, g := range in.Graphs {
				visited := &collections.SyncSet[string]{}
				for _, key := range g.Visited {
					visited.Add(key)
				}
				log := []any{}
				level := 0
				options := core.BreadthFirstSearchOptions[string, string]{
					Visited: visited,
					PreprocessLevel: func(l *core.BreadthFirstSearchLevel[string, string]) {
						prune := levelPrune{}
						if level < len(g.Levels) {
							prune = g.Levels[level]
						}
						level++
						read := []any{}
						l.Range(func(node string) bool {
							if len(read) >= prune.Range {
								return false
							}
							read = append(read, Hex(node))
							return true
						})
						has := l.Has(prune.Has)
						for _, key := range prune.Delete {
							l.Delete(key)
						}
						log = append(log, []any{read, has})
					},
				}
				result := core.BreadthFirstSearchParallelEx(g.Start, func(node string) []string { return g.Edges[node] },
					g.visit, options, g.key)
				value := bfsValue(result).([]any)
				out = append(out, append(value, log))
			}
			return out
		}),
		// Per case: [the jobs' values sorted, Wait's error hex or null, whether
		// no more jobs than the limit ran at once]; at most one job fails.
		typedValuesColumn("core.ThrottleGroup", func(in struct {
			Cases []struct {
				Limit int `json:"limit"`
				Jobs  []struct {
					Value int  `json:"value"`
					Fail  bool `json:"fail"`
				} `json:"jobs"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				group := core.NewThrottleGroup(context.Background(), make(chan struct{}, c.Limit))
				var mu sync.Mutex
				values := []int{}
				var running, most atomic.Int64
				for _, job := range c.Jobs {
					group.Go(func() error {
						now := running.Add(1)
						for {
							seen := most.Load()
							if now <= seen || most.CompareAndSwap(seen, now) {
								break
							}
						}
						mu.Lock()
						values = append(values, job.Value)
						mu.Unlock()
						running.Add(-1)
						if job.Fail {
							return errors.New("job failed")
						}
						return nil
					})
				}
				var failure any
				if err := group.Wait(); err != nil {
					failure = Hex(err.Error())
				}
				slices.Sort(values)
				out = append(out, []any{ints(values), failure, most.Load() <= int64(c.Limit)})
			}
			return out
		}),
		// Per case: [the jobs' values sorted, whether no more jobs than the
		// limit held a permit at once]. A non-positive limit is the declared
		// panic of the constructor, recorded through Guard as the value.
		typedValuesColumn("core.LimitedSemaphore", func(in struct {
			Cases []struct {
				Limit int   `json:"limit"`
				Jobs  []int `json:"jobs"`
			} `json:"cases"`
		}) any {
			return Guard([]string{"message:maxConcurrency must be positive"}, func() any {
				out := []any{}
				for _, c := range in.Cases {
					semaphore := core.NewLimitedSemaphore(c.Limit)
					var mu sync.Mutex
					values := []int{}
					var running, most atomic.Int64
					var wg sync.WaitGroup
					for _, job := range c.Jobs {
						wg.Add(1)
						go func() {
							defer wg.Done()
							release := semaphore.Acquire()
							now := running.Add(1)
							for {
								seen := most.Load()
								if now <= seen || most.CompareAndSwap(seen, now) {
									break
								}
							}
							mu.Lock()
							values = append(values, job)
							mu.Unlock()
							// Hold the permit long enough for the other jobs to
							// contend for it, so an unbounded semaphore shows.
							time.Sleep(10 * time.Millisecond)
							running.Add(-1)
							release()
						}()
					}
					wg.Wait()
					slices.Sort(values)
					out = append(out, []any{ints(values), most.Load() <= int64(c.Limit)})
				}
				return out
			})
		}),
	)
}
