// Phase 1 mutation witnesses: the facts oracle's native driver.
//
// scripts/phase1_mutation_go.py copies this file into internal/phase1facts of
// a git-archive export of the pin (upstream/ is never edited) and builds it as
// a main package. Each request is an S06 primary parse request
// (data/s06/requests.json); the parse is the S06 oracle's own call
// (scripts/s06_oracle/behavior.go parse). The subtree_facts stage then records,
// for every node in document order, the pair [kind, SubtreeFacts()] through
// the public production API (*ast.Node).SubtreeFacts.
//
// Document order is a pre-order walk of (*ast.Node).ForEachChild from the
// SourceFile, the walk the S06 decoded_tree observer uses: a node, then each
// child subtree in ForEachChild order. The walk runs first, as observation, and
// only then is SubtreeFacts called on each walked node in that order.
//
// Usage: phase1facts <requests.ndjson> <rows.ndjson>. Each output line is
// {"row","outcomes":{"parse","subtree_facts"},"messages"?,"digest"?,"nodes",
// "micros"} where digest is the sha256 of the canonical JSON list
// [[kind,facts],...] (no whitespace; the pairs computed before a panic when
// the stage panics). With PHASE1_FACTS_RAW=1 the list itself is added as
// "list" so the digest can be recomputed independently.
//
// Coverage segments (inert in the plain build, see phase1_cover_off.go):
// "parse" (production), "facts:walk" (observation) and "subtree_facts"
// (production).
package main

import (
	"bufio"
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"strconv"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

type phase1FactsRequest struct {
	Version       int      `json:"version"`
	ID            string   `json:"id"`
	RequestSHA256 string   `json:"request_sha256"`
	Op            string   `json:"op"`
	Operations    []string `json:"operations"`
	Primary       string   `json:"primary"`
	Filename      string   `json:"filename"`
	Path          string   `json:"path"`
	JSX           bool     `json:"jsx"`
	Force         bool     `json:"force"`
	ScriptKind    int64    `json:"script_kind"`
	SourceHex     string   `json:"source_hex"`
}

func fatal(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "phase1facts: "+format+"\n", args...)
	os.Exit(2)
}

// stage runs one pinned stage, turning a panic of the pinned code into the
// stage's outcome; the open coverage segment is closed either way.
func stage(id string, name string, outcomes map[string]string, messages map[string]string, action func()) (ok bool) {
	defer func() {
		if value := recover(); value != nil {
			phase1CoverEnd(id)
			outcomes[name] = "panic"
			messages[name] = hex.EncodeToString([]byte(fmt.Sprint(value)))
			ok = false
		}
	}()
	phase1CoverBegin(name)
	action()
	phase1CoverEnd(id)
	outcomes[name] = "ok"
	return true
}

func walk(root *ast.Node) []*ast.Node {
	nodes := []*ast.Node{}
	stack := []*ast.Node{root}
	for len(stack) > 0 {
		node := stack[len(stack)-1]
		stack = stack[:len(stack)-1]
		nodes = append(nodes, node)
		children := []*ast.Node{}
		node.ForEachChild(func(child *ast.Node) bool { children = append(children, child); return false })
		for i := len(children) - 1; i >= 0; i-- {
			stack = append(stack, children[i])
		}
	}
	return nodes
}

func observe(req *phase1FactsRequest, raw bool) map[string]any {
	source, err := hex.DecodeString(req.SourceHex)
	if err != nil {
		fatal("%s: source_hex: %v", req.ID, err)
	}
	started := time.Now()
	outcomes := map[string]string{"parse": "not_run", "subtree_facts": "not_run"}
	messages := map[string]string{}
	row := map[string]any{"row": req.ID, "outcomes": outcomes}
	var sf *ast.SourceFile
	if stage(req.ID, "parse", outcomes, messages, func() {
		opts := ast.SourceFileParseOptions{FileName: req.Filename, Path: tspath.Path(req.Path),
			ExternalModuleIndicatorOptions: ast.ExternalModuleIndicatorOptions{JSX: req.JSX, Force: req.Force}}
		sf = parser.ParseSourceFile(opts, string(source), core.ScriptKind(req.ScriptKind))
	}) {
		var nodes []*ast.Node
		var facts []ast.SubtreeFacts
		walked := stage(req.ID, "facts:walk", outcomes, messages, func() { nodes = walk(sf.AsNode()) })
		delete(outcomes, "facts:walk")
		if !walked {
			outcomes["subtree_facts"] = "panic"
			messages["subtree_facts"] = messages["facts:walk"]
			delete(messages, "facts:walk")
		} else {
			stage(req.ID, "subtree_facts", outcomes, messages, func() {
				facts = make([]ast.SubtreeFacts, 0, len(nodes))
				for _, node := range nodes {
					facts = append(facts, node.SubtreeFacts())
				}
			})
		}
		var buffer bytes.Buffer
		buffer.WriteByte('[')
		for i, value := range facts {
			if i > 0 {
				buffer.WriteByte(',')
			}
			buffer.WriteByte('[')
			buffer.WriteString(strconv.Itoa(int(nodes[i].Kind)))
			buffer.WriteByte(',')
			buffer.WriteString(strconv.FormatUint(uint64(value), 10))
			buffer.WriteByte(']')
		}
		buffer.WriteByte(']')
		digest := sha256.Sum256(buffer.Bytes())
		row["digest"] = hex.EncodeToString(digest[:])
		row["nodes"] = len(facts)
		if raw {
			row["list"] = json.RawMessage(buffer.Bytes())
		}
	}
	if len(messages) > 0 {
		row["messages"] = messages
	}
	row["micros"] = time.Since(started).Microseconds()
	return row
}

func main() {
	if len(os.Args) != 3 {
		fatal("usage: phase1facts <requests.ndjson> <rows.ndjson>")
	}
	input, err := os.Open(os.Args[1])
	if err != nil {
		fatal("%v", err)
	}
	defer input.Close()
	output, err := os.Create(os.Args[2])
	if err != nil {
		fatal("%v", err)
	}
	raw := os.Getenv("PHASE1_FACTS_RAW") == "1"
	reader := bufio.NewReaderSize(input, 1<<20)
	writer := bufio.NewWriter(output)
	seen := map[string]bool{}
	for {
		line, err := reader.ReadBytes('\n')
		if err != nil && !errors.Is(err, io.EOF) {
			fatal("%v", err)
		}
		last := err != nil
		if len(bytes.TrimSpace(line)) == 0 {
			if last {
				break
			}
			fatal("blank request line")
		}
		var req phase1FactsRequest
		decoder := json.NewDecoder(bytes.NewReader(line))
		decoder.DisallowUnknownFields()
		if err := decoder.Decode(&req); err != nil {
			fatal("request: %v", err)
		}
		if req.ID == "" || seen[req.ID] || req.Op != "parse" {
			fatal("invalid, duplicate or non-parse request %q", req.ID)
		}
		seen[req.ID] = true
		encoded, err := json.Marshal(observe(&req, raw))
		if err != nil {
			fatal("%s: %v", req.ID, err)
		}
		writer.Write(encoded)
		writer.WriteByte('\n')
		// One flushed line per row: a crash loses no finished row.
		if err := writer.Flush(); err != nil {
			fatal("%v", err)
		}
		if last {
			break
		}
	}
	if err := output.Close(); err != nil {
		fatal("%v", err)
	}
}
