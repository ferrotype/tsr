//! Pinned statistics table, including explicit allocator availability.
use crate::{CommandLineTesting, CompileTimes, MemoryStatistics, SharedWriter};
use std::time::Duration;
use tsr_compiler::{CheckedProgram, Error};

#[derive(Clone, Default)]
pub struct Statistics {
    pub is_aggregate: bool,
    pub projects: i64,
    pub projects_built: i64,
    pub timestamp_updates: i64,
    pub files: i64,
    pub lines: i64,
    pub identifiers: i64,
    pub symbols: i64,
    pub types: i64,
    pub instantiations: i64,
    pub memory_used: Option<u64>,
    pub memory_allocs: Option<u64>,
    pub compile_times: CompileTimes,
}

impl Statistics {
    // port: tsc/internal/execute/tsc/statistics.go:statisticsFromProgram
    pub fn from_program(
        program: &CheckedProgram,
        compile_times: &CompileTimes,
        memory: Option<MemoryStatistics>,
    ) -> Result<Self, Error> {
        Ok(Self {
            files: program.program().files().len() as i64,
            lines: program.program().line_count()?,
            identifiers: program.program().identifier_count()?,
            symbols: i64::from(program.symbol_count()?),
            types: i64::from(program.type_count()?),
            instantiations: i64::from(program.instantiation_count()?),
            memory_used: memory.map(|memory| memory.live_bytes),
            memory_allocs: memory.map(|memory| memory.allocation_count),
            compile_times: compile_times.clone(),
            ..Self::default()
        })
    }
    /// port: tsc/internal/execute/tsc/statistics.go:Statistics.Report
    pub fn report(&self, writer: &SharedWriter, testing: Option<&dyn CommandLineTesting>) {
        struct End<'a>(Option<&'a dyn CommandLineTesting>, &'a SharedWriter);
        impl Drop for End<'_> {
            fn drop(&mut self) {
                if let Some(testing) = self.0 {
                    testing.on_statistics_end(self.1);
                }
            }
        }
        if let Some(testing) = testing {
            testing.on_statistics_start(writer);
        }
        let _end = End(testing, writer);
        let mut table = Table::default();
        let prefix = if self.is_aggregate {
            table.add("Projects in scope", self.projects);
            table.add("Projects built", self.projects_built);
            table.add("Timestamps only updates", self.timestamp_updates);
            "Aggregate "
        } else {
            ""
        };
        table.add(&format!("{prefix}Files"), self.files);
        table.add(&format!("{prefix}Lines"), self.lines);
        table.add(&format!("{prefix}Identifiers"), self.identifiers);
        table.add(&format!("{prefix}Symbols"), self.symbols);
        table.add(&format!("{prefix}Types"), self.types);
        table.add(&format!("{prefix}Instantiations"), self.instantiations);
        table.add(
            &format!("{prefix}Memory used"),
            self.memory_used.map_or_else(
                || "unavailable".into(),
                |value| format!("{}K", value / 1024),
            ),
        );
        table.add(
            &format!("{prefix}Memory allocs"),
            self.memory_allocs
                .map_or_else(|| "unavailable".into(), |value| value.to_string()),
        );
        let times = &self.compile_times;
        for (name, duration, required) in [
            ("Config time", times.config_time, false),
            ("BuildInfo read time", times.build_info_read_time, false),
            ("Parse time", times.parse_time, true),
            ("Bind time", times.bind_time, false),
            ("Check time", times.check_time, false),
            ("Emit time", times.emit_time, false),
            ("Changes compute time", times.changes_compute_time, false),
        ] {
            if required || duration != Duration::ZERO {
                table.add(&format!("{prefix}{name}"), format_duration(duration));
            }
        }
        // port: tsc/internal/execute/tsc/statistics.go:Statistics.addContentMapperStatistics
        if times.content_mapper_times.request_wait != Duration::ZERO {
            table.add(
                &format!("{prefix}Content mapper request wait time"),
                format_duration(times.content_mapper_times.request_wait),
            );
        }
        let mut mappers: Vec<_> = times.content_mapper_times.mappers.iter().collect();
        mappers.sort_unstable_by_key(|(identity, _)| *identity);
        for (identity, mapper) in mappers {
            for (name, count, duration) in [
                (
                    "initialization",
                    mapper.spawn.count,
                    mapper.spawn.duration + mapper.initialize.duration,
                ),
                (
                    "transform",
                    mapper.transform.count,
                    mapper.transform.duration,
                ),
                (
                    "openProject",
                    mapper.open_project.count,
                    mapper.open_project.duration,
                ),
                (
                    "closeProject",
                    mapper.close_project.count,
                    mapper.close_project.duration,
                ),
            ] {
                if count != 0 {
                    table.add(
                        &format!("{prefix}{identity} {name} time"),
                        format_duration(duration),
                    );
                }
            }
        }
        table.add(
            &format!("{prefix}Total time"),
            format_duration(times.total_time),
        );
        table.print(writer);
    }
    /// port: tsc/internal/execute/tsc/statistics.go:Statistics.Aggregate
    pub fn aggregate(&mut self, stat: &Self) {
        let first = !self.is_aggregate;
        self.is_aggregate = true;
        self.files = self.files.wrapping_add(stat.files);
        self.lines = self.lines.wrapping_add(stat.lines);
        self.identifiers = self.identifiers.wrapping_add(stat.identifiers);
        self.symbols = self.symbols.wrapping_add(stat.symbols);
        self.types = self.types.wrapping_add(stat.types);
        self.instantiations = self.instantiations.wrapping_add(stat.instantiations);
        self.memory_used = if first {
            stat.memory_used
        } else {
            self.memory_used
                .zip(stat.memory_used)
                .map(|(a, b)| a.wrapping_add(b))
        };
        self.memory_allocs = if first {
            stat.memory_allocs
        } else {
            self.memory_allocs
                .zip(stat.memory_allocs)
                .map(|(a, b)| a.wrapping_add(b))
        };
        let times = &mut self.compile_times;
        times.config_time += stat.compile_times.config_time;
        times.build_info_read_time += stat.compile_times.build_info_read_time;
        times.parse_time += stat.compile_times.parse_time;
        times.bind_time += stat.compile_times.bind_time;
        times.check_time += stat.compile_times.check_time;
        times.emit_time += stat.compile_times.emit_time;
        times.changes_compute_time += stat.compile_times.changes_compute_time;
        // The pin deliberately does not aggregate total or mapper durations.
    }
    // port: tsc/internal/execute/tsc/statistics.go:Statistics.SetTotalTime
    pub fn set_total_time(&mut self, total_time: Duration) {
        self.compile_times.total_time = total_time;
    }
}

// port: tsc/internal/execute/tsc/statistics.go:formatDuration
fn format_duration(duration: Duration) -> String {
    format!("{:.3}s", duration.as_secs_f64())
}

#[derive(Default)]
struct Table {
    rows: Vec<(String, String)>,
}
impl Table {
    // port: tsc/internal/execute/tsc/statistics.go:table.add
    fn add(&mut self, name: &str, value: impl std::fmt::Display) {
        self.rows.push((name.into(), value.to_string()));
    }
    // port: tsc/internal/execute/tsc/statistics.go:table.print
    fn print(&self, writer: &SharedWriter) {
        // Width selection uses byte lengths, while Go fmt's padding counts
        // Unicode code points. Mapper identities can make that distinction visible.
        let name_width = self
            .rows
            .iter()
            .map(|(name, _)| name.len())
            .max()
            .unwrap_or(0)
            + 1;
        let value_width = self
            .rows
            .iter()
            .map(|(_, value)| value.len())
            .max()
            .unwrap_or(0);
        for (name, value) in &self.rows {
            let name = format!("{name}:");
            let line = format!("{name:<name_width$} {value:>value_width$}\n");
            crate::write_all(writer.as_ref(), line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests;
