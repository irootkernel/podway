#!/usr/bin/env python3
"""Validate Podway's English documentation, local links, and active roadmap."""

from __future__ import annotations

from pathlib import Path
import re
import shutil
import sys
import tempfile
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"
SKILLS = ROOT / "skills"
ROADMAP = DOCS / "roadmap/README.md"
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
EXPECTED_SKILL_FILES = {
    "create-podway-procedure": {
        "SKILL.md",
        "agents/openai.yaml",
        "references/authoring.md",
        "references/distribution.md",
    },
    "use-podway": {
        "SKILL.md",
        "references/goal.md",
        "references/lifecycle.md",
        "references/recovery.md",
    },
}


class DocumentationError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise DocumentationError(message)


def is_hidden_path(path: Path) -> bool:
    return any(part.startswith(".") for part in path.parts)


def skill_markdown_files(skills_root: Path = SKILLS) -> list[Path]:
    return sorted(
        path
        for name in EXPECTED_SKILL_FILES
        for path in (skills_root / name).rglob("*.md")
        if not is_hidden_path(path.relative_to(skills_root / name))
    )


def markdown_files() -> list[Path]:
    files = [ROOT / "README.md", ROOT / "RELEASE_NOTES.md"]
    files.extend(sorted(DOCS.rglob("*.md")))
    files.extend(skill_markdown_files())
    return files


def validate_path_components(root: Path, relative: Path) -> Path:
    current = root
    for part in relative.parts:
        current /= part
        if current.is_symlink():
            fail(f"unsafe symlink in skill path: {current}")
    return current


def validate_skill_frontmatter(name: str, entrypoint: str) -> None:
    prefix = f"---\nname: {name}\ndescription: "
    if not entrypoint.startswith(prefix):
        fail(f"invalid skill frontmatter: skills/{name}/SKILL.md")
    frontmatter, separator, _ = entrypoint.partition("\n---\n")
    description = frontmatter[len(prefix) :]
    if not separator or not description.strip() or "\n" in description:
        fail(f"invalid skill frontmatter: skills/{name}/SKILL.md")


def validate_skills(skills_root: Path = SKILLS) -> int:
    if skills_root.is_symlink() or not skills_root.is_dir():
        fail(f"unsafe or missing skills root: {skills_root}")
    actual_names = {
        path.name for path in skills_root.iterdir() if not path.name.startswith(".")
    }
    if actual_names != set(EXPECTED_SKILL_FILES):
        fail(
            "source-distributed skill set differs from the contract: "
            f"expected {sorted(EXPECTED_SKILL_FILES)}, got {sorted(actual_names)}"
        )

    for name, expected_files in EXPECTED_SKILL_FILES.items():
        root = validate_path_components(skills_root, Path(name))
        if not root.is_dir():
            fail(f"unsafe or missing skill root: {root}")
        actual_files: set[str] = set()
        for path in root.rglob("*"):
            relative = path.relative_to(root)
            if is_hidden_path(relative):
                continue
            validate_path_components(root, relative)
            if path.is_file():
                actual_files.add(relative.as_posix())
        if actual_files != expected_files:
            fail(
                f"{name} file set differs from the contract: "
                f"expected {sorted(expected_files)}, got {sorted(actual_files)}"
            )
        for relative in expected_files:
            path = validate_path_components(root, Path(relative))
            if not path.is_file():
                fail(f"unsafe or missing skill file: {path}")

        entrypoint = (root / "SKILL.md").read_text(encoding="utf-8")
        validate_skill_frontmatter(name, entrypoint)

    authoring_root = skills_root / "create-podway-procedure"
    authoring_entrypoint = (authoring_root / "SKILL.md").read_text(encoding="utf-8")
    authoring_reference = (authoring_root / "references/authoring.md").read_text(
        encoding="utf-8"
    )
    distribution_reference = (
        authoring_root / "references/distribution.md"
    ).read_text(encoding="utf-8")
    runtime_entrypoint = (skills_root / "use-podway/SKILL.md").read_text(encoding="utf-8")
    required_markers = {
        "authoring entrypoint": (
            ("podway version --json", ".podway/procedures/<procedure-id>.yaml"),
            ("WORKSPACE_CONFIG_INVALID", "$use-podway"),
            ("podway preset show sw-dev-v2", "--expect-procedure-digest"),
        ),
        "authoring reference": (
            ("podway help procedure.<operation>", "podway preset show sw-dev-v2"),
            ("WORKSPACE_CONFIG_INVALID", "configured `procedure_paths`"),
        ),
        "distribution reference": (
            ("separate runtime authorization", "$use-podway"),
            ("exact preview start suggestion", "accepted-path inventory"),
        ),
        "runtime entrypoint": (("$create-podway-procedure", "runtime skill"),),
    }
    contents = {
        "authoring entrypoint": authoring_entrypoint,
        "authoring reference": authoring_reference,
        "distribution reference": distribution_reference,
        "runtime entrypoint": runtime_entrypoint,
    }
    for label, marker_groups in required_markers.items():
        for markers in marker_groups:
            missing = [marker for marker in markers if marker not in contents[label]]
            if missing:
                fail(f"{label} omits required contract markers: {missing}")

    authoring_metadata = (
        skills_root / "create-podway-procedure/agents/openai.yaml"
    ).read_text(encoding="utf-8")
    if "$create-podway-procedure" not in authoring_metadata:
        fail("create-podway-procedure UI metadata omits its invocation name")
    return len(EXPECTED_SKILL_FILES)


def copy_skill_fixture(destination: Path) -> None:
    for name, expected_files in EXPECTED_SKILL_FILES.items():
        for relative in expected_files:
            source = SKILLS / name / relative
            target = destination / name / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)


def expect_validation_failure(skills_root: Path, expected: str) -> None:
    try:
        validate_skills(skills_root)
    except DocumentationError as error:
        if expected not in str(error):
            fail(f"self-test expected {expected!r}, got {error!r}")
        return
    fail(f"self-test expected validation failure containing {expected!r}")


def validate_skills_self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="podway-verify-docs-") as temporary:
        temporary_root = Path(temporary)
        valid = temporary_root / "valid"
        copy_skill_fixture(valid)
        validate_skills(valid)

        (valid / "create-podway-procedure/.DS_Store").write_bytes(b"host artifact")
        for directory in (".gjc", ".omc"):
            hidden_markdown = valid / "use-podway" / directory / "notes.md"
            hidden_markdown.parent.mkdir()
            hidden_markdown.write_text("hidden host note\n", encoding="utf-8")
        validate_skills(valid)
        discovered = skill_markdown_files(valid)
        if any(is_hidden_path(path.relative_to(valid)) for path in discovered):
            fail("self-test discovered hidden skill Markdown")

        visible_extra = temporary_root / "visible-extra"
        shutil.copytree(valid, visible_extra)
        (visible_extra / "use-podway/extra.txt").write_text("extra\n", encoding="utf-8")
        expect_validation_failure(visible_extra, "file set differs")

        empty_description = temporary_root / "empty-description"
        shutil.copytree(valid, empty_description)
        entrypoint = empty_description / "use-podway/SKILL.md"
        text = entrypoint.read_text(encoding="utf-8")
        text = re.sub(r"(?m)^description: .+$", "description: ", text, count=1)
        entrypoint.write_text(text, encoding="utf-8")
        expect_validation_failure(empty_description, "invalid skill frontmatter")

        late_delimiter = temporary_root / "late-delimiter"
        shutil.copytree(valid, late_delimiter)
        entrypoint = late_delimiter / "use-podway/SKILL.md"
        entrypoint.write_text(
            "---\nname: use-podway\ndescription: Valid description\n"
            "# Body before a later thematic break\n\n---\n\nMore body\n",
            encoding="utf-8",
        )
        expect_validation_failure(late_delimiter, "invalid skill frontmatter")

        symlink_root = temporary_root / "symlink-root"
        copy_skill_fixture(symlink_root)
        real_root = temporary_root / "real-use-podway"
        (symlink_root / "use-podway").rename(real_root)
        (symlink_root / "use-podway").symlink_to(real_root, target_is_directory=True)
        expect_validation_failure(symlink_root, "unsafe symlink in skill path")

        symlink_intermediate = temporary_root / "symlink-intermediate"
        copy_skill_fixture(symlink_intermediate)
        references = symlink_intermediate / "use-podway/references"
        real_references = temporary_root / "real-references"
        references.rename(real_references)
        references.symlink_to(real_references, target_is_directory=True)
        expect_validation_failure(symlink_intermediate, "unsafe symlink in skill path")


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


def main(arguments: list[str] | None = None) -> int:
    arguments = sys.argv[1:] if arguments is None else arguments
    try:
        if arguments == ["self-test"]:
            validate_skills_self_test()
            print("documentation verifier self-test passed")
            return 0
        if arguments:
            fail(f"unsupported arguments: {arguments}")
        if (ROOT / "sot").exists():
            fail("legacy sot directory still exists")
        validate_layout()
        skills = validate_skills()
        files = markdown_files()
        validate_english(files)
        links = validate_links(files)
        epics, tasks = validate_roadmap()
    except (DocumentationError, OSError, UnicodeError) as error:
        print(f"documentation verification failed: {error}", file=sys.stderr)
        return 1
    print(
        f"documentation verified: {len(files)} Markdown files, {links} links, "
        f"{epics} active epics, {tasks} active tasks, {skills} source-distributed skills"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
