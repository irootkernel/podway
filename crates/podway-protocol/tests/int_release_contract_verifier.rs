use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

use podway_core::canonicalize_json_v1;
use podway_protocol::{ReleaseContractVerifierConfigV1, verify_release_contract_v1};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const TARGET: &str = "aarch64-apple-darwin";
const SOURCE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

#[derive(Clone, Copy)]
enum Layout {
    Source,
    Packaged,
}

struct ContractFixture {
    _temporary: TempDir,
    root: PathBuf,
    layout: Layout,
}

impl ContractFixture {
    fn new(layout: Layout) -> Self {
        let temporary = tempfile::tempdir().expect("contract fixture root");
        let root = temporary.path().join("contracts-root");
        fs::create_dir(&root).expect("contract fixture directory");
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest: Value = serde_json::from_slice(
            &fs::read(source_root.join("contracts/contract-manifest-v2.json"))
                .expect("source contract manifest"),
        )
        .expect("source manifest JSON");
        for asset in manifest["assets"].as_array().expect("manifest assets") {
            let logical = asset["path"].as_str().expect("logical asset path");
            let source = source_physical_path(&source_root, logical);
            let destination = physical_path(&root, layout, logical);
            fs::create_dir_all(destination.parent().expect("asset parent"))
                .expect("fixture asset parent");
            fs::copy(source, destination).expect("copy fixture contract asset");
        }
        let manifest_path = root.join("contracts/contract-manifest-v2.json");
        fs::create_dir_all(manifest_path.parent().expect("manifest parent"))
            .expect("fixture manifest parent");
        fs::copy(
            source_root.join("contracts/contract-manifest-v2.json"),
            manifest_path,
        )
        .expect("copy fixture manifest");
        Self {
            _temporary: temporary,
            root,
            layout,
        }
    }

    fn manifest(&self) -> Value {
        serde_json::from_slice(
            &fs::read(self.root.join("contracts/contract-manifest-v2.json"))
                .expect("fixture manifest"),
        )
        .expect("fixture manifest JSON")
    }

    fn write_manifest(&self, mut manifest: Value) {
        refresh_manifest_digest(&mut manifest);
        fs::write(
            self.root.join("contracts/contract-manifest-v2.json"),
            serde_json::to_vec(&manifest).expect("serialize fixture manifest"),
        )
        .expect("write fixture manifest");
    }

    fn refresh_asset(&self, logical: &str) {
        let mut manifest = self.manifest();
        let bytes = fs::read(physical_path(&self.root, self.layout, logical))
            .expect("changed fixture asset");
        let asset = manifest["assets"]
            .as_array_mut()
            .expect("manifest assets")
            .iter_mut()
            .find(|asset| asset["path"] == logical)
            .expect("changed asset is manifest-bound");
        asset["sha256"] = json!(sha256_identity(&bytes));
        self.write_manifest(manifest);
    }

    fn valid_envelope(&self) -> Value {
        let manifest = self.manifest();
        json!({
            "schema": "podway.output/v4",
            "request_id": "123e4567-e89b-42d3-a456-426614174000",
            "command": "version",
            "generated_at": "2026-08-03T00:00:00.000Z",
            "result": {
                "schema": "podway.version-result/v2",
                "product": "podway",
                "version": format!("v{}", manifest["product_version"].as_str().unwrap()),
                "target": TARGET,
                "build_identity": release_build_identity(&manifest),
                "source_commit": SOURCE_COMMIT,
                "contract_manifest_schema": manifest["schema_version"],
                "contract_manifest_digest": manifest["digest"],
            },
            "warnings": [],
        })
    }

    fn verify_document(
        &self,
        document: &Value,
        expected_source_commit: &str,
    ) -> Result<Value, String> {
        let cli_path = self.write_probe("podway", document);
        let config = ReleaseContractVerifierConfigV1::new(
            &self.root,
            cli_path,
            TARGET,
            expected_source_commit,
        );
        verify_release_contract_v1(&config)
            .map(|receipt| serde_json::to_value(receipt).expect("receipt serializes"))
            .map_err(|error| error.to_string())
    }

    fn verify_valid(&self) -> Result<Value, String> {
        let envelope = self.valid_envelope();
        self.verify_document(&envelope, SOURCE_COMMIT)
    }

    fn write_probe(&self, name: &str, document: &Value) -> PathBuf {
        let path = self.root.join(format!("{name}-probe"));
        let body = serde_json::to_string(document).expect("probe document serializes");
        fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' '{body}'\n"))
            .expect("write probe script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("make probe executable");
        path
    }
}

fn source_physical_path(root: &Path, logical: &str) -> PathBuf {
    if let Some(suffix) = logical.strip_prefix("schemas/") {
        root.join("assets/schemas").join(suffix)
    } else if let Some(suffix) = logical.strip_prefix("presets/") {
        root.join("assets/presets").join(suffix)
    } else if let Some(suffix) = logical.strip_prefix("spec/") {
        root.join("assets/specifications").join(suffix)
    } else {
        root.join(logical)
    }
}

fn physical_path(root: &Path, layout: Layout, logical: &str) -> PathBuf {
    match layout {
        Layout::Source => source_physical_path(root, logical),
        Layout::Packaged => root.join(logical),
    }
}

fn sha256_identity(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn release_build_identity(manifest: &Value) -> String {
    let preimage = json!({
        "contract_manifest_digest": manifest["digest"],
        "product": manifest["product"],
        "profile": "release",
        "source_commit": SOURCE_COMMIT,
        "target": TARGET,
        "version": manifest["product_version"],
    });
    let canonical = canonicalize_json_v1(&preimage).expect("canonical release preimage");
    sha256_identity(canonical.as_bytes())
}

fn refresh_manifest_digest(manifest: &mut Value) {
    manifest
        .as_object_mut()
        .expect("manifest object")
        .remove("digest");
    let canonical = canonicalize_json_v1(manifest).expect("canonical manifest");
    manifest["digest"] = json!(sha256_identity(canonical.as_bytes()));
}

#[test]
fn source_and_packaged_roots_share_one_authoritative_verifier() {
    for layout in [Layout::Source, Layout::Packaged] {
        let fixture = ContractFixture::new(layout);
        let receipt = fixture
            .verify_valid()
            .unwrap_or_else(|error| panic!("valid contract root failed: {error}"));
        assert_eq!(receipt["schema"], "podway.contract-verification/v2");
        assert_eq!(receipt["binary"], "podway");
        assert_eq!(
            receipt["contract_manifest_schema"],
            "podway.contract-manifest/v2"
        );
        assert_eq!(
            receipt["build_identity"],
            release_build_identity(&fixture.manifest())
        );
        assert!(receipt.get("supported_ipc_ids").is_none());
        assert!(receipt.get("binary_results_equal").is_none());
    }

    let packaged_path_reference = ContractFixture::new(Layout::Packaged);
    let schema_path = physical_path(
        &packaged_path_reference.root,
        packaged_path_reference.layout,
        "schemas/version-summary-v1.schema.json",
    );
    let mut schema: Value = serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    schema["$defs"] = json!({
        "manifest_registered_path": {
            "$ref": "podway:///schemas/version-result-v2.schema.json"
        }
    });
    fs::write(&schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
    packaged_path_reference.refresh_asset("schemas/version-summary-v1.schema.json");
    packaged_path_reference
        .verify_valid()
        .unwrap_or_else(|error| panic!("registered packaged path reference failed: {error}"));
}

#[test]
fn source_and_packaged_contract_roots_reject_v2_preset_byte_drift() {
    for layout in [Layout::Source, Layout::Packaged] {
        for logical in [
            "presets/analysis-v2.yaml",
            "presets/bug-fix-v2.yaml",
            "presets/small-change-v2.yaml",
            "presets/sw-dev-v2.yaml",
        ] {
            let fixture = ContractFixture::new(layout);
            let path = physical_path(&fixture.root, fixture.layout, logical);
            let mut bytes = fs::read(&path).expect("preset fixture bytes");
            bytes.extend_from_slice(b"# drift\n");
            fs::write(path, bytes).expect("mutate preset fixture");
            assert!(
                fixture.verify_valid().is_err(),
                "{logical} drift must fail in source and packaged layouts"
            );
        }
    }
}

#[test]
fn complete_identity_validation_rejects_generated_drift() {
    let fixture = ContractFixture::new(Layout::Source);
    let valid = fixture.valid_envelope();
    let mut mutations = Vec::new();
    let required_fields = [
        "schema",
        "product",
        "version",
        "target",
        "build_identity",
        "source_commit",
        "contract_manifest_schema",
        "contract_manifest_digest",
    ];
    for field in required_fields {
        let mut missing = valid.clone();
        missing["result"]
            .as_object_mut()
            .expect("result object")
            .remove(field);
        mutations.push((format!("missing {field}"), missing));
    }
    let mut wrong_result_schema = valid.clone();
    wrong_result_schema["result"]["schema"] = json!("podway.version-result/v1");
    mutations.push(("wrong result discriminator".to_owned(), wrong_result_schema));
    let mut unprefixed_version = valid.clone();
    unprefixed_version["result"]["version"] = fixture.manifest()["product_version"].clone();
    mutations.push(("unprefixed version".to_owned(), unprefixed_version));
    let mut retired_ipc = valid.clone();
    retired_ipc["result"]["supported_ipc_ids"] = json!(["podway.ipc/v1"]);
    mutations.push(("retired IPC field".to_owned(), retired_ipc));
    let mut legacy_manifest = valid.clone();
    legacy_manifest["result"]["contract_manifest_schema"] = json!("podway.contract-manifest/v1");
    mutations.push(("legacy manifest generation".to_owned(), legacy_manifest));
    let mut unknown_result = valid.clone();
    unknown_result["result"]["unknown"] = json!(true);
    mutations.push(("unknown result field".to_owned(), unknown_result));
    let mut wrong_outer = valid.clone();
    wrong_outer["schema"] = json!("podway.output/v3");
    mutations.push(("wrong outer discriminator".to_owned(), wrong_outer));
    let mut wrong_command = valid.clone();
    wrong_command["command"] = json!("daemon.status");
    mutations.push(("wrong command".to_owned(), wrong_command));
    let mut manifest_drift = valid.clone();
    manifest_drift["result"]["contract_manifest_digest"] =
        json!(format!("sha256:{}", "b".repeat(64)));
    mutations.push(("manifest identity drift".to_owned(), manifest_drift));

    for field in ["product", "target", "source_commit"] {
        let mut drift = valid.clone();
        drift["result"][field] = json!("unexpected");
        mutations.push((format!("identity {field} drift"), drift));
    }
    let mut missing_commit = valid.clone();
    missing_commit["result"]["source_commit"] = Value::Null;
    mutations.push(("null release source commit".to_owned(), missing_commit));
    let mut workspace_state = valid.clone();
    workspace_state["workspace"] = json!({"uuid":"123e4567-e89b-42d3-a456-426614174000","root":"/tmp/worktree","revision_before":1,"revision_after":1});
    mutations.push((
        "worktree state in version identity".to_owned(),
        workspace_state,
    ));
    for (label, mutation) in mutations {
        assert!(
            fixture.verify_document(&mutation, SOURCE_COMMIT).is_err(),
            "verifier accepted {label}"
        );
    }

    let mut build_drift = valid.clone();
    build_drift["result"]["build_identity"] = json!(format!("sha256:{}", "b".repeat(64)));
    assert!(
        fixture
            .verify_document(&build_drift, SOURCE_COMMIT)
            .is_err(),
        "verifier accepted build preimage identity drift"
    );
    let manifest = fixture.manifest();
    let debug_preimage = json!({
        "contract_manifest_digest": manifest["digest"],
        "product": manifest["product"],
        "profile": "debug",
        "source_commit": SOURCE_COMMIT,
        "target": TARGET,
        "version": manifest["product_version"],
    });
    let mut debug_identity = valid.clone();
    debug_identity["result"]["build_identity"] = json!(sha256_identity(
        canonicalize_json_v1(&debug_preimage).unwrap().as_bytes()
    ));
    assert!(
        fixture
            .verify_document(&debug_identity, SOURCE_COMMIT)
            .is_err(),
        "verifier accepted a debug-profile identity as a release binary"
    );
    assert!(
        fixture
            .verify_document(&valid, "fedcba9876543210fedcba9876543210fedcba98")
            .is_err(),
        "verifier accepted source-commit drift"
    );
}

#[test]
fn manifest_and_schema_registry_fail_closed_for_every_authority_drift() {
    let retired_ipc = ContractFixture::new(Layout::Source);
    let mut manifest = retired_ipc.manifest();
    manifest["supported_ipc_ids"] = json!(["podway.ipc/v1"]);
    retired_ipc.write_manifest(manifest);
    assert!(retired_ipc.verify_valid().is_err());

    let self_digest = ContractFixture::new(Layout::Source);
    let mut manifest = self_digest.manifest();
    manifest["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
    fs::write(
        self_digest.root.join("contracts/contract-manifest-v2.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert!(self_digest.verify_valid().is_err());

    let member_digest = ContractFixture::new(Layout::Source);
    let member = physical_path(
        &member_digest.root,
        member_digest.layout,
        "schemas/version-result-v2.schema.json",
    );
    let mut bytes = fs::read(&member).unwrap();
    bytes.push(b' ');
    fs::write(member, bytes).unwrap();
    assert!(member_digest.verify_valid().is_err());

    let missing = ContractFixture::new(Layout::Source);
    fs::remove_file(physical_path(
        &missing.root,
        missing.layout,
        "schemas/version-result-v2.schema.json",
    ))
    .unwrap();
    assert!(missing.verify_valid().is_err());

    let extra = ContractFixture::new(Layout::Source);
    let extra_path = physical_path(&extra.root, extra.layout, "schemas/extra.schema.json");
    fs::write(
        extra_path,
        br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","$id":"urn:podway:schema:extra:v1"}"#,
    )
    .unwrap();
    assert!(extra.verify_valid().is_err());

    let duplicate_path = ContractFixture::new(Layout::Source);
    let mut manifest = duplicate_path.manifest();
    let duplicate = manifest["assets"][0].clone();
    manifest["assets"].as_array_mut().unwrap().push(duplicate);
    duplicate_path.write_manifest(manifest);
    assert!(duplicate_path.verify_valid().is_err());

    let duplicate_id = ContractFixture::new(Layout::Source);
    let schema_path = physical_path(
        &duplicate_id.root,
        duplicate_id.layout,
        "schemas/version-summary-v1.schema.json",
    );
    let mut schema: Value = serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    schema["$id"] = json!("urn:podway:schema:version-result:v2");
    fs::write(&schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
    duplicate_id.refresh_asset("schemas/version-summary-v1.schema.json");
    assert!(duplicate_id.verify_valid().is_err());

    for reference in [
        "https://example.invalid/external.schema.json",
        "file:///tmp/external.schema.json",
        "urn:podway:schema:missing:v1",
    ] {
        let external = ContractFixture::new(Layout::Source);
        let schema_path = physical_path(
            &external.root,
            external.layout,
            "schemas/output-v4.schema.json",
        );
        let mut schema: Value = serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
        schema["$ref"] = json!(reference);
        fs::write(&schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
        external.refresh_asset("schemas/output-v4.schema.json");
        assert!(
            external.verify_valid().is_err(),
            "verifier accepted external or unknown reference {reference}"
        );
    }

    let symlinked = ContractFixture::new(Layout::Source);
    let schema_path = physical_path(
        &symlinked.root,
        symlinked.layout,
        "schemas/version-result-v2.schema.json",
    );
    let target = symlinked.root.join("outside-schema.json");
    fs::copy(&schema_path, &target).unwrap();
    fs::remove_file(&schema_path).unwrap();
    symlink(&target, &schema_path).unwrap();
    assert!(symlinked.verify_valid().is_err());
}
