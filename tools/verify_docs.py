#!/usr/bin/env python3
"""Validate Podway's English documentation, internal links, and roadmap shape."""

from __future__ import annotations

from pathlib import Path
import re
import sys
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parent.parent
ROADMAP = ROOT / "docs/roadmap.md"
EPIC_IDS = ("DESGN", "FOUND", "COREX", "STORE", "GITFS", "DAEMN", "CLINT", "MACOS", "DOGFD", "HARDN")
LINK_RE = re.compile(r"(?<!!)\[[^\]]+\]\(([^)]+)\)")
HEADING_RE = re.compile(r"^(#{1,6})\s+(.+?)\s*$")
HANGUL_RE = re.compile(r"[\u1100-\u11ff\u3130-\u318f\uac00-\ud7af]")
EPIC_RE = re.compile(r"^## ([A-Z]{5}) — (.+)$")
TASK_RE = re.compile(r"^\| `([A-Z]{5})(\d{3})` \| (.+?) \| (Completed) \| (.+?) \| (.+?) \|$")


class DocumentationError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise DocumentationError(message)


def markdown_files() -> list[Path]:
    files = [ROOT / "README.md"]
    files.extend(sorted((ROOT / "docs").rglob("*.md")))
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


def validate_roadmap() -> tuple[int, int]:
    lines = ROADMAP.read_text(encoding="utf-8").splitlines()
    epic_positions = [(index, match.group(1)) for index, line in enumerate(lines) if (match := EPIC_RE.match(line))]
    epic_ids = tuple(epic_id for _, epic_id in epic_positions)
    if epic_ids != EPIC_IDS:
        fail(f"roadmap epic order drift: expected={EPIC_IDS}, actual={epic_ids}")

    task_count = 0
    for epic_index, (start, epic_id) in enumerate(epic_positions):
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
        for expected_number, row in enumerate(rows, start=1):
            match = TASK_RE.fullmatch(row)
            if match is None:
                fail(f"malformed roadmap row in {epic_id}: {row}")
            row_epic, suffix, _, status, _, references = match.groups()
            if row_epic != epic_id or suffix != f"{expected_number:03d}":
                fail(f"non-sequential roadmap task in {epic_id}: {row_epic}{suffix}")
            if status != "Completed":
                fail(f"historical roadmap task is not Completed: {row_epic}{suffix}")
            if LINK_RE.search(references) is None:
                fail(f"roadmap task has no documentation reference: {row_epic}{suffix}")
            task_count += 1
    return len(epic_positions), task_count


def main() -> int:
    try:
        if (ROOT / "sot").exists():
            fail("legacy sot directory still exists")
        files = markdown_files()
        validate_english(files)
        links = validate_links(files)
        epics, tasks = validate_roadmap()
    except (DocumentationError, OSError, UnicodeError) as error:
        print(f"documentation verification failed: {error}", file=sys.stderr)
        return 1
    print(f"documentation verified: {len(files)} Markdown files, {links} links, {epics} epics, {tasks} tasks")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
