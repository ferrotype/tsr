#!/usr/bin/env python3
"""Phase 4 X0: compare a Rust command-line run with the committed references.

The result of a scenario is its whole rendered baseline against the committed
reference `testdata/baselines/reference/<id>`, read from the pinned commit
(`git cat-file` at data/upstream.json's pin, never the working tree). Every
scenario of the inventory lands in exactly one category:

  match        a completed row whose transcript equals the reference byte for byte
  different    a completed row whose transcript differs
  failed       a production panic (located under crates/), or a harness defect
               (a harness failure or a panic elsewhere), which also makes the
               report invalid
  unsupported  a named refusal of the Rust command line (the operation)
  unexecuted   the run did not select the scenario (a partial run)

The orphan reference, which no scenario of the pin renders, is listed apart and
is never counted as a scenario.

ATTRIBUTION. For a `different` row both texts are split into the runner's
steps and each step into its sections (tsctests/runner.go `tscInput.run`,
sys.go `serializeState` and `baselinePrograms`, fsbaselineutil
`BaselineFSwithDiff`). The split is driven by the recorded scenario (its edit
captions, command lines and family) and is lossless: the sections concatenate
to the text. Steps are `initial` and `edit N` (the runner's `Edit [N]`):

  input        step 0: "currentDirectory::", "useCaseSensitiveFileNames::",
               "Input::" and the initial files ("//// [path] label" entries)
  edit         step N: the "\\n\\nEdit [N]:: caption" line and the files the edit
               changed
  command      "tsgo <arguments>" and the "ExitStatus:: <status>" line (no
               command for a watch cycle)
  output       "\\nOutput::\\n" and the sanitized console output
  files        the file-system difference after the command, by path and label
               (*new*, *modified*, *deleted*, *rewrite with same content*,
               *mTime changed*, *Lib*, -> target *new*), build information aside
  buildinfo    the `.tsbuildinfo` and `.tsbuildinfo.readable.baseline.txt`
               entries of that difference
  watch        "Watch Registrations::" and the directory watches
  program      each incremental program's "<config>::", "SemanticDiagnostics::"
               and "Signatures::" blocks, and an include-reason failure block
  incremental  the "\\n\\nDiff:: " block: the incremental build against a clean
               build of the same state
  unparsed     what follows a marker the split could not find (a truncated or
               malformed transcript)

A `different` row names its first differing step and section in transcript
order (a missing step is section `step`), for file sections the first
differing path with both labels, for text sections the first differing line,
and every section that differs anywhere in the row. An edit step agrees
incrementally when its `incremental` section equals the reference's (absent on
both sides, or the same explained difference).

    report   --rust DIR [--output FILE] [--record]
    mutation --output DIR [--scenario ID ...]

`report` writes the comparison to DIR/comparison.json; `--record` writes the
acceptance summary (the report without its rows) to
data/phase4/first-comparison.json, only for a full, harness-valid run of the
current sources. `mutation` checks the comparison itself: it builds synthetic
completed rows from the committed references (untouched; one changed byte;
the last edit step dropped; two steps' outputs swapped; two scenarios'
transcripts swapped) as captures under DIR, reports each one, and fails unless
every untouched row matches and every mutated row reads `different`.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase4_corpus as corpus  # noqa: E402

RECORD = ROOT / "data/phase4/first-comparison.json"
APPROVED = ROOT / "data/phase4/approved-differences.json"
REFERENCES = "tsc/testdata/baselines/reference"
CATEGORIES = ("match", "different", "failed", "unsupported", "unexecuted")
SECTIONS = ("input", "edit", "command", "output", "files", "buildinfo", "watch", "program", "incremental",
            "unparsed", "step")
EXAMPLES = 3
OUTPUT = b"\nOutput::\n"
DIFF = b"\n\nDiff:: "
INCLUDE_REASONS = b"\n!!! Include reasons expectations don't match pls review!!!\n"
SEMANTIC = b"\nSemanticDiagnostics::\n"
WATCH = b"\nWatch Registrations::\n"
NOT_A_CONFIG = (b"Directory watches::", b"Watch Registrations::", b"Output::")
# fsbaselineutil's entry: "//// [" path "] " diff "\n", the diff starting with
# its label; a text-carrying label ends its line and the text follows.
ENTRY = re.compile(rb"//// \[([^\n]*?)\] (\*new\* \n|\*modified\* \n|\*Lib\*\n|\*deleted\*\n"
                   rb"|\*rewrite with same content\*\n|\*mTime changed\*\n|-> [^\n]* \*new\*\n)")
BUILD_INFO = (".tsbuildinfo", ".tsbuildinfo.readable.baseline.txt")
EMITTED = (".js", ".mjs", ".cjs", ".jsx", ".d.ts", ".d.mts", ".d.cts", ".map")


# --- the committed references ----------------------------------------------------

def pin():
    return strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]


def read_references(ids, commit=None):
    """{id: (bytes, git blob id)} of the committed references at the pin."""
    commit = commit or pin()
    request = b"".join(f"{commit}:{REFERENCES}/{identity}\n".encode() for identity in ids)
    completed = subprocess.run(["git", "-C", str(ROOT / "upstream"), "cat-file", "--batch"], input=request,
                               capture_output=True, check=False)
    if completed.returncode:
        raise ValueError("git cat-file failed: " + completed.stderr.decode(errors="replace")[-500:])
    data, position, result = completed.stdout, 0, {}
    for identity in ids:
        end = data.index(b"\n", position)
        header = data[position:end].split()
        if len(header) != 3 or header[1] != b"blob":
            raise ValueError("the pin has no committed reference " + identity)
        size = int(header[2])
        result[identity] = (data[end + 1:end + 1 + size], header[0].decode())
        position = end + 1 + size + 1
    return result


# --- the split ----------------------------------------------------------------------

def command_line(arguments):
    return b"tsgo " + b" ".join(argument.encode() for argument in (arguments or [])) + b"\n"


def entry_starts(text, start=0):
    """File-system entries at line starts from `start`."""
    return [match for match in ENTRY.finditer(text, start)
            if match.start() == 0 or text[match.start() - 1:match.start()] == b"\n"]


def step_markers(text, edits):
    """Where each step starts: 0, then each edit's "\\n\\nEdit [N]:: caption\\n",
    found in order; a marker not found ends the list."""
    starts, position = [0], 0
    for index, edit in enumerate(edits):
        marker = f"\n\nEdit [{index}]:: {edit['caption']}\n".encode()
        found = text.find(marker, position)
        if found < 0:
            break
        starts.append(found)
        position = found + len(marker)
    return starts


def split_tail(tail, header):
    """The parts after a step's command: output, the file-system difference,
    the watch state, the program data and the incremental difference."""
    parts = []
    incremental = b""
    found = tail.find(DIFF, len(OUTPUT))
    if found >= 0:
        tail, incremental = tail[:found], tail[found:]
    include = b""
    found = tail.find(b"\n\n" + header + INCLUDE_REASONS, len(OUTPUT))
    if found >= 0:
        tail, include = tail[:found], tail[found:]
    program = b""
    found = tail.find(SEMANTIC, len(OUTPUT) - 1)
    if found >= 0:
        line = tail.rfind(b"\n", 0, found) + 1
        previous = tail[line:found]
        start = (line if previous.endswith(b"::") and previous not in NOT_A_CONFIG
                 and not previous.startswith(b"//// [") else found + 1)
        tail, program = tail[:start], tail[start:]
    program += include
    watch = b""
    found = tail.find(WATCH, len(OUTPUT) - 1)
    if found >= 0:
        tail, watch = tail[:found + 1], tail[found + 1:]
    entries = entry_starts(tail, len(OUTPUT))
    if entries:
        output, fs = tail[:entries[0].start()], tail[entries[0].start():]
    elif len(tail) > len(OUTPUT) and tail.endswith(b"\n"):
        output, fs = tail[:-1], tail[-1:]
    else:
        output, fs = tail, b""
    parts += [("output", output), ("fs", fs)]
    parts += [(name, value) for name, value in (("watch", watch), ("program", program),
                                                ("incremental", incremental)) if value]
    return parts


def split(text, scenario):
    """[(step name, [(part, bytes)])], lossless: the parts concatenate to `text`."""
    edits = scenario["edits"]
    watch = scenario["family"].endswith("Watch")
    starts = step_markers(text, edits)
    steps = []
    for index, start in enumerate(starts):
        step = text[start:starts[index + 1] if index + 1 < len(starts) else len(text)]
        parts = []
        if index == 0:
            name, header = "initial", b"Initial build"
            found = step.find(b"\n" + command_line(scenario["command_line_args"]))
            if found < 0:
                found = step.find(b"\ntsgo ")
            if found < 0:
                steps.append((name, [("input", step)]))
                continue
            parts.append(("input", step[:found + 1]))
            command = found + 1
        else:
            edit = edits[index - 1]
            name = f"edit {index - 1}"
            header = f"Edit [{index - 1}]:: {edit['caption']}\n".encode()
            after = len(header) + 1
            arguments = edit["command_line_args"] if edit["command_line_args"] is not None \
                else scenario["command_line_args"]
            with_command = step.find(b"\n" + command_line(arguments), after)
            cycle = step.find(b"\n" + OUTPUT, after)
            generic = step.find(b"\ntsgo ", after)
            preferred = (cycle,) if watch else (with_command,)
            fallback = (with_command, generic) if watch else (cycle, generic)
            candidates = [value for value in preferred if value >= 0] or [value for value in fallback if value >= 0]
            if not candidates:
                steps.append((name, [("edit", step)]))
                continue
            found = min(candidates)
            parts.append(("edit", step[:found + 1]))
            command = found + 1 if step[found + 1:found + 6] == b"tsgo " else None
        if command is None:
            output = found + 1
        else:
            output = step.find(OUTPUT, command)
            if output < 0:
                parts.append(("command", step[command:]))
                steps.append((name, parts))
                continue
            parts.append(("command", step[command:output]))
        parts += split_tail(step[output:], header)
        steps.append((name, parts))
    return steps


def entries(region):
    """[(path, label, bytes)] of a file-system region: the text before the first
    entry (the step's header lines) as path "" and label "header", then one
    item per entry; the region's terminating empty line is not an entry."""
    body = region[:-1] if region.endswith(b"\n\n") or region == b"\n" else region
    found = entry_starts(body)
    result = []
    head = body[:found[0].start()] if found else body
    if head:
        result.append(("", "header", head))
    for index, match in enumerate(found):
        end = found[index + 1].start() if index + 1 < len(found) else len(body)
        label = match.group(2).decode(errors="replace").rstrip("\n").rstrip()
        result.append((match.group(1).decode(errors="replace"), "-> *new*" if label.startswith("-> ") else label,
                       body[match.start():end]))
    return result


def is_build_info(path):
    return path.endswith(BUILD_INFO)


# --- comparison ---------------------------------------------------------------------

def first_line(reference, rust):
    """The first line where two texts differ: (1-based number, reference line, Rust line)."""
    left, right = reference.split(b"\n"), rust.split(b"\n")
    for number, (a, b) in enumerate(zip(left, right), 1):
        if a != b:
            return number, a, b
    number = min(len(left), len(right)) + 1
    return number, (left[number - 1] if number <= len(left) else None), (right[number - 1] if number <= len(right)
                                                                           else None)


def shown(line):
    return None if line is None else line[:160].decode("utf-8", "replace")


def entry_difference(reference, rust):
    """The first path (in transcript order) whose entry differs: present on one
    side only, another label, or other text."""
    left = {path: (label, text) for path, label, text in entries(reference)}
    right = {path: (label, text) for path, label, text in entries(rust)}
    order = [path for path, _, _ in entries(reference)]
    order += [path for path, _, _ in entries(rust) if path not in left]
    for path in sorted(order, key=lambda value: (value != "", value)):
        if left.get(path) != right.get(path):
            detail = {"path": path or None, "reference_label": left.get(path, (None,))[0],
                      "rust_label": right.get(path, (None,))[0]}
            if path in left and path in right and left[path][0] == right[path][0]:
                number, a, b = first_line(left[path][1], right[path][1])
                detail.update(difference="text", line=number, reference_line=shown(a), rust_line=shown(b))
            else:
                detail["difference"] = "missing" if path not in right else "extra" if path not in left else "label"
            return detail
    if [path for path, _, _ in entries(reference)] != [path for path, _, _ in entries(rust)]:
        return {"path": None, "difference": "order"}
    return {"path": None, "difference": "terminator"}


def fs_section(path):
    return "buildinfo" if is_build_info(path or "") else "files"


def part_difference(step, part, reference, rust):
    """The attribution of one differing part of a step."""
    base = {"step": step}
    if part in ("input", "edit"):
        return dict(base, section=part, **entry_difference(reference, rust))
    if part == "fs":
        detail = entry_difference(reference, rust)
        return dict(base, section=fs_section(detail.get("path")), **detail)
    number, a, b = first_line(reference, rust)
    return dict(base, section=part, difference="text", line=number, reference_line=shown(a), rust_line=shown(b))


def compare_steps(reference_steps, rust_steps):
    """(first difference, every differing section, edit steps agreeing incrementally)."""
    first, sections, agreeing = None, set(), 0
    for index in range(max(len(reference_steps), len(rust_steps))):
        if index >= len(rust_steps) or index >= len(reference_steps):
            name = (reference_steps if index < len(reference_steps) else rust_steps)[index][0]
            difference = {"step": name, "section": "step",
                          "difference": "missing" if index >= len(rust_steps) else "extra"}
            first = first or difference
            sections.add("step")
            continue
        name, left = reference_steps[index]
        _, right = rust_steps[index]
        left_parts, right_parts = dict(left), dict(right)
        if index and left_parts.get("incremental", b"") == right_parts.get("incremental", b""):
            agreeing += 1
        # The parts come in one fixed order; a part on one side only differs.
        order = [part for part, _ in left] + [part for part, _ in right if part not in left_parts]
        for part in order:
            a, b = left_parts.get(part, b""), right_parts.get(part, b"")
            if part in left_parts and part in right_parts and a == b:
                continue
            difference = part_difference(name, part, a, b)
            sections.add(difference["section"])
            if part == "fs":
                old = {path: (label, text) for path, label, text in entries(a)}
                new = {path: (label, text) for path, label, text in entries(b)}
                sections.update(fs_section(path) for path in set(old) | set(new) if old.get(path) != new.get(path))
            first = first or difference
    return first, sorted(sections, key=SECTIONS.index), agreeing


def emitted_file(difference):
    """A content difference inside an emitted output file (routed to Phase 3,
    plan section 7)."""
    return (difference.get("section") == "files" and difference.get("difference") == "text"
            and (difference.get("path") or "").endswith(EMITTED))


def outcome(category, **detail):
    if category not in CATEGORIES:
        raise ValueError("unknown category " + category)
    return dict(detail, category=category)


def approved_difference(identity, reference, transcript, ledger):
    """Exact observed pairs only. Raw comparison results are never rewritten."""
    if ledger.get("version") != 1 or ledger.get("pin") != pin():
        raise ValueError("Phase 4 difference approvals have another version or pin")
    matches = [entry["id"] for entry in ledger["exceptions"] if entry.get("approved") is True
               and entry.get("approval") and entry.get("reason")
               for row in entry["observations"] if row["scenario"] == identity
               and row["native_sha256"] == digest(reference) and row["rust_sha256"] == digest(transcript)]
    if len(matches) > 1:
        raise ValueError("a Phase 4 observation has duplicate approvals")
    return matches[0] if matches else None


def compare_row(scenario, reference, row, transcript):
    """One scenario's outcome; never blank."""
    edits = len(scenario["edits"])
    if row["state"] in ("unsupported", "failed"):
        stopped = {"stopped_at": row["progress"]["stage"], "prefix_of_reference": reference.startswith(transcript)}
        if row["state"] == "unsupported":
            return outcome("unsupported", operation=row["operation"], **stopped)
        reason = f"{row['class']}: {row['reason']}"[:300]
        if row["location"]:
            reason += " at " + row["location"]
        return outcome("failed", reason=reason, **stopped)
    if transcript == reference:
        return outcome("match", incremental_agreeing=edits)
    first, sections, agreeing = compare_steps(split(reference, scenario), split(transcript, scenario))
    if first is None:
        raise ValueError("a different transcript without a differing section: " + scenario["id"])
    return outcome("different", first=first, sections=sections, incremental_agreeing=agreeing,
                   unexpected_diff=row["unexpected_diff"] is not None, emitted_file=emitted_file(first))


def cause_of(result):
    """The bucket of a row that is not a match."""
    category = result["category"]
    if category == "unsupported":
        return result["operation"]
    if category in ("failed", "unexecuted"):
        return result["reason"]
    if result["emitted_file"]:
        return "an emitted file differs"
    kind = "initial" if result["first"]["step"] == "initial" else "edit"
    return f"{result['first']['section']} differs ({kind} step)"


def finish(table):
    return [dict(cause=key, rows=len(value["rows"]), families=dict(sorted(value["families"].items())),
                 examples=value["examples"])
            for key, value in sorted(table.items(), key=lambda item: (-len(item[1]["rows"]), item[0]))]


def report(rust_dir=corpus.DEFAULT_OUTPUT, *, capture=None):
    """The categorized comparison of one capture (`phase4_corpus.load_capture`)."""
    capture = capture or corpus.load_capture(rust_dir)
    document = capture.document
    identities = [item["id"] for item in document["scenarios"]]
    orphans = list(document["orphan_references"])
    references = read_references(identities + orphans, document["provenance"]["pin"])
    approvals = strict_json_loads(APPROVED.read_bytes())
    rows = {row["id"]: row for row in capture.rows}
    results, harness = [], []
    buckets = defaultdict(lambda: defaultdict(lambda: {"rows": [], "families": Counter(), "examples": []}))
    for scenario in document["scenarios"]:
        identity = scenario["id"]
        reference = references[identity][0]
        row = rows.get(identity)
        if row is None:
            result = outcome("unexecuted", reason="not selected by the run")
        else:
            problem = corpus.harness_problem(row)
            if not problem and row["state"] == "completed" and row["unexpected_diff"] is not None \
                    and capture.transcripts[identity] == reference:
                # The pin reports an unexpected difference only with a block
                # the baseline shows, and no reference has one.
                problem = "an unexpected difference with a transcript equal to the reference"
            if problem:
                harness.append({"id": identity, "problem": problem})
                result = outcome("failed", reason="harness: " + problem)
            else:
                result = compare_row(scenario, reference, row, capture.transcripts[identity])
                if result["category"] == "different":
                    approval = approved_difference(identity, reference, capture.transcripts[identity], approvals)
                    if approval:
                        result["approved_difference"] = approval
        if result["category"] != "match":
            bucket = buckets[result["category"]][cause_of(result)]
            bucket["rows"].append(identity)
            bucket["families"][scenario["family"]] += 1
            if len(bucket["examples"]) < EXAMPLES:
                bucket["examples"].append(identity)
        results.append(dict(result, id=identity, family=scenario["family"],
                            edits=len(scenario["edits"])))
    categories = Counter(result["category"] for result in results)
    families = {family: {category: sum(1 for result in results
                                       if result["family"] == family and result["category"] == category)
                         for category in CATEGORIES} for family in corpus.FAMILIES}
    stopped = [result for result in results if "prefix_of_reference" in result]
    different_sections = Counter(section for result in results for section in result.get("sections", ()))
    summary = {
        "rows": len(results), "selected": len(capture.rows), "partial": capture.metadata["partial"],
        "valid": not harness, "harness_errors": len(harness),
        "categories": {category: categories[category] for category in CATEGORIES},
        "families": families,
        "matched": categories["match"],
        "approved_differences": sum("approved_difference" in result for result in results),
        "accepted": sum(result["category"] == "match" or "approved_difference" in result for result in results),
        "unsupported_rows": categories["unsupported"],
        "edit_steps": sum(result["edits"] for result in results),
        "edit_steps_agreeing": sum(result.get("incremental_agreeing", 0) for result in results),
        "stopped_rows": len(stopped),
        "stopped_rows_prefix_of_reference": sum(result["prefix_of_reference"] for result in stopped),
        "different_sections": {section: different_sections[section] for section in SECTIONS
                               if different_sections[section]},
    }
    if sum(summary["categories"].values()) != len(identities):
        raise ValueError("a scenario is not in exactly one category")
    return {
        "version": 1, "pin": document["provenance"]["pin"], "summary": summary,
        "approvals_sha256": digest(APPROVED.read_bytes()),
        "inventory": capture.metadata["inventory"],
        "references": {"scenarios": len(identities),
                       "git_blobs_sha256": digest(canonical({identity: references[identity][1]
                                                             for identity in identities}))},
        "rust": {"capture_sha256": capture.sha256, "executable_sha256": capture.metadata["artifacts"]["executable"],
                 "selectors": capture.metadata["selectors"], "partial": capture.metadata["partial"],
                 "source_stable": corpus.sources() == capture.record["sources"]},
        "orphan_references": [{"reference": identity, "git_blob": references[identity][1],
                               "reason": "no scenario of the pinned suite renders it; it is not a scenario"}
                              for identity in orphans],
        "buckets": {category: finish(buckets[category]) for category in CATEGORIES if category != "match"},
        "harness_errors": harness,
        "rows": results,
    }


def acceptance_summary(comparison):
    """The recorded part of a comparison: everything but the per-row report."""
    return {key: value for key, value in comparison.items() if key != "rows"}


def record(comparison, path=None):
    """Write the acceptance summary of a full, harness-valid run of the current
    sources to `path` (default RECORD)."""
    path = RECORD if path is None else path
    if comparison["summary"]["partial"]:
        raise ValueError("a partial Rust run is informational and cannot be recorded")
    if not comparison["summary"]["valid"]:
        raise ValueError("a run with harness errors cannot be recorded")
    if not comparison["rust"]["source_stable"]:
        raise ValueError("record requires a Rust capture of the current sources")
    Path(path).write_bytes(json.dumps(acceptance_summary(comparison), indent=1, sort_keys=True).encode() + b"\n")


# --- the mutation check ----------------------------------------------------------------

def mutable_column(line):
    """Where a letter of `line` may change without touching a marker the split
    reads: a command's arguments, an exit status's name, any line without
    "::" that is not a file-system entry's header; None elsewhere."""
    for prefix in (b"tsgo ", b"ExitStatus:: "):
        if line.startswith(prefix):
            return len(prefix)
    return None if b"::" in line or line.startswith(b"//// [") else 0


def changed_byte(text, scenario, index=0):
    """One ASCII lowercase letter made uppercase, nearest the middle of one
    part of the transcript (the `index`-th part that has such a letter, so the
    parts mutated vary across scenarios); returns (text, (step, section))."""
    candidates, offset = [], 0
    for name, parts in split(text, scenario):
        for part, value in parts:
            position, middle, best = 0, len(value) // 2, None
            for line in value.split(b"\n"):
                start = mutable_column(line)
                if start is not None:
                    column = next((column for column in range(start, len(line)) if 97 <= line[column] <= 122), None)
                    if column is not None and (best is None or abs(position + column - middle) < abs(best - middle)):
                        best = position + column
                position += len(line) + 1
            if best is not None:
                candidates.append((offset, name, part, value, best))
            offset += len(value)
    if not candidates:
        return None
    start, name, part, value, best = candidates[index % len(candidates)]
    at = start + best
    mutated = text[:at] + bytes([text[at] - 32]) + text[at + 1:]
    section = part
    if part == "fs":
        # The entries are contiguous from the region's start.
        position, owner = 0, ""
        for path, _, item in entries(value):
            if position <= best < position + len(item):
                owner = path
            position += len(item)
        section = fs_section(owner)
    return mutated, (name, section)


def dropped_edit(text, scenario):
    """The transcript without its last edit step."""
    starts = step_markers(text, scenario["edits"])
    if len(starts) < 2:
        return None
    return text[:starts[-1]], (f"edit {len(starts) - 2}", "step")


def swapped_outputs(text, scenario):
    """The first two steps with different outputs, their outputs swapped."""
    steps = split(text, scenario)
    located, offset = [], 0
    for name, parts in steps:
        for part, value in parts:
            if part == "output":
                located.append((offset, value, name))
            offset += len(value)
    for index, (start, value, name) in enumerate(located):
        for other_start, other, _ in located[index + 1:]:
            if other != value:
                mutated = (text[:start] + other + text[start + len(value):other_start] + value
                           + text[other_start + len(other):])
                return mutated, (name, "output")
    return None


MUTATIONS = {"changed_byte": changed_byte, "dropped_edit": dropped_edit, "swapped_outputs": swapped_outputs}


def mutation(output, scenario_ids=None):
    """Synthetic captures of completed rows built from the committed references,
    each reported through `report`: the untouched references, each mutation of
    MUTATIONS, and pairs of scenarios with their transcripts swapped."""
    document = corpus.read_document()
    chosen = scenario_ids or [item["id"] for item in document["scenarios"]]
    selectors = ["all"] if not scenario_ids else list(scenario_ids)
    ids = corpus.selection(document, selectors)
    by_id = {item["id"]: item for item in document["scenarios"]}
    references = {identity: value[0] for identity, value in read_references(ids, document["provenance"]["pin"]).items()}
    output = Path(output).resolve()
    variants = {"untouched": ({identity: references[identity] for identity in ids}, {})}
    for name, function in MUTATIONS.items():
        texts, expected = {}, {}
        for number, identity in enumerate(ids):
            arguments = (number,) if function is changed_byte else ()
            mutated = function(references[identity], by_id[identity], *arguments)
            if mutated is None:
                texts[identity] = references[identity]
            else:
                texts[identity], expected[identity] = mutated
        variants[name] = (texts, expected)
    texts, expected = dict(references), {}
    families = defaultdict(list)
    for identity in ids:
        families[by_id[identity]["family"]].append(identity)
    for members in families.values():
        pending = list(members)
        while len(pending) >= 2:
            first = pending.pop(0)
            partner = next((other for other in pending if references[other] != references[first]), None)
            if partner is None:
                continue
            pending.remove(partner)
            texts[first], texts[partner] = references[partner], references[first]
            expected[first] = expected[partner] = (None, None)
    variants["swapped_rows"] = (texts, expected)
    results = {}
    for name, (texts, expected) in variants.items():
        directory = output / name
        corpus.write_capture(directory, document, selectors,
                             {identity: texts[identity] for identity in ids}, replace=True)
        comparison = report(directory)
        rows = {row["id"]: row for row in comparison["rows"] if row["id"] in texts}
        mutated = [identity for identity in ids if identity in expected]
        controls = [identity for identity in ids if identity not in expected]
        attributed = sum(1 for identity in mutated if expected[identity][0] is not None
                         and (rows[identity]["first"]["step"], rows[identity]["first"]["section"])
                         == expected[identity])
        results[name] = {
            "mutated": len(mutated),
            "mutated_different": sum(rows[identity]["category"] == "different" for identity in mutated),
            "controls": len(controls),
            "controls_matched": sum(rows[identity]["category"] == "match" for identity in controls),
            "attributed_to_the_mutated_section": attributed if name not in ("untouched", "swapped_rows") else None,
            "first_sections": dict(sorted(Counter(rows[identity]["first"]["section"] for identity in mutated
                                                  if rows[identity]["category"] == "different").items())),
            "unexpected": [identity for identity in mutated if rows[identity]["category"] != "different"][:10]
            + [identity for identity in controls if rows[identity]["category"] != "match"][:10],
        }
    passed = all(item["mutated_different"] == item["mutated"] and item["controls_matched"] == item["controls"]
                 for item in results.values()) and results["untouched"]["controls"] == len(ids) and all(
        results[name]["mutated"] > 0 for name in (*MUTATIONS, "swapped_rows"))
    summary = {"version": 1, "scenarios": len(ids), "variants": results, "passed": passed}
    (output / "mutation.json").write_bytes(json.dumps(summary, indent=1, sort_keys=True).encode() + b"\n")
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("report")
    sub.add_argument("--rust", type=Path, default=corpus.DEFAULT_OUTPUT)
    sub.add_argument("--output", type=Path, help="default: <rust>/comparison.json")
    sub.add_argument("--record", action="store_true", help="write data/phase4/first-comparison.json")
    sub = commands.add_parser("mutation")
    sub.add_argument("--output", type=Path, default=ROOT / "target/phase4/mutation")
    sub.add_argument("--scenario", action="append", default=[], help="scenario id; repeatable (default: all)")
    args = parser.parse_args()
    if args.command == "mutation":
        summary = mutation(args.output, args.scenario or None)
        print(json.dumps(summary, indent=1, sort_keys=True))
        if not summary["passed"]:
            raise SystemExit(1)
        return
    result = report(args.rust)
    target = args.output or args.rust / "comparison.json"
    target.write_bytes(json.dumps(result, indent=1, sort_keys=True).encode() + b"\n")
    if args.record:
        record(result)
    print(json.dumps({"summary": result["summary"],
                      "buckets": {category: [{key: item[key] for key in ("cause", "rows", "families")}
                                             for item in items] for category, items in result["buckets"].items()},
                      "orphan_references": [item["reference"] for item in result["orphan_references"]]},
                     sort_keys=True))
    if not result["summary"]["valid"]:
        raise SystemExit(1)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase4 compare failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
