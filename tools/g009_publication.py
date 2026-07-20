#!/usr/bin/env python3
"""Fail-closed, transport-independent G009 GitHub release publication controller.

The immutable G009 tag is controller-owned.  GitHub exposes no conditional asset
upload; uploads are therefore preceded and followed by exact snapshots.  The
controller never deletes, overwrites, or claims atomic publication.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol


class PublicationError(RuntimeError):
    pass


class CreateRace(PublicationError):
    pass


@dataclass(frozen=True)
class Asset:
    id: int
    name: str
    size: int
    digest: str


@dataclass(frozen=True)
class Release:
    id: int
    tag: str
    target: str
    title: str
    draft: bool
    prerelease: bool
    immutable: bool
    make_latest: bool | None
    assets: tuple[Asset, ...]


class Transport(Protocol):
    def snapshot(self, tag: str) -> Release | None: ...
    def read_asset(self, asset: Asset) -> bytes: ...
    def create_draft(self, tag: str, target: str) -> None: ...
    def upload(self, release: Release, name: str, data: bytes) -> None: ...
    def publish(self, release: Release) -> None: ...


def _digest(data: bytes) -> str:
    return f"sha256:{hashlib.sha256(data).hexdigest()}"
_PUBLIC_RECORDS = ("publication-handoff.json", "RELEASE_NOTES.md")

def _json_asset(desired: dict[str, bytes], name: str) -> dict[str, Any]:
    try:
        value = json.loads(desired[name])
    except (KeyError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PublicationError(f"required publication record {name} is missing or malformed") from error
    if not isinstance(value, dict):
        raise PublicationError(f"required publication record {name} is not an object")
    return value

def _validate_publication_records(desired: dict[str, bytes]) -> None:
    """Bind the reviewed unsigned release inputs before any remote mutation."""
    policy = _json_asset(desired, "release-policy.json")
    evidence = policy.get("signing_evidence")
    current = {
        "posture": "unsigned-not-notarized",
        "codesign": "not_attempted_missing_credentials",
        "notarization": "not_attempted_missing_credentials",
        "stapling": "not_applicable_zip",
        "gatekeeper": "not_claimed",
        "release_notes_asset": "RELEASE_NOTES.md",
        "release_notes_must_document_status": True,
        "status_frozen_for_current_release": True,
    }
    human_steps = {
        "recommendation": "should_be_completed_when_infrastructure_allows",
        "qualification_requirement": False,
        "detached_human_release_step": True,
    }
    receipt = {
        "generated_only_after_successful_publication": True,
        "must_bind_release_tag_and_exact_pre_publication_asset_digests": True,
        "is_not_a_pre_publication_asset": True,
    }
    if not isinstance(evidence, dict) or evidence != {
        "current_public_package": current,
        "developer_id_and_notarization": human_steps,
        "publication_receipt": receipt,
    }:
        raise PublicationError("release policy does not authorize the documented unsigned publication posture")
    if not all(name in desired for name in _PUBLIC_RECORDS) or "publication-receipt.json" in desired:
        raise PublicationError("publication records or receipt boundary are incomplete")
    notes = desired["RELEASE_NOTES.md"].decode("utf-8", errors="replace")
    required_notes = (
        "unsigned and not notarized",
        "Developer ID signing and notarization were not attempted",
        "detached human release steps outside qualification",
    )
    if not all(statement in notes for statement in required_notes):
        raise PublicationError("release notes do not document the unsigned-not-notarized status")
    handoff = _json_asset(desired, "publication-handoff.json")
    archive = handoff.get("product_archive")
    source, target = handoff.get("source"), handoff.get("target_tuple")
    if (
        handoff.get("schema") != "podway.g009.publication-handoff/v1"
        or not isinstance(source, dict)
        or set(source) != {"commit", "tree", "tools"}
        or not isinstance(archive, str)
        or handoff.get("product_archive_sha256") != _digest(desired.get(archive, b"")).removeprefix("sha256:")
        or handoff.get("release_policy_sha256") != _digest(desired["release-policy.json"]).removeprefix("sha256:")
        or not isinstance(handoff.get("controller_manifest_sha256"), str)
        or target != {"triple": "aarch64-apple-darwin", "arch": "arm64", "host_arch": "arm64", "mach_o_arch": "arm64"}
    ):
        raise PublicationError("review handoff is not bound to the archive, source, policy, and controller")


def _publication_receipt(tag: str, target: str, desired: dict[str, bytes]) -> bytes:
    """Create, but never predeclare, the receipt after an exact public snapshot."""
    return (json.dumps({
        "schema": "podway.g009.publication-receipt/v1", "tag": tag, "target_commit": target,
        "pre_publication_assets": {name: _digest(data).removeprefix("sha256:") for name, data in sorted(desired.items())},
    }, separators=(",", ":"), sort_keys=True) + "\n").encode()


def _valid_asset_name(name: str) -> bool:
    return bool(name) and "/" not in name and "\\" not in name


def _validate_snapshot(release: Release, tag: str, target: str) -> None:
    if (
        not isinstance(release.id, int) or release.id <= 0
        or release.tag != tag or release.target != target
        or release.title != f"Podway Apple Silicon {tag}"
        or not isinstance(release.draft, bool) or release.prerelease is not False
        or release.immutable is not False or release.make_latest not in (False, None)
    ):
        raise PublicationError("release identity or policy-controlled metadata differs")
    ids: set[int] = set()
    names: set[str] = set()
    for asset in release.assets:
        if (
            not isinstance(asset.id, int) or asset.id <= 0
            or not _valid_asset_name(asset.name) or asset.name in names or asset.id in ids
            or not isinstance(asset.size, int) or asset.size < 0
            or not re.fullmatch(r"sha256:[0-9a-f]{64}", asset.digest)
        ):
            raise PublicationError("release asset listing is malformed")
        ids.add(asset.id)
        names.add(asset.name)

def _release_identity(release: Release) -> tuple[object, ...]:
    """Compare only metadata GitHub exposes in GET release snapshots."""
    return (
        release.id,
        release.tag,
        release.target,
        release.title,
        release.prerelease,
        release.immutable,
    )


def _require_same_release(left: Release, right: Release) -> None:
    if _release_identity(left) != _release_identity(right):
        raise PublicationError("release identity changed across publication mutation boundary")



def _verify_assets(transport: Transport, release: Release, desired: dict[str, bytes], *, complete: bool) -> dict[str, Asset]:
    remote = {asset.name: asset for asset in release.assets}
    if set(remote) - set(desired):
        raise PublicationError("release contains unexpected asset")
    if complete and set(remote) != set(desired):
        raise PublicationError("published release is incomplete or differs from verified handoff")
    for name, asset in remote.items():
        data = transport.read_asset(asset)
        if len(data) != asset.size or _digest(data) != asset.digest or data != desired[name]:
            raise PublicationError("remote release asset differs from verified handoff")
    return remote


def _fresh_verified_snapshot(transport: Transport, tag: str, target: str, desired: dict[str, bytes], *, complete: bool) -> Release:
    """Verify bytes and an unchanged exhaustive snapshot before a mutation."""
    snapshot = transport.snapshot(tag)
    if snapshot is None:
        raise PublicationError("release disappeared during publication")
    _validate_snapshot(snapshot, tag, target)
    _verify_assets(transport, snapshot, desired, complete=complete)
    fresh = transport.snapshot(tag)
    if fresh is None:
        raise PublicationError("release disappeared during publication")
    _validate_snapshot(fresh, tag, target)
    if snapshot != fresh:
        raise PublicationError("release changed during remote asset verification")
    return fresh


def publish_release(transport: Transport, tag: str, target: str, desired: dict[str, bytes], *, require_public_records: bool = True) -> None:
    """Repair only a stable draft; accept only a stable exact published release."""
    if not re.fullmatch(r"g009-[0-9a-f]{64}", tag) or not re.fullmatch(r"[0-9a-f]{40}", target):
        raise PublicationError("release tag or candidate commit is malformed")
    if not desired or any(not _valid_asset_name(name) for name in desired):
        raise PublicationError("declared release asset set is malformed")
    if require_public_records:
        _validate_publication_records(desired)

    release = transport.snapshot(tag)
    if release is None:
        try:
            transport.create_draft(tag, target)
        except CreateRace:
            pass
        release = transport.snapshot(tag)
        if release is None:
            raise PublicationError("release creation did not produce a release")
    _validate_snapshot(release, tag, target)
    if not release.draft:
        verified = _fresh_verified_snapshot(transport, tag, target, desired, complete=True)
        _require_same_release(release, verified)
        return

    for name, data in desired.items():
        previous = release
        release = _fresh_verified_snapshot(transport, tag, target, desired, complete=False)
        _require_same_release(previous, release)
        if not release.draft:
            _fresh_verified_snapshot(transport, tag, target, desired, complete=True)
            return
        if name not in {asset.name for asset in release.assets}:
            transport.upload(release, name, data)
            uploaded = _fresh_verified_snapshot(transport, tag, target, desired, complete=False)
            _require_same_release(release, uploaded)
            if not uploaded.draft:
                raise PublicationError("release became public during asset upload")
            release = uploaded

    publication = _fresh_verified_snapshot(transport, tag, target, desired, complete=True)
    _require_same_release(release, publication)
    if not publication.draft:
        _fresh_verified_snapshot(transport, tag, target, desired, complete=True)
        return
    transport.publish(publication)
    final = _fresh_verified_snapshot(transport, tag, target, desired, complete=True)
    _require_same_release(publication, final)
    if final.draft:
        raise PublicationError("release publication binding failed")


class GitHubTransport:
    """The sole GitHub REST adapter; controller decisions never construct HTTP requests."""

    def __init__(self, api: str, repo: str, token: str):
        self.api, self.repo = api.rstrip("/"), repo
        self.headers = {"Accept": "application/vnd.github+json", "Authorization": f"Bearer {token}", "X-GitHub-Api-Version": "2022-11-28"}
        self._asset_urls: dict[int, str] = {}
        self._release_urls: dict[int, tuple[str, str]] = {}

    def _json(self, url: str, *, data: bytes | None = None, method: str | None = None) -> object:
        headers = self.headers if data is None else {**self.headers, "Content-Type": "application/json"}
        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)

    def snapshot(self, tag: str) -> Release | None:
        try:
            raw = self._json(f"{self.api}/repos/{self.repo}/releases/tags/{urllib.parse.quote(tag, safe='')}")
        except urllib.error.HTTPError as error:
            if error.code == 404:
                return None
            raise
        if not isinstance(raw, dict):
            raise PublicationError("release response is malformed")
        release_id, assets_url, upload_url, release_url = raw.get("id"), raw.get("assets_url"), raw.get("upload_url"), raw.get("url")
        fields = (raw.get("tag_name"), raw.get("target_commitish"), raw.get("name"), raw.get("draft"), raw.get("prerelease"), raw.get("immutable"))
        if not isinstance(release_id, int) or not all(isinstance(value, str) for value in (assets_url, upload_url, release_url)) or not isinstance(fields[0], str) or not isinstance(fields[1], str) or not isinstance(fields[2], str) or not all(isinstance(value, bool) for value in fields[3:]):
            raise PublicationError("release response is malformed")
        # GitHub's GET release representation currently omits make_latest.  Accept
        # only False when a compatible endpoint exposes it; creation/PATCH force false.
        make_latest = raw.get("make_latest")
        if make_latest is not None and not isinstance(make_latest, bool):
            raise PublicationError("release make_latest response is malformed")
        assets: list[Asset] = []
        page = 1
        while True:
            page_raw = self._json(f"{assets_url}?per_page=100&page={page}")
            if not isinstance(page_raw, list):
                raise PublicationError("release asset page is malformed")
            for item in page_raw:
                if not isinstance(item, dict) or not isinstance(item.get("id"), int) or not isinstance(item.get("name"), str) or not isinstance(item.get("size"), int) or not isinstance(item.get("digest"), str) or not isinstance(item.get("url"), str):
                    raise PublicationError("release asset listing is malformed")
                self._asset_urls[item["id"]] = item["url"]
                assets.append(Asset(item["id"], item["name"], item["size"], item["digest"]))
            if len(page_raw) < 100:
                break
            page += 1
        self._release_urls[release_id] = (release_url, upload_url.split("{", 1)[0])
        return Release(release_id, fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], make_latest, tuple(assets))

    def read_asset(self, asset: Asset) -> bytes:
        url = self._asset_urls.get(asset.id)
        if url is None:
            raise PublicationError("remote release asset locator is malformed")
        with urllib.request.urlopen(urllib.request.Request(url, headers={**self.headers, "Accept": "application/octet-stream"}), timeout=120) as response:
            return response.read()

    def create_draft(self, tag: str, target: str) -> None:
        body = json.dumps({"tag_name": tag, "target_commitish": target, "name": f"Podway Apple Silicon {tag}", "draft": True, "prerelease": False, "make_latest": "false"}, separators=(",", ":")).encode()
        try:
            self._json(f"{self.api}/repos/{self.repo}/releases", data=body, method="POST")
        except urllib.error.HTTPError as error:
            if error.code == 422:
                raise CreateRace() from error
            raise

    def upload(self, release: Release, name: str, data: bytes) -> None:
        upload = self._release_urls.get(release.id, (None, None))[1]
        if upload is None:
            raise PublicationError("release upload locator is malformed")
        request = urllib.request.Request(f"{upload}?{urllib.parse.urlencode({'name': name})}", data=data, headers={**self.headers, "Content-Type": "application/octet-stream"}, method="POST")
        with urllib.request.urlopen(request, timeout=120) as response:
            if response.status != 201:
                raise PublicationError("release asset upload failed")

    def publish(self, release: Release) -> None:
        url = self._release_urls.get(release.id, (None, None))[0]
        if url is None:
            raise PublicationError("release publication locator is malformed")
        self._json(url, data=b'{"draft":false,"prerelease":false,"make_latest":"false"}', method="PATCH")


def self_test() -> None:
    """Table-driven deterministic production-controller transition/failure matrix."""
    tag, target = "g009-" + "a" * 64, "b" * 40
    wanted = {"a.json": b"a", "b.zip": b"bb"}

    def record_bytes(value: dict[str, Any]) -> bytes:
        return json.dumps(value, separators=(",", ":"), sort_keys=True).encode()

    policy = {
        "signing_evidence": {
            "current_public_package": {
                "posture": "unsigned-not-notarized",
                "codesign": "not_attempted_missing_credentials",
                "notarization": "not_attempted_missing_credentials",
                "stapling": "not_applicable_zip",
                "gatekeeper": "not_claimed",
                "release_notes_asset": "RELEASE_NOTES.md",
                "release_notes_must_document_status": True,
                "status_frozen_for_current_release": True,
            },
            "developer_id_and_notarization": {
                "recommendation": "should_be_completed_when_infrastructure_allows",
                "qualification_requirement": False,
                "detached_human_release_step": True,
            },
            "publication_receipt": {
                "generated_only_after_successful_publication": True,
                "must_bind_release_tag_and_exact_pre_publication_asset_digests": True,
                "is_not_a_pre_publication_asset": True,
            },
        },
    }
    desired = {
        "podway.zip": b"archive",
        "RELEASE_NOTES.md": (
            b"The package is unsigned and not notarized. "
            b"Developer ID signing and notarization were not attempted. "
            b"They are detached human release steps outside qualification."
        ),
        "release-policy.json": record_bytes(policy),
    }
    desired["publication-handoff.json"] = record_bytes({
        "schema": "podway.g009.publication-handoff/v1",
        "source": {"commit": "a" * 40, "tree": "b" * 40, "tools": []},
        "target_tuple": {"triple": "aarch64-apple-darwin", "arch": "arm64", "host_arch": "arm64", "mach_o_arch": "arm64"},
        "product_archive": "podway.zip",
        "product_archive_sha256": _digest(desired["podway.zip"]).removeprefix("sha256:"),
        "release_policy_sha256": _digest(desired["release-policy.json"]).removeprefix("sha256:"),
        "controller_manifest_sha256": "c" * 64,
    })
    _validate_publication_records(desired)

    def reject(label: str, records: dict[str, bytes]) -> None:
        try:
            _validate_publication_records(records)
        except PublicationError:
            return
        raise AssertionError(f"{label} was accepted")

    missing_status = dict(desired)
    missing_status["RELEASE_NOTES.md"] = b"release notes"
    reject("undocumented unsigned status", missing_status)
    tampered_archive = dict(desired)
    tampered_archive["podway.zip"] = b"wrong-content"
    reject("archive binding", tampered_archive)
    tampered_policy = dict(desired)
    tampered_policy["release-policy.json"] = b"{}"
    reject("policy posture", tampered_policy)
    tampered_handoff = dict(desired)
    handoff = json.loads(tampered_handoff["publication-handoff.json"])
    handoff["controller_manifest_sha256"] = 1
    tampered_handoff["publication-handoff.json"] = record_bytes(handoff)
    reject("controller binding", tampered_handoff)
    reject("receipt pre-publication asset", {**desired, "publication-receipt.json": b"{}"})
    reject("missing publication records", {})

    class Fake:
        def __init__(self, release: Release | None, data: dict[str, bytes], *, race: bool = False, fail: str | None = None, after: str | None = None, drift_at: int | None = None, final_draft: bool = False, disappear_at: int | None = None, publish_at: int | None = None):
            self.release, self.data = release, dict(data)
            self.race, self.fail, self.after, self.drift_at = race, fail, after, drift_at
            self.final_draft, self.disappear_at, self.publish_at = final_draft, disappear_at, publish_at
            self.log: list[str] = []; self.snapshots = 0
        def snapshot(self, _: str) -> Release | None:
            self.snapshots += 1
            if self.fail == "snapshot": raise OSError("snapshot")
            if self.disappear_at == self.snapshots:
                self.release = None
            if self.publish_at == self.snapshots:
                self.release = make(tuple(wanted), False)
            if self.drift_at == self.snapshots and self.release:
                self.release = Release(*self.release.__dict__.values())
                self.release = Release(self.release.id + 9, self.release.tag, self.release.target, self.release.title, self.release.draft, self.release.prerelease, self.release.immutable, self.release.make_latest, self.release.assets)
            return self.release
        def read_asset(self, asset: Asset) -> bytes:
            if self.fail == "read": raise OSError("read")
            return self.data[asset.name]
        def create_draft(self, actual_tag: str, actual_target: str) -> None:
            if self.fail == "create": raise OSError("create")
            self.log.append("create"); self.release = make((), True, actual_tag, actual_target)
            if self.after == "create": raise OSError("create-after")
            if self.race: raise CreateRace()
        def upload(self, release: Release, name: str, data: bytes) -> None:
            if self.fail == "upload": raise OSError("upload")
            self.log.append(f"upload:{name}"); self.data[name] = data
            self.release = make(tuple((*[a.name for a in release.assets], name)), True)
            if self.after == "upload": raise OSError("upload-after")
        def publish(self, release: Release) -> None:
            if self.fail == "publish": raise OSError("publish")
            self.log.append("publish"); self.release = make(tuple(a.name for a in release.assets), self.final_draft)
            if self.after == "publish": raise OSError("publish-after")

    def make(names: tuple[str, ...], draft: bool, actual_tag: str = tag, actual_target: str = target, **metadata: object) -> Release:
        data = {name: wanted.get(name, b"x") for name in names}
        return Release(1, actual_tag, actual_target, str(metadata.get("title", f"Podway Apple Silicon {actual_tag}")), draft, bool(metadata.get("prerelease", False)), bool(metadata.get("immutable", False)), metadata.get("make_latest", False), tuple(Asset(i + 1, name, len(data[name]), _digest(data[name])) for i, name in enumerate(names)))

    cases = [
        ("absent", None, {}, ["create", "upload:a.json", "upload:b.zip", "publish"], None),
        ("race", None, {}, ["create", "upload:a.json", "upload:b.zip", "publish"], "race"),
        ("partial", make(("a.json",), True), {"a.json": b"a"}, ["upload:b.zip", "publish"], None),
        ("complete-draft", make(tuple(wanted), True), wanted, ["publish"], None),
        ("published-exact", make(tuple(wanted), False), wanted, [], None),
    ]
    for label, release, data, expected, option in cases:
        fake = Fake(release, data, race=option == "race"); publish_release(fake, tag, target, wanted, require_public_records=False)
        if fake.log != expected: raise AssertionError(f"{label} transition differs: {fake.log}")
    try:
        publish_release(Fake(None, {}, disappear_at=2), tag, target, wanted, require_public_records=False)
    except PublicationError:
        pass
    else:
        raise AssertionError("release disappearance after creation accepted")
    for label, publish_at in (("upload-loop-convergence", 2), ("pre-publish-convergence", 6)):
        fake = Fake(make(tuple(wanted), True), wanted, publish_at=publish_at)
        publish_release(fake, tag, target, wanted, require_public_records=False)
        if fake.log:
            raise AssertionError(f"{label} mutated an already published exact release")
    try:
        publish_release(Fake(make(tuple(wanted), True), wanted, final_draft=True), tag, target, wanted, require_public_records=False)
    except PublicationError:
        pass
    else:
        raise AssertionError("final draft state accepted")
    invalid = [
        make(("a.json",), False), make(("a.json", "extra"), True), make(tuple(wanted), False, title="wrong"), make(tuple(wanted), False, prerelease=True), make(tuple(wanted), False, immutable=True), make(tuple(wanted), False, make_latest=True), make(tuple(wanted), True, actual_tag="g009-" + "c" * 64), make(tuple(wanted), True, actual_target="c" * 40),
    ]
    duplicate = make(("a.json",), True)
    invalid.extend((
        Release(duplicate.id, duplicate.tag, duplicate.target, duplicate.title, duplicate.draft, duplicate.prerelease, duplicate.immutable, duplicate.make_latest, (duplicate.assets[0], duplicate.assets[0])),
        Release(duplicate.id, duplicate.tag, duplicate.target, duplicate.title, duplicate.draft, duplicate.prerelease, duplicate.immutable, duplicate.make_latest, (duplicate.assets[0], Asset(duplicate.assets[0].id + 1, duplicate.assets[0].name, duplicate.assets[0].size, duplicate.assets[0].digest))),
        Release(duplicate.id, duplicate.tag, duplicate.target, duplicate.title, duplicate.draft, duplicate.prerelease, duplicate.immutable, duplicate.make_latest, (duplicate.assets[0], Asset(duplicate.assets[0].id, "other.zip", duplicate.assets[0].size, duplicate.assets[0].digest))),
    ))
    for release in invalid:
        fake = Fake(release, wanted)
        try: publish_release(fake, tag, target, wanted, require_public_records=False)
        except PublicationError: pass
        else: raise AssertionError("invalid release accepted")
    failure_states = {
        "snapshot": (make(tuple(wanted), True), wanted),
        "read": (make(tuple(wanted), True), wanted),
        "create": (None, {}),
        "upload": (make(("a.json",), True), {"a.json": b"a"}),
        "publish": (make(tuple(wanted), True), wanted),
    }
    for failure, (release, data) in failure_states.items():
        try: publish_release(Fake(release, data, fail=failure), tag, target, wanted, require_public_records=False)
        except OSError: pass
        else: raise AssertionError(f"{failure} exception suppressed")
    for after in ("create", "upload", "publish"):
        fake = Fake(None, {}, after=after)
        try: publish_release(fake, tag, target, wanted, require_public_records=False)
        except OSError: pass
        else: raise AssertionError(f"{after} post-side-effect exception suppressed")
        fake.after = None; publish_release(fake, tag, target, wanted, require_public_records=False)
    for point in range(1, 20):
        fake = Fake(make(("a.json",), True), {"a.json": b"a"}, drift_at=point)
        rejected = False
        try:
            publish_release(fake, tag, target, wanted, require_public_records=False)
        except PublicationError:
            rejected = True
        if rejected != (point <= 11):
            raise AssertionError(f"snapshot drift point {point} classification differs")
    # Real adapter pagination is covered without credentials.
    adapter = GitHubTransport("https://api.example", "owner/repo", "token")
    pages = [[{"id": i + 1, "name": f"p{i}", "size": 0, "digest": _digest(b""), "url": f"https://asset/{i}"} for i in range(100)], [{"id": 101, "name": "p100", "size": 0, "digest": _digest(b""), "url": "https://asset/100"}]]
    adapter._json = lambda url, **_: {"id": 1, "tag_name": tag, "target_commitish": target, "name": f"Podway Apple Silicon {tag}", "draft": True, "prerelease": False, "immutable": False, "assets_url": "https://assets", "upload_url": "https://upload{?name}", "url": "https://release"} if "/tags/" in url else pages[int(url.rsplit("=", 1)[1]) - 1]  # type: ignore[method-assign]
    if len(adapter.snapshot(tag).assets) != 101: raise AssertionError("pagination was not exhaustive")


def main() -> int:
    names = os.environ["RELEASE_ASSETS"].split(",")
    desired = {name: (Path("release-assets") / name).read_bytes() for name in names}
    tag, target = os.environ["RELEASE_TAG"], os.environ["CANDIDATE_COMMIT"]
    publish_release(GitHubTransport(os.environ["GITHUB_API_URL"], os.environ["GITHUB_REPOSITORY"], os.environ["RELEASE_TOKEN"]), tag, target, desired)
    Path("publication-receipt.json").write_bytes(_publication_receipt(tag, target, desired))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
