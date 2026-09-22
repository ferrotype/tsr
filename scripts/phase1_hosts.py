"""Reviewed host applicability for the supported Phase 1 CI matrix.

`any` and `posix` both require Linux and Darwin evidence today. They describe
different case contracts, not evidence for operating systems outside that
matrix. A new CI host must extend this inventory before it can certify cases.
"""

SUPPORTED_GOOS = ("linux", "darwin")
HOST_TAGS = (*SUPPORTED_GOOS, "any", "posix")


def request_hosts(request: dict) -> list[str]:
    tags = request.get("hosts", ["any"])
    if (not isinstance(tags, list) or not tags
            or any(not isinstance(tag, str) or tag not in HOST_TAGS for tag in tags)
            or len(set(tags)) != len(tags)
            or (len(tags) != 1 and any(tag in ("any", "posix") for tag in tags))):
        raise ValueError(f"{request.get('case', '<case>')}: invalid hosts {tags!r}")
    return list(tags)


def required_goos(request: dict) -> tuple[str, ...]:
    tags = request_hosts(request)
    return SUPPORTED_GOOS if tags in (["any"], ["posix"]) else tuple(tags)


def applies(request: dict, goos: str) -> bool:
    if goos not in SUPPORTED_GOOS:
        raise ValueError(f"unsupported Phase 1 capture GOOS {goos!r}")
    return goos in required_goos(request)


def validate_case(request: dict, case: dict) -> None:
    """A reviewed case cannot advertise different hosts from its actual input."""
    if "hosts" not in request or "hosts" not in case:
        raise ValueError(f"{request['case']}: filesystem case and request must both declare hosts")
    if set(request_hosts(request)) != set(request_hosts(case)):
        raise ValueError(f"{request['case']}: filesystem case hosts differ from its request")
