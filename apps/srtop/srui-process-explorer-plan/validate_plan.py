#!/usr/bin/env python3
"""Validate planning consistency, not the SRUI implementation."""
from pathlib import Path
import json
import re


def main() -> int:
    root = Path(__file__).resolve().parent
    index = json.loads((root / "task-index.json").read_text(encoding="utf-8"))
    tasks = index["tasks"]
    ledger = json.loads((root / "feature-ledger.seed.json").read_text(encoding="utf-8"))["families"]
    master = (root / "IMPLEMENTATION_PLAN.md").read_text(encoding="utf-8")
    contract = (root / "STANDING_AGENT_CONTRACT.md").read_text(encoding="utf-8")
    contract = contract.split("\n\n", 1)[1].strip()
    by_id = {t["id"]: t for t in tasks}
    errors = []
    if len(by_id) != len(tasks):
        errors.append("Duplicate ticket IDs")
    original_ids = {f"PX-{i:03d}" for i in range(140)}
    if not original_ids.issubset(by_id):
        errors.append("An original PX-000 through PX-139 ID was removed")
    if any(not re.fullmatch(r"PX-\d{3}(?:-G\d{2})?", tid) for tid in by_id):
        errors.append("Invalid ticket ID")
    if len({t["key"] for t in tasks}) != len(tasks):
        errors.append("Duplicate task keys")
    queue = (root / "TASK_INDEX.md").read_text(encoding="utf-8")
    fence = chr(96) * 3
    for t in tasks:
        path = root / t["file"]
        if not path.is_file():
            errors.append(f"Missing ticket: {t['id']}")
            continue
        text = path.read_text(encoding="utf-8")
        if f"Edition {index['edition']}" not in text:
            errors.append(f"Stale edition: {t['id']}")
        if f"## {t['id']} — {t['title']}" not in text:
            errors.append(f"Title mismatch: {t['id']}")
        metadata = (
            f"**Phase:** {t['phase']} · **Scope size:** {t['size']} · "
            f"**Status:** {t['status']} · **Gate:** {t['gate']}"
        )
        if metadata not in text:
            errors.append(f"Metadata mismatch: {t['id']}")
        dependencies = ", ".join(t["depends_on"]) or "Revision-specific baseline audit"
        if f"**Dependencies:** {dependencies}\n" not in text:
            errors.append(f"Dependency text mismatch: {t['id']}")
        expected_aliases = [by_id[d]["key"] for d in t["depends_on"] if d in by_id]
        if t["deps"] != expected_aliases:
            errors.append(f"Dependency aliases mismatch: {t['id']}")
        for dep in t["depends_on"]:
            if dep not in by_id:
                errors.append(f"Unknown dependency {dep} for {t['id']}")
        if f"{fence}text\n{contract}\n{fence}" not in text:
            errors.append(f"Shared contract mismatch: {t['id']}")
        marker = f"#### {t['id']} — {t['title']}\n"
        if master.count(marker) != 1:
            errors.append(f"Master ticket count/title mismatch: {t['id']}")
            block = ""
        else:
            block = master.split(marker, 1)[1].split("\n#### ", 1)[0]
        if metadata not in block or f"**Dependencies:** {dependencies}\n" not in block:
            errors.append(f"Master metadata mismatch: {t['id']}")
        for heading, field, following in [
            ("Build", "build", "Out of scope"),
            ("Out of scope", "out", "Verification / acceptance criteria"),
            ("Verification / acceptance criteria", "verify", "Handoff and completion"),
        ]:
            if not t[field].strip():
                errors.append(f"Empty {field}: {t['id']}")
            for level, target in [("###", text), ("#####", block)]:
                expected = f"{level} {heading}\n\n{t[field]}\n\n{level} {following}"
                if expected not in target:
                    errors.append(f"{level} {field} mismatch: {t['id']}")
        if f"docs/process-explorer/completions/{t['id']}.md" not in text:
            errors.append(f"Missing completion path: {t['id']}")
        row = f"| [{t['id']}]({t['file']}) | {t['title']} | {', '.join(t['depends_on']) or 'Baseline audit'} | {t['gate']} |"
        if row not in queue:
            errors.append(f"Queue catalogue mismatch: {t['id']}")
    visiting, visited = set(), set()

    def visit(tid: str) -> None:
        if tid in visiting:
            errors.append(f"Dependency cycle at {tid}")
            return
        if tid in visited or tid not in by_id:
            return
        visiting.add(tid)
        for dep in by_id[tid]["depends_on"]:
            visit(dep)
        visiting.remove(tid)
        visited.add(tid)

    for tid in by_id:
        visit(tid)
    order = index["execution_order"]
    if len(order) != len(tasks) or set(order) != set(by_id):
        errors.append("Execution order must contain each ticket exactly once")
    position = {tid: i for i, tid in enumerate(order)}
    for t in tasks:
        for dep in t["depends_on"]:
            if position.get(dep, len(order)) >= position.get(t["id"], -1):
                errors.append(f"Execution order violates {dep} -> {t['id']}")
    queue_ids = re.findall(r"^\d+\. \[(PX-[^]]+)\]", queue, re.M)
    if queue_ids != order:
        errors.append("Markdown execution order differs from JSON")
    for name, tid in index["delivery_milestones"].items():
        if tid not in by_id:
            errors.append(f"Unknown milestone {name}: {tid}")
    for family in ledger:
        if not family["tickets"]:
            errors.append(f"No owner for {family['family_id']}")
        for owner in family["tickets"]:
            if owner not in by_id:
                errors.append(f"Unknown owner {owner} for {family['family_id']}")
        if family["status"] != "planned" or family["evidence"]:
            errors.append(f"Fabricated seed evidence: {family['family_id']}")
    actual_files = {p.relative_to(root).as_posix() for p in (root / "tickets").glob("*.md")}
    if actual_files != {t["file"] for t in tasks}:
        errors.append("Ticket file set differs from index")
    for path in root.rglob("*.md"):
        text = path.read_text(encoding="utf-8")
        if sum(line.startswith(fence) for line in text.splitlines()) % 2:
            errors.append(f"Unbalanced fences: {path.name}")
        for target in re.findall(r"\[[^]]*\]\(([^)]+)\)", text):
            if re.match(r"[a-zA-Z][a-zA-Z0-9+.-]*:", target) or target.startswith("#"):
                continue
            local = target.split("#", 1)[0].strip("<>")
            if local and not (path.parent / local).exists():
                errors.append(f"Broken link in {path.name}: {target}")
    for error in errors:
        print("FAIL:", error)
    if errors:
        return 1
    print(f"PASS: {len(tasks)} unique tickets; original IDs preserved; dependencies resolve; graph acyclic.")
    print("PASS: JSON, master, standalone tickets, shared contracts, and execution queue agree.")
    print(f"PASS: {len(actual_files)} ticket files; local Markdown links and fences valid.")
    print(f"PASS: {len(ledger)} feature families with valid owners and no fabricated evidence.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
