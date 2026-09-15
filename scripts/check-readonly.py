#!/usr/bin/env python3
"""Fail if src/ calls an AWS SDK operation that isn't read-only.

neboto's core promise is that it only ever issues describe/list/get-style
calls (README "Why read-only", PERMISSIONS.md). This makes that mechanical:

1. The universe of SDK operations is read from the `aws-sdk-*` crates in
   Cargo.lock (`src/operation/<op>` in each crate's source — cargo metadata
   gives the paths, so the set is exact for the pinned versions).
2. Every `.<op>()` call in src/ whose name is in that universe is an SDK
   call — this is shape-independent, so it sees through macros, builders
   parked in variables and paginators, all of which a `.send()`-based walk
   missed.
3. The op's verb must be on READ_VERBS, or the op must be in ALLOW_OPS with a
   written reason. Anything else fails.

Also checks PERMISSIONS.md the same way: every IAM action it grants must have
a read-only verb or be in ALLOW_ACTIONS.

Usage: scripts/check-readonly.py [--list]   (--list prints every op found)
Runs in CI (.github/workflows/ci.yml). Needs python3 and a fetched registry
(any cargo build/check/metadata has done that).
"""
import json, re, subprocess, sys, pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent

# Verb prefixes that are read-only by AWS convention (snake_case, SDK side).
READ_VERBS = (
    "describe", "list", "get", "lookup", "search", "select", "filter", "query",
    "scan", "head", "batch_get", "check", "estimate", "retrieve", "count",
    "view", "preview", "simulate", "validate", "decode", "detect", "evaluate",
    "download", "is_", "read",
)
# Operations whose name doesn't start with a read verb but which mutate
# nothing in the account. Each needs a reason; PRs adding here get a hard look.
ALLOW_OPS = {
    "assume_role": "STS AssumeRole for member-account switching — scoped by a ReadOnlyAccess session policy (README: Multi-account)",
    "start_query": "CloudWatch Logs Insights StartQuery — runs a read-only log query, creates no resource",
    "start_live_tail": "CloudWatch Logs StartLiveTail — streaming read of log events",
}

READ_ACTIONS_RE = re.compile(
    r"^(Describe|List|Get|Lookup|Search|Select|Filter|Query|Scan|Head|BatchGet|Check|Estimate|Retrieve|Count|View|Preview|Simulate|Validate|Decode|Detect|Read|Evaluate|Test|Download)"
)
ALLOW_ACTIONS = {
    "sts:AssumeRole": "member-account switch, ReadOnlyAccess session policy",
    "sts:GetCallerIdentity": "read",
    "logs:StartQuery": "Logs Insights query (read)",
    "logs:StopQuery": "stops our own Insights query",
    "logs:StartLiveTail": "live tail (read)",
    "ssm:StartSession": "opens an interactive session via the aws CLI — user-initiated, changes no resource",
    "ssm:TerminateSession": "ends our own session",
}

def sdk_operations():
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"], cwd=ROOT))
    ops = {}
    for pkg in meta["packages"]:
        if not pkg["name"].startswith("aws-sdk-"):
            continue
        opdir = pathlib.Path(pkg["manifest_path"]).parent / "src" / "operation"
        if not opdir.is_dir():
            continue
        for entry in opdir.iterdir():
            name = entry.name[:-3] if entry.suffix == ".rs" else entry.name
            if name in ("mod",) or not re.fullmatch(r"[a-z0-9_]+", name):
                continue
            ops.setdefault(name, set()).add(pkg["name"])
    return ops

CALL = re.compile(r"\.(?:r#)?([a-z][a-z0-9_]*)\(\s*\)")
CLIENT_RECEIVER = re.compile(r"(client|_client\(\)|Client::new\([^)]*\)|clients\.[a-z0-9_]+\(\))\s*$")

def is_client_call(text, start, end):
    """SDK data types have accessors that collide with operation names
    (`attr.tag()`, `cb.enable()`); a real operation is called on a client
    and/or its chain ends in `.send()` before the statement does."""
    receiver = text[max(0, start - 80):start].rstrip()
    if CLIENT_RECEIVER.search(receiver):
        return True
    stmt_end = text.find(";", end)
    tail = text[end:stmt_end if stmt_end != -1 else end + 400]
    return ".send(" in tail or ".into_paginator(" in tail or ".customize(" in tail

def main():
    list_mode = "--list" in sys.argv
    universe = sdk_operations()
    if len(universe) < 500:
        print(f"only {len(universe)} SDK operations found — registry not fetched?"); sys.exit(2)

    found, bad, collisions = {}, [], []
    for path in sorted((ROOT / "src").rglob("*.rs")):
        if "harness_tests" in path.parts:
            continue
        text = path.read_text(encoding="utf-8")
        text = re.sub(r"(?m)//[^\n]*$", "", text)          # drop line comments
        for m in CALL.finditer(text):
            op = m.group(1)
            if op not in universe:
                continue
            where = f"{path.relative_to(ROOT)}:{text.count(chr(10), 0, m.start()) + 1}"
            if op.startswith(READ_VERBS) or op in ALLOW_OPS:
                found.setdefault(op, []).append(where)
            elif is_client_call(text, m.start(), m.end()):
                found.setdefault(op, []).append(where)
                bad.append((op, where))
            else:
                collisions.append((op, where))

    perm = (ROOT / "PERMISSIONS.md").read_text(encoding="utf-8")
    bad_actions = []
    for act in sorted(set(re.findall(r'"([a-z0-9-]+:[A-Z][A-Za-z0-9]+)"', perm))):
        if READ_ACTIONS_RE.match(act.split(":", 1)[1]) or act in ALLOW_ACTIONS:
            continue
        bad_actions.append(act)

    if list_mode:
        for op in sorted(found):
            tag = "" if (op.startswith(READ_VERBS) or op in ALLOW_OPS) else "  <-- NOT READ-ONLY"
            print(f"{op:45} {len(found[op]):3} call sites  ({', '.join(sorted(universe[op]))}){tag}")
        print(f"\n{len(found)} distinct SDK operations at {sum(map(len, found.values()))} call sites "
              f"(universe: {len(universe)} ops across the pinned aws-sdk-* crates)")

    ok = True
    if bad:
        ok = False
        print("Non-read-only SDK operations in src/ (only add to ALLOW_OPS, with a reason, if it truly mutates nothing):")
        for op, where in bad:
            print(f"  {op:40} {where}")
    if bad_actions:
        ok = False
        print("Non-read-only IAM actions in PERMISSIONS.md:")
        for a in bad_actions:
            print(f"  {a}")
    if list_mode and collisions:
        print("accessor name collisions ignored (not client calls): " +
              ", ".join(f"{op}@{w}" for op, w in collisions))
    if ok and not list_mode:
        print(f"read-only guard: {len(found)} distinct SDK operations at "
              f"{sum(map(len, found.values()))} call sites, all read-only; PERMISSIONS.md clean")
    sys.exit(0 if ok else 1)

if __name__ == "__main__":
    main()
