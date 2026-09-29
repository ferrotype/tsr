package compiler

// Phase 2 C6.7 diagnostic overlay: the association step over the per-file
// inputs of PHASE2_ASSIGNMENT_INPUT (one JSON object per line), written to
// PHASE2_ASSIGNMENT_OUTPUT in the same order.

import (
	"bufio"
	"encoding/json"
	"os"
	"runtime"
	"testing"
)

type phase2AssignmentInput struct {
	ID                string  `json:"id"`
	CheckerCount      int     `json:"checker_count"`
	NodeCounts        []int   `json:"node_counts"`
	TextLengths       []int   `json:"text_lengths"`
	ImportCounts      []int   `json:"import_counts"`
	IsDeclarationFile []bool  `json:"is_declaration_file"`
	Adjacency         [][]int `json:"adjacency"`
}

func TestPhase2Assignments(t *testing.T) {
	input, err := os.Open(os.Getenv("PHASE2_ASSIGNMENT_INPUT"))
	if err != nil {
		t.Fatal(err)
	}
	defer input.Close()
	output, err := os.Create(os.Getenv("PHASE2_ASSIGNMENT_OUTPUT"))
	if err != nil {
		t.Fatal(err)
	}
	defer output.Close()
	writer := bufio.NewWriter(output)
	encoder := json.NewEncoder(writer)
	scanner := bufio.NewScanner(input)
	scanner.Buffer(make([]byte, 1<<20), 1<<30)
	rows := 0
	for scanner.Scan() {
		var request phase2AssignmentInput
		if err := json.Unmarshal(scanner.Bytes(), &request); err != nil {
			t.Fatal(err)
		}
		result := phase2Associate(request.CheckerCount, request.NodeCounts, request.TextLengths, request.ImportCounts, request.IsDeclarationFile, request.Adjacency)
		result["id"] = request.ID
		if err := encoder.Encode(result); err != nil {
			t.Fatal(err)
		}
		rows++
	}
	if err := scanner.Err(); err != nil {
		t.Fatal(err)
	}
	if err := writer.Flush(); err != nil {
		t.Fatal(err)
	}
	summary, err := json.Marshal(map[string]any{"rows": rows, "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("PHASE2_ASSIGNMENT_SUMMARY"), summary, 0o644); err != nil {
		t.Fatal(err)
	}
}
