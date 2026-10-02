//! Program counters and access to complete checker-owned trace records.
use crate::{CheckedProgram, Error, Program};
use std::sync::atomic::{AtomicU32, Ordering};

impl Program {
    /// port: tsc/internal/compiler/program.go:Program.LineCount
    pub fn line_count(&self) -> Result<i64, Error> {
        self.files().iter().try_fold(0_i64, |count, file| {
            Ok(count.wrapping_add(file.bound().view().source_file()?.ecma_line_map().len() as i64))
        })
    }
    /// port: tsc/internal/compiler/program.go:Program.IdentifierCount
    /// port: tsc/internal/execute/tsc/statistics.go:identifierCount
    pub fn identifier_count(&self) -> Result<i64, Error> {
        self.files().iter().try_fold(0_i64, |count, file| {
            Ok(count.wrapping_add(file.bound().view().source_file()?.identifier_count))
        })
    }
}

impl CheckedProgram {
    /// Includes symbols created by binding and by every checker. The pin
    /// explicitly aggregates these counters through uint32, including wrap.
    /// port: tsc/internal/compiler/program.go:Program.SymbolCount
    pub fn symbol_count(&self) -> Result<u32, Error> {
        let bound_count = self.program().files().iter().fold(0_u32, |count, file| {
            count.wrapping_add(file.bound().view().result().symbol_count() as u32)
        });
        let count = AtomicU32::new(bound_count);
        self.for_each_checker_parallel(&|_, operation| {
            count.fetch_add(operation.symbol_count(), Ordering::Relaxed);
        })?;
        Ok(count.into_inner())
    }
    /// port: tsc/internal/compiler/program.go:Program.TypeCount
    pub fn type_count(&self) -> Result<u32, Error> {
        let count = AtomicU32::new(0);
        self.for_each_checker_parallel(&|_, operation| {
            count.fetch_add(operation.type_count() as u32, Ordering::Relaxed);
        })?;
        Ok(count.into_inner())
    }
    /// port: tsc/internal/compiler/program.go:Program.InstantiationCount
    pub fn instantiation_count(&self) -> Result<u32, Error> {
        let count = AtomicU32::new(0);
        self.for_each_checker_parallel(&|_, operation| {
            count.fetch_add(operation.instantiation_count() as u32, Ordering::Relaxed);
        })?;
        Ok(count.into_inner())
    }
    /// Resolve the recorded IDs in the checker that allocated them. No type
    /// handles escape its operation, and formatting may safely reenter tracing.
    pub fn trace_type_records(
        &self,
        checker_index: usize,
        ids: &[u32],
    ) -> Result<Vec<tsr_checker::TraceTypeRecord>, Error> {
        let pool = self.compiler_checker_pool().ok_or(Error::Unsupported(
            "trace type dump requires its compiler checker pool",
        ))?;
        let checker = pool
            .checkers()?
            .get(checker_index)
            .ok_or(Error::Unsupported(
                "trace checker index is not in this program",
            ))?;
        Ok(checker.operation()?.trace_type_records(ids)?)
    }
}
