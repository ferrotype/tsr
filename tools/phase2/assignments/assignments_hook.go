package compiler

// Phase 2 C6.7 diagnostic overlay (scripts/phase2_assignments.py); never an
// input of the canonical native oracle. createCheckers reports, per program,
// the association inputs it read and what it computed from them.

import (
	"slices"
	"sync"
)

// Phase2ObserveAssignments receives one record per createCheckers call.
var Phase2ObserveAssignments func(record map[string]any)

var phase2AssignmentsMu sync.Mutex

func phase2Sorted(adjacentFiles [][]int) [][]int {
	adjacency := make([][]int, len(adjacentFiles))
	for i, adjacent := range adjacentFiles {
		sorted := append(make([]int, 0, len(adjacent)), adjacent...)
		slices.Sort(sorted)
		adjacency[i] = sorted
	}
	return adjacency
}

func phase2Policy(policy checkerAssociationPolicy) map[string]any {
	return map[string]any{
		"prioritize_source_files":       policy.prioritizeSourceFiles,
		"source_file_weight_multiplier": policy.sourceFileWeightMultiplier,
		"balance_penalty_multiplier":    policy.balancePenaltyMultiplier,
	}
}

// phase2RecordAssignments is called at the end of createCheckers' association
// step. With one checker the pin computes nothing but the zero associations.
func phase2RecordAssignments(program *Program, checkerCount int, importCounts []int, isDeclarationFile []bool, adjacentFiles [][]int, policy *checkerAssociationPolicy, fileWeights []int, fileOrder []int, associations []int) {
	files := make([]string, len(program.files))
	nodeCounts := make([]int, len(program.files))
	textLengths := make([]int, len(program.files))
	for i, file := range program.files {
		files[i] = file.FileName()
		nodeCounts[i] = file.NodeCount
		textLengths[i] = len(file.Text())
	}
	record := map[string]any{
		"files":         files,
		"checker_count": checkerCount,
		"node_counts":   nodeCounts,
		"text_lengths":  textLengths,
		"associations":  append(make([]int, 0, len(associations)), associations...),
	}
	if policy != nil {
		record["import_counts"] = importCounts
		record["is_declaration_file"] = isDeclarationFile
		record["adjacency"] = phase2Sorted(adjacentFiles)
		record["policy"] = phase2Policy(*policy)
		record["file_weights"] = fileWeights
		record["order"] = fileOrder
	}
	phase2AssignmentsMu.Lock()
	defer phase2AssignmentsMu.Unlock()
	if Phase2ObserveAssignments != nil {
		Phase2ObserveAssignments(record)
	}
}

// phase2Associate is createCheckers' association step over given per-file
// inputs instead of a program's files: the same helper calls in the same
// order (checkerpool.go, createCheckers). The recorder checks it against every
// record createCheckers itself produced before it trusts it with synthetic
// inputs.
func phase2Associate(checkerCount int, nodeCounts []int, textLengths []int, importCounts []int, isDeclarationFile []bool, adjacentFiles [][]int) map[string]any {
	associations := make([]int, len(nodeCounts))
	result := map[string]any{"checker_count": checkerCount}
	if checkerCount > 1 {
		baseWeights := make([]int, len(nodeCounts))
		totalBaseWeight := 0
		declarationBaseWeight := 0
		for i := range nodeCounts {
			baseWeight := getCheckerAssociationBaseWeight(nodeCounts[i], textLengths[i])
			totalBaseWeight += baseWeight
			if isDeclarationFile[i] {
				declarationBaseWeight += baseWeight
			}
			baseWeights[i] = baseWeight
		}
		policy := getCheckerAssociationPolicy(totalBaseWeight, declarationBaseWeight, checkerCount)
		if policy.sourceFileWeightMultiplier != 1 {
			for i, declaration := range isDeclarationFile {
				if !declaration {
					baseWeights[i] *= policy.sourceFileWeightMultiplier
				}
			}
		}
		fileWeights := getCheckerAssociationWeights(baseWeights, importCounts)
		fileOrder := getCheckerAssociationOrder(fileWeights, isDeclarationFile, policy.prioritizeSourceFiles)
		associations = getCheckerAssociationsInOrder(fileWeights, adjacentFiles, fileOrder, checkerCount, policy.balancePenaltyMultiplier)
		result["policy"] = phase2Policy(policy)
		result["file_weights"] = fileWeights
		result["order"] = fileOrder
	}
	result["associations"] = associations
	return result
}
