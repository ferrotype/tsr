"""Freeze exact operation requests for the bounded generated-AST vertical slice.

This emits inputs and operation identities, never expected output. Extend the
schema-backed dispatch only after every newly claimed operation executes.
"""
import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
_shape_module = {"__file__": str(Path(__file__).with_name("generate_shapes.py"))}
exec(compile(Path(_shape_module["__file__"]).read_text(), _shape_module["__file__"], "exec"), _shape_module)
SHAPES = ("QualifiedName", "Block", "JSDocParameterOrPropertyTag")


def predicate_dispatch():
    go = (ROOT / "upstream/tsc/internal/ast/ast_generated.go").read_text()
    rust = (ROOT / "crates/tsr_ast/src/runtime_generated.rs").read_text()
    declarations = re.findall(r"^func (Is\w+)\((node \*Node|kind Kind)\) bool \{", go, re.M)
    rust_matches = re.findall(r"// upstream: tsc/internal/ast/ast_generated.go:(Is\w+)\s+pub fn (\w+)\(", rust)
    rust_declarations = dict(rust_matches)
    if len(rust_matches) != len(rust_declarations):
        raise ValueError("duplicate generated Rust predicate identity")
    scope = json.loads((ROOT / "data/s06/generated-ast-scope.json").read_text())
    expected = {identity.rsplit(":", 1)[1] for identity in scope["functions"]
                if identity.startswith("tsc/internal/ast/ast_generated.go:Is")}
    if expected != {name for name, _ in declarations}:
        raise ValueError("generated predicate signature inventory changed")
    missing = {name for name, _ in declarations} - rust_declarations.keys()
    if missing:
        raise ValueError(f"generated predicate lacks production Rust dispatch: {sorted(missing)}")
    return [(name, argument, rust_declarations[name]) for name, argument in declarations]


AST = "tsc/internal/ast/ast.go:"


def runtime_actions(shape, mode, actions):
    """The typed actions with the shared runtime operations the probe enters:
    the factory, its counters and node and list positions; the 'new'
    snapshot's full child walk (Node.ForEachChild and the pinned body's visit
    helpers; visitNodes only on a built list); updateNode on the changed
    update; cloneNode; Node.VisitEachChild and the JSDoc tag's hand-written
    visitor."""
    listed = mode != "nil" and shape != "QualifiedName"
    walk = [AST + "visit"] if shape != "Block" else []
    if shape != "QualifiedName":
        walk.append(AST + "visitNodeList")
        if listed:
            walk.append(AST + "visitNodes")
    if shape == "JSDocParameterOrPropertyTag":
        walk.append(AST + "forEachChild_JSDocParameterOrPropertyTag")
    extra = {"new": [AST + "NewNodeFactory", AST + "NodeFactory.newNode", AST + "newNode", AST + "Node.Pos",
                     AST + "Node.End", "tsc/internal/ast/ast_generated.go:Node.ForEachChild"]
             + ([AST + "NodeList.Pos", AST + "NodeList.End"] if listed else []) + walk,
             "children-stop": ["tsc/internal/ast/ast_generated.go:Node.ForEachChild"],
             "update-changed": [AST + "updateNode"], "clone": [AST + "cloneNode"],
             "visit-same": [AST + "Node.VisitEachChild"], "visit-replace": [AST + "Node.VisitEachChild"],
             "counts": [AST + "NodeFactory.NodeCount", AST + "NodeFactory.TextCount"]}
    if shape == "JSDocParameterOrPropertyTag":
        for label in ("visit-same", "visit-replace"):
            extra[label] = extra[label] + [AST + "visitEachChild_JSDocParameterOrPropertyTag"]
    return {label: ops + [op for op in extra.get(label, []) if op not in ops] for label, ops in actions.items()}


def document():
    schema = json.loads((ROOT / "data/s03/schema/ast.json").read_text())
    nodes = {row["name"]: row for row in schema["nodes"]}
    scope = json.loads((ROOT / "data/s06/generated-ast-scope.json").read_text())
    ids = set(scope["functions"])
    requests = []
    prefix = "tsc/internal/ast/ast_generated.go:"
    for shape in SHAPES:
        assert shape in nodes
        claims = [prefix + "NodeFactory.New" + shape, prefix + "NodeFactory.Update" + shape,
                  prefix + shape + ".Clone", prefix + shape + ".ForEachChild",
                  prefix + shape + ".VisitEachChild"]
        if nodes[shape]["generateSubtreeFacts"]:
            claims.append(prefix + shape + ".computeSubtreeFacts")
        assert set(claims) <= ids, (shape, set(claims) - ids)
        actions = {"new": [claims[0]], "update-same": [claims[1]],
                   "update-changed": [claims[1]], "clone": [claims[2]],
                   "children-stop": [claims[3]], "visit-same": [claims[4]],
                   "visit-replace": [claims[4]]}
        if nodes[shape]["generateSubtreeFacts"]:
            actions["facts"] = [claims[-1]]
        actions["counts"] = []
        schedule = ["new", "children-stop", "update-same", "update-changed",
                    "clone", "visit-same", "visit-replace"]
        if nodes[shape]["generateSubtreeFacts"]:
            schedule.append("facts")
        schedule.append("counts")
        for absent in (False, True):
            for mode in (("nil",) if shape == "QualifiedName" else ("nil", "empty", "nodes", "nil-element")):
                for first in ((False, True) if shape == "JSDocParameterOrPropertyTag" else (False,)):
                    identity = f"syntax/generated-ast/{shape}/{int(absent)}-{mode}-{int(first)}"
                    linked = runtime_actions(shape, mode, actions)
                    requests.append({"case": identity, "operation": claims[0], "subject": "generatedAst",
                        "shape": shape, "absent": absent, "list": mode, "name_first": first,
                        "actions": [{"op": name} for name in schedule],
                        "operations": claims + sorted({op for ops in linked.values() for op in ops} - set(claims)),
                        "operation_actions": linked, "discriminates": "typed field wiring, update/clone identity and hooks, visitor ordering, nil/list behavior and subtree facts"})
    upper = max(row["value"] for row in schema["kinds"]) + 1
    for name, argument, _ in predicate_dispatch():
        operation = prefix + name
        assert operation in ids
        requests.append({"case": f"syntax/generated-predicate/{name}",
            "operation": operation, "operations": [operation], "subject": "generatedPredicate",
            "predicate": name, "argument": argument, "first_kind": -1, "last_kind": upper,
            "extra_kinds": [-32768, 32767],
            "discriminates": "actual pinned predicate and production Rust predicate on every declared kind and invalid raw-kind boundaries"})
    requests.extend(_shape_module["requests"]())
    return {"version": 1, "family": "syntax", "requests": requests}


def generated_sources():
    rust = ["// Generated test dispatch by generate.py. No expected values or predicate logic.",
            "use tsr_ast::NodeRead;", "#[rustfmt::skip]",
            "#[allow(clippy::too_many_lines)] // Exact generated operation dispatch, no algorithm implementation.", "pub fn call(name: &str, node: &NodeRead<'_>) -> Option<bool> {", "    Some(match name {"]
    go = ["// Code generated by generate.py: test dispatch only, no expected predicate logic.",
          "package ast", "", "func phase1GeneratedPredicate(name string, node *Node) bool {", "\tswitch name {"]
    for name, argument, rust_name in predicate_dispatch():
        rust_arg = "node.kind()" if argument == "kind Kind" else "node"
        go_arg = "node.Kind" if argument == "kind Kind" else "node"
        rust.append(f'        "{name}" => tsr_ast::{rust_name}({rust_arg}),')
        go.append(f'\tcase "{name}":\n\t\treturn {name}({go_arg})')
    rust += ["        _ => return None,", "    })", "}"]
    go += ['\tdefault:\n\t\tpanic("unknown generated predicate")', "\t}", "}"]
    return {"predicates.rs": "\n".join(rust) + "\n", "predicates_test.go": "\n".join(go) + "\n"}


def outputs():
    return {**generated_sources(), **_shape_module["sources"](), "requests.json": json.dumps(document(), indent=2, sort_keys=True) + "\n"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    base = Path(__file__).parent
    for name, content in outputs().items():
        path = base / name
        if args.check:
            if not path.is_file() or path.read_text() != content:
                raise SystemExit(f"generated AST fixture drift: {path}")
        else:
            path.write_text(content)
