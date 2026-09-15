package compiler

import "github.com/microsoft/TypeScript/tsc/internal/core"

// Checker construction at its original acquisition point (checkerPool.createCheckers).
func s08CheckerInitBegin() { core.S08Bench.Push(core.S08PhaseInit) }
func s08CheckerInitEnd()   { core.S08Bench.Pop() }
