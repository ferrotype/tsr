// Phase 1 mutation witnesses: the plain build of a Phase 1 native driver
// (tools/phase1/mutation/go/syntax, tools/phase1/mutation/go/facts).
//
// scripts/phase1_mutation_go.py copies this file, instead of phase1_cover.go,
// into the plain build whose output is frozen, so that binary carries no
// coverage code at all: the segment brackets compile to nothing.
package main

func phase1CoverBegin(string) {}

func phase1CoverEnd(string) {}
