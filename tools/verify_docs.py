#!/usr/bin/env python3
"""Validate Podway's English documentation, local links, and active roadmap."""

from __future__ import annotations

from pathlib import Path
import re
import sys
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"
ROADMAP = DOCS / "roadmap/README.md"
V2_DOSSIER = DOCS / "todo/TODO-podway-v2-full-feature-ga.md"
REQUIRED_SECTIONS = (
    "architecture",
    "architecture-decision-records",
    "specs",
    "implementation-tips",
    "todo",
    "deferred-feedback",
    "roadmap",
)
RETIRED_PATHS = (
    "docs/reference",
    "docs/adr",
    "docs/presets",
    "docs/schemas",
    "docs/spec",
    "presets",
    "schemas",
    "spec",
)
LINK_RE = re.compile(r"(?<!!)\[[^\]]+\]\(([^)]+)\)")
HEADING_RE = re.compile(r"^(#{1,6})\s+(.+?)\s*$")
HANGUL_RE = re.compile(r"[\u1100-\u11ff\u3130-\u318f\uac00-\ud7af]")
EPIC_RE = re.compile(r"^## ([A-Z0-9]{5}) — (.+)$")
TASK_RE = re.compile(
    r"^\| `([A-Z0-9]{5})(-?)(\d{3})` \| (.+?) \| "
    r"(Planned|In Progress|In Review|Completed|Deferred|Blocked) \| (.+?) \| (.+?) \|$"
)
LEGACY_COMPACT_TASK_EPICS = frozenset({"REL12"})


class DocumentationError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise DocumentationError(message)


def markdown_files() -> list[Path]:
    files = [ROOT / "README.md", ROOT / "RELEASE_NOTES.md"]
    files.extend(sorted(DOCS.rglob("*.md")))
    return files


def github_anchor(title: str) -> str:
    title = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", title)
    title = title.replace("`", "").lower()
    title = re.sub(r"[^\w\- ]", "", title, flags=re.UNICODE)
    return re.sub(r"[\s-]+", "-", title).strip("-")


def anchors(path: Path) -> set[str]:
    found: set[str] = set()
    duplicates: dict[str, int] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        match = HEADING_RE.match(line)
        if match is None:
            continue
        base = github_anchor(match.group(2))
        count = duplicates.get(base, 0)
        duplicates[base] = count + 1
        found.add(base if count == 0 else f"{base}-{count}")
    return found


def split_target(raw: str) -> tuple[str, str]:
    target = raw.strip()
    if target.startswith("<") and target.endswith(">"):
        target = target[1:-1]
    path, separator, fragment = target.partition("#")
    return unquote(path), unquote(fragment) if separator else ""


def validate_links(files: list[Path]) -> int:
    checked = 0
    anchor_cache: dict[Path, set[str]] = {}
    for source in files:
        text = source.read_text(encoding="utf-8")
        for raw_target in LINK_RE.findall(text):
            if raw_target.startswith(("https://", "http://", "mailto:")):
                continue
            relative, fragment = split_target(raw_target)
            target = source if not relative else (source.parent / relative).resolve()
            if not target.is_relative_to(ROOT) or not target.exists():
                fail(f"broken documentation link in {source.relative_to(ROOT)}: {raw_target}")
            if fragment:
                if not target.is_file() or target.suffix.lower() != ".md":
                    fail(f"fragment targets a non-Markdown file in {source.relative_to(ROOT)}: {raw_target}")
                available = anchor_cache.setdefault(target, anchors(target))
                if fragment.lower() not in available:
                    fail(f"unknown heading in {source.relative_to(ROOT)}: {raw_target}")
            checked += 1
    return checked


def validate_english(files: list[Path]) -> None:
    for path in files:
        text = path.read_text(encoding="utf-8")
        match = HANGUL_RE.search(text)
        if match is not None:
            line = text.count("\n", 0, match.start()) + 1
            fail(f"non-English Hangul text in {path.relative_to(ROOT)}:{line}")


def validate_layout() -> None:
    for section in REQUIRED_SECTIONS:
        index = DOCS / section / "README.md"
        if not index.is_file():
            fail(f"documentation section omits README.md: docs/{section}")
    for retired in RETIRED_PATHS:
        if (ROOT / retired).exists():
            fail(f"retired documentation or asset path still exists: {retired}")


def validate_v2_dossier() -> None:
    text = V2_DOSSIER.read_text(encoding="utf-8")
    governance = text.partition("### 2.1 Governance decisions")[2].partition("### 2.2 Integration notices")[0]
    if "[ADR-0018](../architecture-decision-records/0018-v2-success-envelope.md)" not in governance:
        fail("v2 dossier governance omits accepted ADR-0018")

    diagnostics = text.partition("### 11.6 Stable diagnostics")[2].partition("## 12. YAML Authority and Graph Projections")[0]
    for field in ('"source_path": "workflow.yaml"', '"location": {'):
        if field not in diagnostics:
            fail(f"v2 authoring diagnostic example omits {field}")

    surface = text.partition("### 16.1 Contract surface delta")[2].partition("### 16.2")[0]
    existing_routes = (
        "procedure.validate",
        "session.start",
        "session.start_replace",
        "session.status",
        "session.next",
        "session.complete",
        "session.skip",
        "session.retry",
        "session.block",
        "session.unblock",
        "session.cancel",
        "session.reset",
        "item.check",
        "item.uncheck",
        "item.set",
        "item.add",
        "item.remove",
        "item.attach",
        "item.clear",
        "job.lookup",
        "job.status",
        "job.wait",
    )
    missing = [route for route in existing_routes if f"`{route}`" not in surface]
    if missing:
        fail(f"v2 contract surface omits existing version-aware routes: {missing}")


def validate_roadmap() -> tuple[int, int]:
    lines = ROADMAP.read_text(encoding="utf-8").splitlines()
    epic_positions = [
        (index, match.group(1))
        for index, line in enumerate(lines)
        if (match := EPIC_RE.match(line))
    ]
    if not epic_positions:
        fail("active roadmap contains no epic")

    task_count = 0
    seen_epics: set[str] = set()
    for epic_index, (start, epic_id) in enumerate(epic_positions):
        if epic_id in seen_epics:
            fail(f"active roadmap repeats epic: {epic_id}")
        seen_epics.add(epic_id)
        end = epic_positions[epic_index + 1][0] if epic_index + 1 < len(epic_positions) else len(lines)
        section = lines[start + 1 : end]
        try:
            header = section.index("| id | title | status | goal | references |")
        except ValueError:
            fail(f"{epic_id} omits the required roadmap table header")
        if header + 1 >= len(section) or section[header + 1] != "|---|---|---|---|---|":
            fail(f"{epic_id} has an invalid roadmap table separator")
        rows = [line for line in section[header + 2 :] if line.startswith("|")]
        if not rows:
            fail(f"{epic_id} has no roadmap tasks")
        expected_separator = "" if epic_id in LEGACY_COMPACT_TASK_EPICS else "-"
        statuses: list[tuple[str, str]] = []
        for expected_number, row in enumerate(rows, start=1):
            match = TASK_RE.fullmatch(row)
            if match is None:
                fail(f"malformed roadmap row in {epic_id}: {row}")
            row_epic, separator, suffix, _, status, _, references = match.groups()
            task_id = f"{row_epic}{separator}{suffix}"
            if separator != expected_separator:
                fail(f"invalid roadmap task separator in {epic_id}: {task_id}")
            if row_epic != epic_id or suffix != f"{expected_number:03d}":
                fail(f"non-sequential roadmap task in {epic_id}: {task_id}")
            if LINK_RE.search(references) is None:
                fail(f"roadmap task has no documentation reference: {task_id}")
            statuses.append((task_id, status))
            task_count += 1

        first_incomplete = next(
            (index for index, (_, status) in enumerate(statuses) if status != "Completed"),
            len(statuses),
        )
        active = statuses[first_incomplete:]
        if active:
            first_id, first_status = active[0]
            if first_status not in {"Planned", "In Progress", "In Review", "Deferred", "Blocked"}:
                fail(f"roadmap has an invalid first incomplete state: {first_id}={first_status}")
            for task_id, status in active[1:]:
                if status != "Planned":
                    fail(f"tasks after the first incomplete task must be Planned: {task_id}={status}")
    return len(epic_positions), task_count


def main() -> int:
    try:
        if (ROOT / "sot").exists():
            fail("legacy sot directory still exists")
        validate_layout()
        validate_v2_dossier()
        files = markdown_files()
        validate_english(files)
        links = validate_links(files)
        epics, tasks = validate_roadmap()
    except (DocumentationError, OSError, UnicodeError) as error:
        print(f"documentation verification failed: {error}", file=sys.stderr)
        return 1
    print(
        f"documentation verified: {len(files)} Markdown files, {links} links, "
        f"{epics} active epics, {tasks} active tasks"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
