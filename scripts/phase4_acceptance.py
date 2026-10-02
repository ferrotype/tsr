"""Replay the independent X7 witnesses selected by an external capture index.

No runner is started here. Missing/stale/partial witnesses withhold a metric;
complete measured failures report false. A bad group cannot hide another
group's evidence. Host gate metrics describe this host, as PHASE4-plan §5
requires; *_all_hosts additionally joins the recorded macOS and Linux facts.
"""
from __future__ import annotations

import hashlib
from pathlib import Path
import platform
import re

from s04_common import strict_json_loads

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_INDEX = ROOT / "target/phase4/acceptance.json"
HOSTS = ("macos", "linux")
GROUPS = {"native": ("smoke", "buildinfo_interop"),
          "live": ("live_watch_parity",), "determinism": ("determinism",),
          "thread_sanitizer": ("thread_sanitizer",)}


def current_host():
    return {"Darwin": "macos", "Linux": "linux"}.get(platform.system(), "unsupported")


def verifiers():
    # Lazy imports keep the join independent of runner command-line setup.
    import phase4_native
    import phase4_live
    import phase4_determinism
    import phase4_sanitizer
    return {"native": phase4_native.verify_witnesses,
            "live": phase4_live.verify_witnesses,
            "determinism": phase4_determinism.verify_witnesses,
            "thread_sanitizer": phase4_sanitizer.verify_witnesses}


def path_from(base, value):
    if not isinstance(value, str) or not value.strip():
        raise ValueError("capture paths must be nonempty strings")
    path = Path(value)
    return path if path.is_absolute() else base / path


def entries(group, value, base):
    if not isinstance(value, list):
        raise ValueError(f"{group} must be a list")
    if group == "determinism" and len(value) > 1:
        raise ValueError("determinism admits one five-run capture")
    result, seen = [], set()
    for item in value:
        required = {"capture"} if group == "determinism" else {"host", "capture"}
        if group in ("native", "live"):
            required.add("build")
        if not isinstance(item, dict) or set(item) != required:
            raise ValueError(f"{group} entry must contain exactly {sorted(required)}")
        host = item.get("host")
        if group != "determinism":
            if host not in HOSTS or host in seen:
                raise ValueError(f"{group} host must be unique and one of {HOSTS}")
            seen.add(host)
        result.append({key: path_from(base, value) if key != "host" else value
                       for key, value in item.items()})
    return result


def validate_result(group, result):
    if not isinstance(result, dict) or not isinstance(result.get("metrics"), dict):
        raise ValueError("witness verifier returned no metric map")
    metrics, identities = result["metrics"], result.get("identities")
    if set(metrics) - set(GROUPS[group]) or not isinstance(identities, dict) or set(identities) != set(metrics):
        raise ValueError("witness metric/identity inventory differs")
    for name, value in metrics.items():
        identity = identities[name]
        if type(value) is not bool or not isinstance(identity, str) or not re.fullmatch(r"[0-9a-f]{64}", identity):
            raise ValueError(f"invalid witness result for {name}")
    return metrics, identities


def collect(index=None, *, host=None, replay=None):
    index = Path(index or DEFAULT_INDEX)
    host = host or current_host()
    output = {"metrics": {}, "identities": {}, "unavailable": {}}
    try:
        raw = index.read_bytes()
        document = strict_json_loads(raw)
        if (not isinstance(document, dict) or type(document.get("version")) is not int
                or document["version"] != 1 or set(document) - {"version", *GROUPS}):
            raise ValueError("expected witness index version 1 and only named witness groups")
    except (OSError, ValueError) as error:
        output["unavailable"]["index"] = str(error)
        return output
    output["index_sha256"] = hashlib.sha256(raw).hexdigest()
    replay = replay or verifiers()
    observed = {name: {} for names in GROUPS.values() for name in names}
    for group in GROUPS:
        try:
            selected = entries(group, document.get(group, []), index.parent)
        except ValueError as error:
            output["unavailable"][group] = str(error)
            continue
        for entry in selected:
            witness_host = entry.get("host", "independent")
            label = f"{group}/{witness_host}"
            try:
                args = ([entry["build"], entry["capture"]] if "build" in entry else [entry["capture"]])
                kwargs = {"expected_host": witness_host} if "host" in entry else {}
                metrics, identities = validate_result(group, replay[group](*args, **kwargs))
                for name, value in metrics.items():
                    observed[name][witness_host] = value
                    output["identities"].setdefault(name, {})[witness_host] = identities[name]
                for name in set(GROUPS[group]) - set(metrics):
                    output["unavailable"][f"{label}/{name}"] = "required witness group was not fully executed"
            except (OSError, ValueError, KeyError, TypeError, RuntimeError) as error:
                output["unavailable"][label] = str(error)
    for name, by_host in observed.items():
        for witness_host, value in by_host.items():
            if witness_host != "independent":
                output["metrics"][f"{name}_{witness_host}"] = value
        selected_host = "independent" if name == "determinism" else host
        if selected_host in by_host:
            output["metrics"][name] = by_host[selected_host]
        else:
            output["unavailable"].setdefault(name, f"no current complete witness for {selected_host}")
        if name in ("smoke", "live_watch_parity") and set(HOSTS) <= by_host.keys():
            output["metrics"][f"{name}_all_hosts"] = all(by_host[item] for item in HOSTS)
    return output
