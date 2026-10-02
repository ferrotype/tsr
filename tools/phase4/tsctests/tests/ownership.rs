//! X7: the C6/E3 retention contract through the actual command-line paths.
use phase4_tsctests::{
    execute::{
        command_line,
        fswatch::{Event, EventKind},
        tsc::{CommandLineTesting, ExitStatus, SharedWriter, Trace},
        watchmanager::{CommandLineTestingWithWatchBackend, WatchBackend},
    },
    runner::TscInput,
    sys::{new_test_sys, TestSys},
};
use std::sync::{Arc, Mutex, Weak};
use tsr_arena::{Counters, Counts};
use tsr_compiler::{CheckedProgram, EmitResult, ProgramFile};
use tsr_core::collections::SyncMap;
use tsr_ipc::Context;
use tsr_jsstring::JsString;
use tsr_locale::Locale;
use tsr_vfs::{iofs::Time, vfstest::InputFile};

const ROOT: &str = "/home/src/workspaces/project/";
struct Observation {
    counters: Counters,
    file: Weak<ProgramFile>,
    retained: Option<Arc<ProgramFile>>,
    text: Vec<u8>,
}
struct Observer {
    sys: Arc<TestSys>,
    observations: Mutex<Vec<Observation>>,
}
impl CommandLineTestingWithWatchBackend for Observer {
    fn watch_backend(&self) -> Arc<dyn WatchBackend> {
        self.sys.mock_watch_backend.clone()
    }
}
impl CommandLineTesting for Observer {
    fn on_compiler_program(&self, checked: &CheckedProgram) {
        let mut observations = self.observations.lock().unwrap();
        let file = checked
            .program()
            .files()
            .iter()
            .find(|f| {
                f.bound().view().source_file().unwrap().file_name()
                    == format!("{ROOT}index.ts").as_bytes()
            })
            .expect("entry file");
        let retain = observations.len() < 2;
        observations.push(Observation {
            counters: file.bound().view().ast().counters().clone(),
            file: Arc::downgrade(file),
            retained: retain.then(|| file.clone()),
            text: file.bound().view().ast().source().as_bytes().to_vec(),
        });
    }
    fn on_program(&self, _: &tsr_incremental::Program) {}
    fn on_emitted_files(
        &self,
        result: Option<&EmitResult>,
        cache: Option<&SyncMap<JsString, Time>>,
    ) {
        self.sys.on_emitted_files(result, cache);
    }
    fn on_list_files_start(&self, _: &SharedWriter) {}
    fn on_list_files_end(&self, _: &SharedWriter) {}
    fn on_statistics_start(&self, _: &SharedWriter) {}
    fn on_statistics_end(&self, _: &SharedWriter) {}
    fn on_build_status_report_start(&self, _: &SharedWriter) {}
    fn on_build_status_report_end(&self, _: &SharedWriter) {}
    fn on_watch_status_report_start(&self) {}
    fn on_watch_status_report_end(&self) {}
    fn get_trace(&self, writer: SharedWriter, locale: Locale) -> Trace {
        self.sys.get_trace(writer, locale)
    }
    fn as_with_watch_backend(&self) -> Option<&dyn CommandLineTestingWithWatchBackend> {
        Some(self)
    }
}
fn setup() -> Arc<Observer> {
    let files = [
        ("tsconfig.json", r#"{"compilerOptions":{"composite":true,"outDir":"out"},"files":["index.ts","dep.ts"]}"#),
        ("index.ts", "import { dep } from './dep'; export const value: number = dep + 100;"),
        ("dep.ts", "export const dep: number = 1;"),
    ].into_iter().map(|(name, text)| (format!("{ROOT}{name}").into_bytes(), InputFile::Text(text.as_bytes().to_vec()))).collect();
    Arc::new(Observer {
        sys: new_test_sys(
            &TscInput {
                files,
                ..Default::default()
            },
            false,
        ),
        observations: Mutex::new(Vec::new()),
    })
}
fn edit(observer: &Observer, generation: usize) {
    let text = format!(
        "import {{ dep }} from './dep'; export const value: number = dep + {};",
        100 + generation
    );
    observer
        .sys
        .fs_from_file_map()
        .write_file(format!("{ROOT}index.ts").as_bytes(), text.as_bytes())
        .unwrap();
    observer.sys.clear_output();
}
fn args(args: &[&str]) -> Vec<JsString> {
    args.iter()
        .map(|s| JsString::from_bytes(s.as_bytes()))
        .collect()
}
fn retained_readable(observations: &[Observation]) {
    for observation in observations.iter().take(2) {
        let file = observation.retained.as_ref().unwrap();
        assert_eq!(
            file.bound().view().ast().source().as_bytes(),
            observation.text
        );
        assert!(file.bound().view().node(file.source()).is_ok());
        assert!(observation.counters.snapshot().owners > 0);
    }
    if observations.len() >= 2 {
        assert_ne!(
            observations[0].retained.as_ref().unwrap().source(),
            observations[1].retained.as_ref().unwrap().source()
        );
    }
}
fn release_and_assert_empty(observer: &Observer) {
    let mut observations = observer.observations.lock().unwrap();
    retained_readable(&observations);
    for observation in observations.iter_mut() {
        observation.retained.take();
    }
    for (generation, observation) in observations.iter().enumerate() {
        assert!(
            observation.file.upgrade().is_none(),
            "generation {generation} still retained"
        );
        assert_eq!(
            observation.counters.snapshot(),
            Counts::default(),
            "generation {generation} leaked tracked owners or allocations"
        );
    }
}
fn repeat_command(arguments: &[&str], build_info: bool) {
    let observer = setup();
    // Each invocation creates a fresh domain, whose starting counts are zero.
    for generation in 0..50 {
        edit(&observer, generation);
        let result = command_line(
            &Context::background(),
            observer.sys.clone(),
            &args(arguments),
            Some(observer.clone()),
        );
        assert_eq!(
            result.status,
            ExitStatus::Success,
            "{}",
            String::from_utf8_lossy(&observer.sys.current_write().string())
        );
        assert!(result.watcher.is_none());
        drop(result);
        let observations = observer.observations.lock().unwrap();
        assert_eq!(
            observations.len(),
            generation + 1,
            "command must actually compile"
        );
        retained_readable(&observations);
        for observation in observations.iter().skip(2) {
            assert_eq!(observation.counters.snapshot(), Counts::default());
            assert!(observation.file.upgrade().is_none());
        }
        if build_info {
            assert!(observer
                .sys
                .fs_from_file_map()
                .file_exists(format!("{ROOT}out/tsconfig.tsbuildinfo").as_bytes()));
        }
    }
    release_and_assert_empty(&observer);
}
#[test]
fn compile_releases_fifty_generations_after_retained_files_drop() {
    repeat_command(
        &[
            "--project",
            "tsconfig.json",
            "--composite",
            "false",
            "--incremental",
            "false",
            "--pretty",
            "false",
        ],
        false,
    );
}
#[test]
fn incremental_compile_releases_fifty_generations_after_retained_files_drop() {
    repeat_command(&["--project", "tsconfig.json", "--pretty", "false"], true);
}
#[test]
fn build_releases_fifty_generations_after_retained_files_drop() {
    repeat_command(&["--build", "--force", "--pretty", "false"], true);
}
#[test]
fn watch_fifty_cycles_plateau_after_second_cycle_and_release_on_close() {
    let observer = setup();
    let result = command_line(
        &Context::background(),
        observer.sys.clone(),
        &args(&["--watch", "--pretty", "false"]),
        Some(observer.clone()),
    );
    assert_eq!(result.status, ExitStatus::Success);
    let watcher = result.watcher.unwrap();
    let mut plateau = None;
    for cycle in 1..=50 {
        edit(&observer, cycle);
        observer.sys.mock_watch_backend.send_events(&[Event {
            kind: EventKind::EventUpdate,
            path: format!("{ROOT}index.ts").into_bytes(),
        }]);
        watcher.do_cycle();
        let observations = observer.observations.lock().unwrap();
        assert_eq!(observations.len(), cycle + 1);
        retained_readable(&observations);
        let counts = observations.last().unwrap().counters.snapshot();
        if cycle == 2 {
            plateau = Some(counts);
        }
        if cycle > 2 {
            assert_eq!(
                Some(counts),
                plateau,
                "watch cycle {cycle} grew tracked storage"
            );
        }
        for old in observations.iter().skip(2).take(cycle.saturating_sub(2)) {
            assert!(old.file.upgrade().is_none());
        }
    }
    drop(watcher);
    release_and_assert_empty(&observer);
}
