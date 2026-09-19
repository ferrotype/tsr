#!/usr/bin/env python3
"""Exact graph parity for the benchmark's unchanged native parse/bind worker pool."""
import argparse
import hashlib
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import sys
import time

from s04_common import strict_json_loads
from s06_protocol import canonical, exact_keys, hex_bytes, integer
from s06_process import Process
from s07_benchmark import (CACHE, ROOT, build_go, build_rust, provision_inputs,
                           source_fingerprint, cargo_configuration, sha)
from s07_binder import KINDS, GraphValidator, name_identity, first_difference, comparable

FIELDS="version kind workers index filename_hex path_hex script_kind jsx force source_bytes source_sha256 canonical_sha256 raw_sha256 records counts qualified_names diagnostics node_count symbol_count parse_diagnostics bind_diagnostics"
SCALARS=("files","loaded_bytes","nodes","symbols","parse_diagnostics","bind_diagnostics")


def check_record(value,index,workers,request):
    exact_keys(value,FIELDS.split(),"workload graph report")
    for field,want in (("version",1),("workers",workers),("index",index)):
        integer(value[field],0,2**63-1,field)
        if value[field]!=want:raise ValueError("duplicate, missing or reordered workload report")
    if value["kind"]!="bind_graph":raise ValueError("unknown workload report kind")
    for field in ("filename","path"):
        if hex_bytes(value[field+"_hex"],field)!=request[field].encode():raise ValueError("workload file identity drift")
    for field in ("script_kind","jsx","force"):
        if type(value[field]) is not type(request[field]) or value[field]!=request[field]:raise ValueError("workload parse option drift")
    for field in ("source_bytes","records","node_count","symbol_count","parse_diagnostics","bind_diagnostics"):
        integer(value[field],0,2**63-1,field)
    for field in ("source_sha256","canonical_sha256","raw_sha256"):
        if len(hex_bytes(value[field],field))!=32:raise ValueError("invalid graph digest")
    exact_keys(value["counts"],KINDS,"workload graph counts")
    for count in value["counts"].values():integer(count,0,2**63-1,"graph count")
    if value["records"]!=2+sum(value["counts"].values()):raise ValueError("incomplete workload graph record count")
    def ref(kind,index,nullable=True):
        integer(index,0,value["counts"][kind],"qualified name edge")
        if not nullable and index==0:raise ValueError("nil qualified identity")
    if type(value["qualified_names"]) is not list or type(value["diagnostics"]) is not list:raise ValueError("invalid workload name/diagnostic collection")
    validator=GraphValidator();validator.ref=ref
    indices=dict.fromkeys(("parse","js","jsdoc","bind"),0);previous=-1
    for diagnostic in value["diagnostics"]:
        if type(diagnostic) is not list or len(diagnostic)!=3 or diagnostic[0] not in indices:raise ValueError("invalid diagnostic grouping")
        order=list(indices).index(diagnostic[0])
        if order<previous or type(diagnostic[1]) is not int or diagnostic[1]!=indices[diagnostic[0]]:raise ValueError("reordered or duplicate diagnostic")
        previous=order;indices[diagnostic[0]]+=1;validator.diagnostic(diagnostic[2])
    if indices["parse"]!=value["parse_diagnostics"] or indices["bind"]!=value["bind_diagnostics"]:raise ValueError("diagnostic counts contradict raw records")
    for name in value["qualified_names"]:
        exact_keys(name,("path","raw_hex","identity"),"qualified workload name")
        if type(name["path"]) is not str or not name["path"].startswith("$["):raise ValueError("invalid name observation path")
        name_identity({key:name[key] for key in ("raw_hex","identity")},ref)


def read_reports(binary,env,inputs,requests,workers,directory,runtime):
    directory.mkdir(parents=True,exist_ok=True)
    path=directory/(runtime+".ndjson")
    process=Process([str(binary),str(inputs),str(workers),"--graphs"],directory/(runtime+".stderr"),env=env,deadline=120)
    process.max_record=128*1024*1024
    process.request_expires=time.monotonic()+1800
    rows=[]
    try:
        with path.open("wb") as output:
            for index,request in enumerate(requests):
                value=process.read();check_record(value,index,workers,request)
                rows.append(value);output.write(canonical(value)+b"\n")
                if index%500==0:output.flush();print(f"S07 bindworkload {runtime}/{workers}: {index+1}/{len(requests)}",file=sys.stderr,flush=True)
        process.request_expires=None;process.finish()
    finally:process.close()
    return rows



def read_graph_records(binary, env, inputs, workers, index, directory, runtime):
    """Keep a raw field witness from the same full workload/pool executable."""
    directory.mkdir(parents=True, exist_ok=True)
    process = Process([str(binary), str(inputs), str(workers), f"--graph-records={index}"],
                      directory/(runtime+".stderr"), env=env, deadline=120)
    process.max_record = 128*1024*1024
    process.request_expires = time.monotonic()+1800
    validator = GraphValidator()
    digest = hashlib.sha256()
    rows = []
    try:
        with (directory/(runtime+".ndjson")).open("wb") as output:
            while not validator.closed:
                row = process.read()
                exact_keys(row, ("version","kind","workers","index","ordinal","record_kind","value"), "raw workload graph record")
                for field, expected in (("version",1),("workers",workers),("index",index),("ordinal",len(rows))):
                    integer(row[field],0,2**63-1,field)
                    if row[field] != expected: raise ValueError("raw graph identity/order changed")
                if row["kind"] != "graph_record": raise ValueError("unknown raw graph record")
                validator.accept(row["record_kind"],row["value"])
                record = [row["record_kind"],row["value"]]
                digest.update(canonical(comparable(record))+b"\n")
                rows.append(record)
                output.write(canonical(row)+b"\n")
        process.request_expires = None
        process.finish()
    finally:
        process.close()
    return rows, digest.hexdigest()


def capture_first_witness(run, binaries, inputs, rows, directory):
    failed = next((item for item in run["results"] if not item["equal"]), None)
    if failed is None:
        return None
    index = failed["index"]
    with ThreadPoolExecutor(2) as pool:
        jobs = [pool.submit(read_graph_records, *binary, inputs, run["workers"], index,
                            directory/"first-mismatch", runtime)
                for runtime,binary in binaries]
        graphs = [job.result() for job in jobs]
    for runtime, ((records,digest), reports) in enumerate(zip(graphs,rows,strict=True)):
        if digest != reports[index]["canonical_sha256"] or len(records) != reports[index]["records"]:
            raise ValueError(f"raw graph witness changed native graph for runtime {runtime}")
    difference = first_difference(comparable(graphs[0][0]),comparable(graphs[1][0]))
    witness = {"index":index,"workers":run["workers"],"first_difference":difference,
               "scope":"first mismatching file in this worker mode; every failed per-file report remains in failures.ndjson"}
    (directory/"first-mismatch/report.json").write_bytes(canonical(witness)+b"\n")
    return witness


def requests_from_frozen(inputs):
    requests=strict_json_loads(Path(inputs).read_bytes())
    options=strict_json_loads((ROOT/"data/s07/vscode-parse-options.json").read_bytes())
    workload=strict_json_loads((ROOT/"data/s07/vscode-files.json").read_bytes())
    if len(requests)!=13094 or len(options["files"])!=len(requests) or len(workload["files"])!=len(requests):raise ValueError("workload denominator changed")
    recipes=[]
    for index,(request,option,source) in enumerate(zip(requests,options["files"],workload["files"],strict=True)):
        exact_keys(request,("filename","path","local","script_kind","jsx","force"),"workload input")
        if {key:value for key,value in request.items() if key!="local"}!=option:raise ValueError("input differs from frozen Go parse options")
        if request["filename"]!="/vscode/"+source["path"]:raise ValueError("workload membership/order differs")
        raw=Path(request["local"]).read_bytes()
        if len(raw)!=source["bytes"] or sha(raw)!=source["sha256"]:raise ValueError("workload input bytes changed")
        # The frozen source-tree workload contains no BOM. A changed workload
        # requires a fresh independent loader preflight, never silent decoding.
        if raw.startswith((b"\xff\xfe",b"\xfe\xff",b"\xef\xbb\xbf")):raise ValueError("workload loader representation changed")
        recipes.append({"index":index,**option,"source_bytes":len(raw),"source_sha256":sha(raw)})
    return requests,recipes


def totals(rows):
    return {"files":len(rows),"loaded_bytes":sum(row["source_bytes"] for row in rows),"nodes":sum(row["node_count"] for row in rows),
        "symbols":sum(row["symbol_count"] for row in rows),"parse_diagnostics":sum(row["parse_diagnostics"] for row in rows),"bind_diagnostics":sum(row["bind_diagnostics"] for row in rows)}


def freeze_document(recipes,rows):
    return {"version":1,"files":len(recipes),"workers":[1,8],"operations":"identical benchmark input loading and native parse/bind pool, then canonical graph collection after the timing endpoint",
        "input_sha256":sha(canonical(recipes)),"options_sha256":sha((ROOT/"data/s07/vscode-parse-options.json").read_bytes()),
        "workload_sha256":sha((ROOT/"data/s07/vscode-files.json").read_bytes()),"requests":recipes,
        "expected_scalars":totals(rows),"oracle_graphs":[{"index":row["index"],"canonical_sha256":row["canonical_sha256"],"records":row["records"],"counts":row["counts"]} for row in rows]}


def compare_rows(oracle,rust,recipes,frozen,workers,directory):
    if len(oracle)!=len(recipes) or len(rust)!=len(recipes):raise ValueError("missing benchmark file result")
    results=[]
    with (directory/"failures.ndjson").open("wb") as failures:
        for index,(go,rs,request,expected) in enumerate(zip(oracle,rust,recipes,frozen["oracle_graphs"],strict=True)):
            mismatch=None
            for runtime,row in (("oracle",go),("rust",rs)):
                if row["source_sha256"]!=request["source_sha256"] or row["source_bytes"]!=request["source_bytes"]:
                    mismatch={"path":"source_bytes","runtime":runtime}
            # Freeze Go behavior before evaluating Rust. A worker-dependent Go
            # graph is a failed preflight, not a qualification added on the fly.
            if go["canonical_sha256"]!=expected["canonical_sha256"] or go["records"]!=expected["records"] or go["counts"]!=expected["counts"]:
                mismatch={"path":"oracle_graph_drift","workers":workers}
            comparable_fields=("canonical_sha256","records","counts","diagnostics","node_count","symbol_count","parse_diagnostics","bind_diagnostics")
            mismatch=mismatch or first_difference({key:go[key] for key in comparable_fields},{key:rs[key] for key in comparable_fields})
            results.append({"index":index,"equal":mismatch is None,"raw_exact":go["raw_sha256"]==rs["raw_sha256"],"first_difference":mismatch})
            if mismatch:
                failures.write(canonical({"request":request,"oracle":go,"rust":rs,"first_difference":mismatch})+b"\n");failures.flush()
                print("S07 bindworkload mismatch "+request["filename"]+": "+json.dumps(mismatch),file=sys.stderr,flush=True)
    return {"workers":workers,"files":len(results),"passed_files":sum(item["equal"] for item in results),"parity":sum(item["equal"] for item in results)/len(results),
        "raw_exact_files":sum(item["raw_exact"] for item in results),"expected_scalars":totals(oracle),"rust_scalars":totals(rust),"results":results}


def validate_measurement_prerequisite(report_path, current_source, binaries, current_configuration=None):
    """Return exact scalar obligations only from complete, current two-mode parity."""
    report=strict_json_loads(Path(report_path).read_bytes()) if not isinstance(report_path,dict) else report_path
    if type(report.get("version")) is not int or report.get("version")!=1 or report.get("diagnostic_subset") is not False or report.get("source_stable") is not True:
        raise ValueError("benchmark requires a complete stable graph capture")
    if type(report.get("parity")) not in (int,float) or isinstance(report["parity"],bool) or report["parity"]!=1:
        raise ValueError("benchmark graph parity is not exact")
    if report.get("source_fingerprint")!=current_source or report.get("source_fingerprint_after")!=current_source:
        raise ValueError("graph capture is not for the current production source bytes")
    # Native timing must bind the external Cargo configuration used to build
    # these exact graph artifacts. The optional argument preserves the older
    # protocol used by self-contained historical diagnostic archive replays.
    if current_configuration is not None and (report.get("cargo_configuration")!=current_configuration or report.get("cargo_configuration_after")!=current_configuration):
        raise ValueError("graph capture is not for the current Cargo configuration")
    expected_binaries={"oracle":binaries["go"],"rust":binaries["rust"]}
    if report.get("binary_sha256")!=expected_binaries or report.get("binary_sha256_after")!=expected_binaries:
        raise ValueError("timing binaries differ from the uninstrumented graph binaries")
    frozen=strict_json_loads((ROOT/"data/s07/bindworkload-probes.json").read_bytes())
    if report.get("files")!=13094 or frozen.get("files")!=13094:
        raise ValueError("graph capture changed the complete workload denominator")
    for field in ("input_sha256","options_sha256","workload_sha256"):
        if report.get(field)!=frozen[field]:raise ValueError("graph workload provenance changed: "+field)
    if frozen["options_sha256"]!=sha((ROOT/"data/s07/vscode-parse-options.json").read_bytes()) or frozen["workload_sha256"]!=sha((ROOT/"data/s07/vscode-files.json").read_bytes()):
        raise ValueError("frozen workload/options changed after graph preflight")
    expected=frozen["expected_scalars"]
    exact_keys(expected,SCALARS,"workload scalar obligations")
    for name,value in expected.items():integer(value,0,2**63-1,name)
    if report.get("expected_scalars")!=expected:raise ValueError("graph scalar obligations changed")
    runs=report.get("runs")
    if type(runs) is not list or [run.get("workers") for run in runs]!=[1,8]:raise ValueError("missing, duplicate or reordered worker mode")
    for run in runs:
        integer(run["workers"],1,8,"worker mode")
        if run.get("files")!=13094 or run.get("passed_files")!=13094 or type(run.get("parity")) not in (int,float) or isinstance(run["parity"],bool) or run["parity"]!=1:
            raise ValueError("worker mode did not match every file")
        if run.get("expected_scalars")!=expected or run.get("rust_scalars")!=expected:raise ValueError("worker-mode scalar count mismatch")
        results=run.get("results")
        if type(results) is not list or len(results)!=13094:raise ValueError("missing per-file graph obligation")
        for index,result in enumerate(results):
            exact_keys(result,("index","equal","raw_exact","first_difference"),"file graph result")
            if type(result["index"]) is not int or result["index"]!=index or result["equal"] is not True or type(result["raw_exact"]) is not bool or result["first_difference"] is not None:
                raise ValueError("missing, duplicate, reordered or failed graph file")
    from s07_benchmark_inputs import loaded_input_digest
    return {**expected, "loaded_input_sha256": loaded_input_digest(frozen["requests"])}


def current_capture(report_path):
    """Reuse only the exact current graph prerequisite, without rewriting it.

    Use the timing consumer's full two-mode checks, including every per-file
    result. Reading the shared normal binaries must not rebuild them: a later
    bindworkload ledger run must preserve an existing benchmark's identities.
    """
    report_path = Path(report_path)
    raw = report_path.read_bytes()
    report = strict_json_loads(raw)
    if not isinstance(report, dict):
        raise ValueError("graph capture is not an object")
    source = source_fingerprint()
    configuration = cargo_configuration()
    paths = {runtime: CACHE / "s07-benchmark" / name for runtime, name in
             (("go", "go-benchmark"), ("rust", "rust-benchmark"))}
    binaries = {runtime: sha(path.read_bytes()) for runtime, path in paths.items()}
    validate_measurement_prerequisite(report, source, binaries, configuration)
    if (source_fingerprint() != source or cargo_configuration() != configuration
            or any(sha(path.read_bytes()) != binaries[runtime] for runtime, path in paths.items())
            or report_path.read_bytes() != raw):
        raise ValueError("graph prerequisites changed during reuse validation")
    return report


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation",choices=("freeze","capture"));parser.add_argument("--write-manifest",action="store_true")
    parser.add_argument("--no-build",action="store_true");parser.add_argument("--diagnostic",action="store_true")
    args=parser.parse_args()
    if args.no_build and not args.diagnostic:raise ValueError("--no-build is diagnostic only; final captures must build their binaries")
    if args.operation=="freeze" and args.diagnostic and args.write_manifest:raise ValueError("diagnostic cached binaries cannot freeze obligations")
    build_source=source_fingerprint();build_configuration=cargo_configuration()
    if args.operation=="capture" and args.write_manifest:raise ValueError("graph capture cannot rewrite obligations")
    if args.no_build:
        from s07_benchmark import native_environment
        go=(CACHE/"s07-benchmark/go-benchmark",native_environment());rust=(CACHE/"s07-benchmark/rust-benchmark",native_environment())
        inputs=CACHE/"s07-benchmark/inputs.json"
    else:
        go=build_go();inputs,options=provision_inputs(*go)
        if options!=strict_json_loads((ROOT/"data/s07/vscode-parse-options.json").read_bytes()):raise ValueError("Go parse options changed")
        rust=build_rust() if args.operation=="capture" else None
    source=source_fingerprint();configuration=cargo_configuration()
    build_stable=source==build_source and configuration==build_configuration and not args.no_build
    if not build_stable and not args.diagnostic:raise ValueError("production sources or Cargo configuration changed while graph binaries were building")
    requests,recipes=requests_from_frozen(inputs)
    output=ROOT/"target/s07-bindworkload";output.mkdir(exist_ok=True,parents=True)
    frozen_path=ROOT/"data/s07/bindworkload-probes.json"
    if args.operation=="freeze":
        rows=read_reports(*go,inputs,requests,1,output/"preflight","oracle")
        document=freeze_document(recipes,rows);content=canonical(document)+b"\n"
        if (source_fingerprint()!=source or cargo_configuration()!=configuration) and not args.diagnostic:raise ValueError("production sources or Cargo configuration changed during Go graph preflight")
        if args.write_manifest:frozen_path.write_bytes(content)
        elif not frozen_path.exists() or frozen_path.read_bytes()!=content:raise ValueError("frozen bindworkload observations drifted")
        print(json.dumps({"files":len(rows),"expected_scalars":document["expected_scalars"],"input_sha256":document["input_sha256"]}));return
    frozen=strict_json_loads(frozen_path.read_bytes())
    if frozen["input_sha256"]!=sha(canonical(recipes)):raise ValueError("frozen bindworkload request hash changed")
    binary_hashes={"oracle":sha(go[0].read_bytes()),"rust":sha(rust[0].read_bytes())}
    # Invalidate an older successful report before launching a fresh capture.
    (output/"report.json").write_bytes(canonical({"version":1,"status":"capture_in_progress","diagnostic_subset":args.diagnostic,"parity":None})+b"\n")
    runs=[]
    for workers in (1,8):
        directory=output/("workers-"+str(workers))
        with ThreadPoolExecutor(2) as pool:
            jobs=[pool.submit(read_reports,*binary,inputs,requests,workers,directory,runtime) for runtime,binary in (("oracle",go),("rust",rust))]
            oracle_rows,rust_rows=[job.result() for job in jobs]
        run=compare_rows(oracle_rows,rust_rows,recipes,frozen,workers,directory)
        run["first_mismatch_witness"]=capture_first_witness(run,(("oracle",go),("rust",rust)),inputs,(oracle_rows,rust_rows),directory)
        runs.append(run)
    after=source_fingerprint();configuration_after=cargo_configuration()
    binary_hashes_after={"oracle":sha(go[0].read_bytes()),"rust":sha(rust[0].read_bytes())}
    stable=build_stable and source==after and configuration==configuration_after and binary_hashes==binary_hashes_after
    report={"version":1,"diagnostic_subset":args.diagnostic,"source_stable":stable,"files":len(recipes),"parity":min(run["parity"] for run in runs),
        "expected_scalars":frozen["expected_scalars"],"input_sha256":frozen["input_sha256"],"options_sha256":frozen["options_sha256"],"workload_sha256":frozen["workload_sha256"],
        "source_fingerprint":source,"source_fingerprint_after":after,"cargo_configuration":build_configuration,"cargo_configuration_after":configuration_after,
        "binary_sha256":binary_hashes,"binary_sha256_after":binary_hashes_after,"runs":runs}
    if not stable or args.diagnostic:report["parity"]=None
    (output/"report.json").write_bytes(canonical(report)+b"\n")
    print(json.dumps({key:value for key,value in report.items() if key not in ("runs","source_fingerprint","source_fingerprint_after")}))
    if not stable and not args.diagnostic:raise ValueError("source, Cargo configuration or binaries changed during graph capture")
    # Complete measured mismatches remain valid captures with a failing parity
    # metric; missing prerequisites and malformed protocols still raise above.


if __name__=="__main__":main()
