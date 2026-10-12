"""External acceptance consumers cannot import implementation crates directly.

Compiler/checker dependencies behind tsr_embed are expected. This guard checks
the independently declared dependencies and every consumer Rust source,
including integration tests and shared modules, without building Cargo targets.
Public facade aliases retain their implementation types' methods; this is a
dependency/entry-point audit, not proof of a smaller capability surface or an
independently stable facade API.
"""
from __future__ import annotations

from pathlib import Path
import re
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
BANNED = {"tsr_compiler", "tsr_checker", "tsr_transpile", "tsr_testrunner", "s10_corpus"}
DEPENDENCY_SECTIONS = {"dependencies", "dev-dependencies", "build-dependencies"}
BANNED_PATH = re.compile(r"\b(?:" + "|".join(sorted(BANNED)) + r")\s*(?:::|;|\bas\b)")
PATH_MODULE = re.compile(r'#\s*\[\s*path\s*=\s*"([^"\n]+)"\s*\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*;')
MODULE = re.compile(r'(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;')
INCLUDE = re.compile(r'\binclude!\s*\(\s*"([^"\n]+)"\s*\)')


def dependency_errors(manifest):
    document = tomllib.loads(manifest.read_text())
    errors = []
    def visit(table, location):
        for key, value in table.items():
            if key in DEPENDENCY_SECTIONS:
                for alias, dependency in value.items():
                    package = dependency.get("package", alias) if isinstance(dependency, dict) else alias
                    if alias in BANNED or package in BANNED:
                        errors.append(f"{manifest}:{location}{key}: forbidden direct dependency {alias} ({package})")
            elif isinstance(value, dict):
                visit(value, location + key + ".")
    visit(document, "")
    return errors


def source_errors(consumer):
    pending = [path for directory in ("src", "tests", "examples", "benches")
               for path in (consumer / directory).rglob("*.rs")]
    visited = set()
    errors = []
    while pending:
        path = pending.pop().resolve()
        if path in visited:
            continue
        visited.add(path)
        if not path.is_file():
            errors.append(f"missing included consumer source: {path}")
            continue
        source = path.read_text()
        for match in BANNED_PATH.finditer(source):
            line = source.count("\n", 0, match.start()) + 1
            errors.append(f"{path}:{line}: forbidden direct compiler crate path {match.group().strip()}")
        paths = list(PATH_MODULE.finditer(source))
        for included in [*(match.group(1) for match in paths), *INCLUDE.findall(source)]:
            pending.append(path.parent / included)
        # Shared modules outside the consumer directory have ordinary child
        # modules too. Follow them instead of checking only the #[path] root.
        module_directory = path.parent if path.name in {"main.rs", "lib.rs", "mod.rs"} else path.with_suffix("")
        for match in MODULE.finditer(source):
            if any(mapped.start() <= match.start(1) < mapped.end() for mapped in paths):
                continue
            name = match.group(1)
            candidates = [module_directory / (name + ".rs"), module_directory / name / "mod.rs"]
            found = [candidate for candidate in candidates if candidate.is_file()]
            if len(found) != 1:
                errors.append(f"cannot resolve included consumer module: {path}:{name}")
            else:
                pending.append(found[0])
    return errors, visited


class ConsumerBoundaryTests(unittest.TestCase):
    def test_budget_consumer_uses_public_session_without_acceptance_helpers(self):
        manifest = ROOT / "tools/phase7/budget/rust/Cargo.toml"
        self.assertEqual(dependency_errors(manifest), [])
        errors, visited = source_errors(manifest.parent)
        self.assertEqual(errors, [])
        for name in ("main.rs", "engine.rs", "protocol.rs", "allocator.rs"):
            self.assertIn(manifest.parent / "src" / name, visited)
        # Its protocol and observations are independent of the S10 acceptance
        # schema/walkers. Following an include must not quietly import them.
        self.assertTrue(all(path.is_relative_to(manifest.parent) for path in visited))
        document = tomllib.loads(manifest.read_text())
        for alias, dependency in document["dependencies"].items():
            if isinstance(dependency, dict) and "path" in dependency:
                package_root = (manifest.parent / dependency["path"]).resolve()
                self.assertTrue(package_root.is_relative_to(ROOT / "crates"), alias)

    def test_wasm_compiler_uses_the_same_public_executor_and_walker(self):
        manifest = ROOT / "tools/phase7/portable/wasm-compiler/Cargo.toml"
        self.assertEqual(dependency_errors(manifest), [])
        errors, visited = source_errors(manifest.parent)
        self.assertEqual(errors, [])
        for shared in ("compiler_consumer.rs", "compiler_protocol.rs", "compiler_codec.rs", "mapper_codec.rs",
                       "assertions.rs", "walker/mod.rs", "walker/query.rs", "walker/rows.rs"):
            self.assertIn(ROOT / "tools/phase7/portable" / shared, visited)

    def test_external_consumer_declares_only_public_api_and_host_value_dependencies(self):
        manifest = ROOT / "tools/s10/rust-consumer/Cargo.toml"
        self.assertEqual(dependency_errors(manifest), [])
        errors, visited = source_errors(manifest.parent)
        self.assertEqual(errors, [])
        self.assertIn(ROOT / "tools/s10/rust-consumer/tests/lifetime.rs", visited)
        self.assertIn(ROOT / "tools/phase7/portable/consumer.rs", visited)
        self.assertIn(ROOT / "tools/phase7/portable/protocol.rs", visited)
        for shared in ("compiler_consumer.rs", "compiler_protocol.rs", "compiler_codec.rs", "mapper_codec.rs",
                       "assertions.rs", "walker/mod.rs", "walker/query.rs", "walker/rows.rs"):
            self.assertIn(ROOT / "tools/phase7/portable" / shared, visited)

    def test_dependency_guard_catches_renamed_dev_build_and_target_dependencies(self):
        with tempfile.TemporaryDirectory() as temporary:
            manifest = Path(temporary) / "Cargo.toml"
            for section in ("dependencies", "dev-dependencies", "build-dependencies",
                            "target.'cfg(unix)'.dev-dependencies"):
                for crate in BANNED:
                    with self.subTest(section=section, crate=crate):
                        manifest.write_text(f'[{section}]\nprivate_alias = {{ package = "{crate}", version = "0.4.0" }}\n')
                        self.assertEqual(len(dependency_errors(manifest)), 1)

    def test_source_guard_checks_tests_and_shared_include_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            consumer = Path(temporary) / "consumer"
            (consumer / "src").mkdir(parents=True)
            (consumer / "tests").mkdir()
            shared = Path(temporary) / "shared.rs"
            (consumer / "src/main.rs").write_text('#[path = "../../shared.rs"]\nmod shared;\n')
            shared.write_text("use tsr_checker as internal;\n")
            test = consumer / "tests/lifetime.rs"
            test.write_text("use tsr_compiler::Program;\n")
            errors, visited = source_errors(consumer)
            self.assertEqual(len(errors), 2)
            self.assertIn(shared.resolve(), visited)
            self.assertTrue(any("lifetime.rs" in error for error in errors))
            shared.write_text('include!("included.rs");\n')
            (Path(temporary) / "included.rs").write_text("fn fallback() { tsr_transpile::transpile_module(); }\n")
            test.write_text("use tsr_embed::Session;\n")
            errors, _ = source_errors(consumer)
            self.assertEqual(len(errors), 1)
            self.assertIn("included.rs", errors[0])

    def test_shared_module_children_are_checked(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            consumer = root / "consumer"
            (consumer / "src").mkdir(parents=True)
            shared = root / "shared"
            shared.mkdir()
            (consumer / "src/main.rs").write_text('#[path = "../../shared/mod.rs"]\nmod shared;\n')
            (shared / "mod.rs").write_text("pub mod child;\n")
            (shared / "child.rs").write_text("use tsr_checker::Operation;\n")
            errors, visited = source_errors(consumer)
            self.assertEqual(len(errors), 1)
            self.assertIn((shared / "child.rs").resolve(), visited)
            self.assertIn("forbidden direct compiler", errors[0])


if __name__ == "__main__":
    unittest.main()
