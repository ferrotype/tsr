"""Access-only S06 preprocessing bridges over a fresh export of the Go pin."""

from contextlib import contextmanager
import errno
import io
import json
from pathlib import Path
import shutil
import tarfile
import tempfile
import time

from s04 import go_environment, verified_upstream
from s04_common import command, strict_json_loads

ROOT = Path(__file__).resolve().parents[1]
EXPORT_PATHS = (
    "tsc/go.mod", "tsc/go.sum", "tsc/internal",
    "tsc/testdata/tests/cases/compiler", "tsc/testdata/tests/cases/conformance",
)
# The test harness eagerly asks repo.RootPath for testdata. That package
# explicitly rejects trimmed paths; retain its path without changing its code.
GO_TEST_FLAGS = ("-trimpath", "-mod=readonly")


def _cleanup_export(temporary):
    # Finder can recreate .DS_Store while rmtree removes an exported subtree.
    # Retry only that cleanup race, never the export or its caller's body.
    for attempt in range(3):
        try:
            temporary.cleanup()
            return
        except OSError as error:
            if error.errno != errno.ENOTEMPTY or attempt == 2:
                raise
            time.sleep(0.05)


@contextmanager
def oracle_export():
    upstream = verified_upstream()
    pin = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    env = go_environment()
    destination = ROOT / "target/s06-oracle"
    destination.mkdir(parents=True, exist_ok=True)
    temporary = tempfile.TemporaryDirectory(prefix="export-", dir=destination)
    try:
        checkout = Path(temporary.name)
        archive = command(["git", "archive", pin, *EXPORT_PATHS], cwd=upstream)
        try:
            with tarfile.open(fileobj=io.BytesIO(archive)) as stream:
                stream.extractall(checkout, filter="data")
        except (tarfile.TarError, ValueError) as error:
            raise RuntimeError(f"cannot unpack pinned S06 sources: {error}") from error
        bridges = {
            "export_test.go": "internal/testrunner/s06_export_test.go",
            "export_boundaries_test.go": "internal/testrunner/s06_export_boundaries_test.go",
            "fixture_export_test.go": "internal/testrunner/s06_fixture_export_test.go",
            "metadata_bridge.go": "internal/compiler/s06_metadata_bridge.go",
        }
        for source, target in bridges.items():
            shutil.copyfile(ROOT / "scripts/s06_oracle" / source, checkout / "tsc" / target)
        yield checkout, env, pin
    finally:
        _cleanup_export(temporary)


def export_cases(paths, destination, *, extract_only=False):
    """A failed Go test never yields a usable partial export."""
    destination = Path(destination).resolve()
    with oracle_export() as (checkout, env, pin):
        request = checkout / "s06-input.json"
        request.write_text(json.dumps(paths), encoding="utf-8")
        output = checkout / "s06-output.ndjson"
        fixtures = checkout / "s06-fixtures.json"
        env.update(S06_INPUT=str(request), S06_OUTPUT=str(output),
                   S06_EXTRACT_ONLY="1" if extract_only else "0", S06_FIXTURE_OUTPUT=str(fixtures), S06_FAILURE_PROBE="")
        # A unique unmatched prefix retains this package's real source path and
        # prevents Go's trimmed cache key from reusing a deleted earlier export.
        repo_path = f"-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath={checkout}/s06-unmatched-prefix"
        command(["go", "test", *GO_TEST_FLAGS, repo_path, "./internal/testrunner", "-run",
                 "^TestS06", "-count=1", "-timeout=5m"], cwd=checkout / "tsc", env=env)
        # The disposable file becomes visible only after successful preprocessing.
        shutil.copyfile(output, destination)
        shutil.copyfile(fixtures, destination.with_name("native-fixtures.json"))
        verified_upstream()
        return pin


def export_fixtures(destination):
    with oracle_export() as (checkout, env, _):
        output = checkout / "s06-fixtures.json"
        env.update(S06_INPUT="", S06_FIXTURE_OUTPUT=str(output))
        repo_path = f"-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath={checkout}/s06-unmatched-prefix"
        command(["go", "test", *GO_TEST_FLAGS, repo_path, "./internal/testrunner", "-run",
                 "^TestS06FixtureExport$", "-count=1", "-timeout=5m"], cwd=checkout / "tsc", env=env)
        shutil.copyfile(output, destination)
        return strict_json_loads(output.read_bytes())


def build_oracle():
    """Compile the persistent access-only runtime from a fresh canonical export."""
    destination = ROOT / "target/s06-oracle/go-oracle"
    with oracle_export() as (checkout, env, _):
        package = checkout / "tsc/internal/s06oracle"
        package.mkdir()
        for name in ("json.go", "protocol.go", "behavior.go", "factory.go", "codec.go", "protocol_test.go"):
            shutil.copyfile(ROOT / "scripts/s06_oracle" / name, package / name)
        shutil.copyfile(ROOT / "scripts/s06_oracle/codec_bridge.go", checkout / "tsc/internal/ast/s06_codec_bridge.go")
        command(["go", "test", "-trimpath", "-mod=readonly", "./internal/s06oracle", "-count=1"],
                cwd=checkout / "tsc", env=env)
        command(["go", "build", "-trimpath", "-mod=readonly", "-o", str(destination),
                 "./internal/s06oracle"], cwd=checkout / "tsc", env=env)
        verified_upstream()
    return destination
