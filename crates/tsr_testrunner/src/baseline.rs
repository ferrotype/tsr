//! `testutil/baseline/baseline.go`: compare a composed baseline with the
//! committed reference and write the differing output under the local
//! directory.
//!
//! One deliberate difference from `go test`: `writeComparison` writes a
//! `<name>.delete` marker and *passes* when the runner produces
//! `<no content>` for a baseline the reference directory still has; the
//! pin's CI then fails on the marker in its baseline check. This runner
//! fails that case directly, so a Rust run is at least as strict as the
//! pin's CI.
use crate::harness::baselines::patience;
use crate::result::Outcome;
use std::path::{Path, PathBuf};

// source: tsc/internal/testutil/baseline/baseline.go:NoContent
pub const NO_CONTENT: &[u8] = b"<no content>";

/// `baseline.Options`; the diff fix-ups only shape the diff text.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options<'a> {
    pub subfolder: &'a str,
    pub skip_diff_with_old: bool,
}

/// `referenceRoot` and `localRoot`.
#[derive(Clone, Debug)]
pub struct Roots {
    pub reference: PathBuf,
    pub local: PathBuf,
}

impl Roots {
    pub fn new(reference: PathBuf, local: PathBuf) -> Self {
        Self { reference, local }
    }
}

/// Lines of the unified diff kept in a failing line's `detail`.
const DIFF_LINES: usize = 60;

/// `baseline.Run(t, fileName, actual, opts)`: pass when `actual` equals the
/// reference (or there is none and `actual` is `<no content>`); otherwise
/// the differing output is written under `local/<subfolder>/<fileName>` and
/// the outcome names the reference's state with a unified diff.
// port: tsc/internal/testutil/baseline/baseline.go:Run
// port: tsc/internal/testutil/baseline/baseline.go:writeComparison
pub fn run(roots: &Roots, file_name: &[u8], actual: &[u8], options: Options<'_>) -> Outcome {
    let relative = Path::new(options.subfolder).join(String::from_utf8_lossy(file_name).as_ref());
    let local = roots.local.join(&relative);
    let marker = PathBuf::from(format!("{}.delete", local.display()));
    let reference = roots.reference.join(&relative);
    if actual.is_empty() {
        return Outcome::fail(
            "the generated content was \"\". Return 'baseline.NoContent' if no baselining is required.",
        );
    }
    // An earlier run's output for this baseline says nothing about this one.
    for stale in [&local, &marker] {
        if let Err(error) = std::fs::remove_file(stale) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Outcome::fail(format!(
                    "failed to remove the local baseline file {}: {error}",
                    stale.display()
                ));
            }
        }
    }
    let expected = std::fs::read(&reference).ok();
    let found_expected = expected.is_some();
    let expected = expected.unwrap_or_else(|| NO_CONTENT.to_vec());
    if expected == actual && !(actual == NO_CONTENT && found_expected) {
        return Outcome::Pass;
    }
    if let Some(parent) = local.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return Outcome::fail(format!(
                "failed to create directories for the local baseline file {}: {error}",
                local.display()
            ));
        }
    }
    if actual == NO_CONTENT {
        if let Err(error) = std::fs::write(&marker, b"") {
            return Outcome::fail(format!(
                "failed to write the local baseline file {}: {error}",
                marker.display()
            ));
        }
        return Outcome::fail(format!(
            "the baseline file {} is no longer produced; the pin's CI would delete it",
            relative.display()
        ));
    }
    if let Err(error) = std::fs::write(&local, actual) {
        return Outcome::fail(format!(
            "failed to write the local baseline file {}: {error}",
            local.display()
        ));
    }
    if !found_expected {
        return Outcome::fail(format!("new baseline created at {}.", relative.display()));
    }
    let diff = if options.skip_diff_with_old {
        String::new()
    } else {
        diff_detail(&relative, &expected, actual)
    };
    Outcome::fail_with(
        format!("the baseline file {} has changed.", relative.display()),
        diff,
    )
}

/// The first lines of the unified diff of the reference against `actual`
/// (`baseline.DiffText`).
// port: tsc/internal/testutil/baseline/baseline.go:DiffText
pub fn diff_detail(relative: &Path, expected: &[u8], actual: &[u8]) -> String {
    let name = relative.display().to_string();
    let text = patience::diff_text(
        format!("reference/{name}").as_bytes(),
        format!("local/{name}").as_bytes(),
        expected,
        actual,
    );
    let text = String::from_utf8_lossy(&text);
    let mut lines: Vec<&str> = text.lines().take(DIFF_LINES + 1).collect();
    if lines.len() > DIFF_LINES {
        lines.truncate(DIFF_LINES);
        lines.push("…");
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{run, Options, Roots, NO_CONTENT};
    use crate::result::Outcome;

    fn roots() -> (tempdir::Dir, Roots) {
        let dir = tempdir::Dir::new();
        let roots = Roots::new(dir.path.join("reference"), dir.path.join("local"));
        std::fs::create_dir_all(roots.reference.join("compiler")).unwrap();
        (dir, roots)
    }

    #[test]
    fn equal_content_passes_and_nothing_is_written() {
        let (_dir, roots) = roots();
        std::fs::write(roots.reference.join("compiler/a.js"), b"x\r\n").unwrap();
        let outcome = run(
            &roots,
            b"a.js",
            b"x\r\n",
            Options {
                subfolder: "compiler",
                ..Options::default()
            },
        );
        assert_eq!(outcome, Outcome::Pass);
        assert!(!roots.local.exists());
    }

    #[test]
    fn no_content_without_a_reference_passes_and_with_one_fails() {
        let (_dir, roots) = roots();
        let options = Options {
            subfolder: "compiler",
            ..Options::default()
        };
        assert_eq!(run(&roots, b"a.js", NO_CONTENT, options), Outcome::Pass);
        std::fs::write(roots.reference.join("compiler/a.js"), b"x").unwrap();
        let outcome = run(&roots, b"a.js", NO_CONTENT, options);
        assert!(
            matches!(outcome, Outcome::Fail { ref reason, .. } if reason.contains("no longer produced"))
        );
        assert!(roots.local.join("compiler/a.js.delete").exists());
    }

    #[test]
    fn a_pass_removes_the_local_output_and_marker_of_an_earlier_failure() {
        let (_dir, roots) = roots();
        let options = Options {
            subfolder: "compiler",
            ..Options::default()
        };
        std::fs::write(roots.reference.join("compiler/a.js"), b"x").unwrap();
        assert!(matches!(
            run(&roots, b"a.js", b"y", options),
            Outcome::Fail { .. }
        ));
        assert!(roots.local.join("compiler/a.js").exists());
        assert_eq!(run(&roots, b"a.js", b"x", options), Outcome::Pass);
        assert!(!roots.local.join("compiler/a.js").exists());
        std::fs::remove_file(roots.reference.join("compiler/a.js")).unwrap();
        std::fs::write(roots.reference.join("compiler/b.js"), b"x").unwrap();
        assert!(matches!(
            run(&roots, b"b.js", NO_CONTENT, options),
            Outcome::Fail { .. }
        ));
        assert!(roots.local.join("compiler/b.js.delete").exists());
        assert_eq!(run(&roots, b"b.js", b"x", options), Outcome::Pass);
        assert!(!roots.local.join("compiler/b.js.delete").exists());
    }

    #[test]
    fn changed_and_new_baselines_fail_and_land_under_local() {
        let (_dir, roots) = roots();
        let options = Options {
            subfolder: "compiler",
            ..Options::default()
        };
        std::fs::write(roots.reference.join("compiler/a.js"), b"one\ntwo\n").unwrap();
        match run(&roots, b"a.js", b"one\nthree\n", options) {
            Outcome::Fail { reason, detail } => {
                assert!(reason.contains("has changed"), "{reason}");
                let detail = detail.unwrap();
                assert!(
                    detail.contains("-two") && detail.contains("+three"),
                    "{detail}"
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            std::fs::read(roots.local.join("compiler/a.js")).unwrap(),
            b"one\nthree\n"
        );
        match run(&roots, b"b.js", b"new", options) {
            Outcome::Fail { reason, .. } => {
                assert!(reason.contains("new baseline created"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            run(&roots, b"c.js", b"", options),
            Outcome::Fail { .. }
        ));
    }

    mod tempdir {
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicUsize, Ordering};

        static NEXT: AtomicUsize = AtomicUsize::new(0);

        pub struct Dir {
            pub path: PathBuf,
        }

        impl Dir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "tsr-testrunner-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&path).unwrap();
                Self { path }
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }
}
