use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MANIFEST_SCHEMA: &str = "podway.contract-manifest/v1";

fn fail(message: impl AsRef<str>) -> ! {
    panic!("contract identity build failed: {}", message.as_ref());
}

fn string_field<'a>(value: &'a Value, field: &str) -> &'a str {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail(format!("manifest field {field} is missing or invalid")))
}

fn source_commit(workspace: &Path) -> Option<String> {
    let explicit = env::var("PODWAY_SOURCE_COMMIT").ok();
    let observed = explicit.or_else(|| {
        let output = Command::new("git")
            .args(["-C", workspace.to_str()?, "rev-parse", "--verify", "HEAD"])
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    });
    observed.filter(|value| {
        matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn rust_string(value: &str) -> String {
    serde_json::to_string(value).expect("identity strings serialize")
}

fn main() {
    println!("cargo:rerun-if-env-changed=PODWAY_SOURCE_COMMIT");
    let crate_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let workspace = crate_dir.join("../..");
    let manifest_path = workspace.join("contracts/contract-manifest-v1.json");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    if let Ok(output) = Command::new("git")
        .args([
            "-C",
            workspace.to_str().unwrap_or("."),
            "rev-parse",
            "--git-path",
            "HEAD",
        ])
        .output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !path.is_empty() {
            let path = PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                workspace.join(path)
            };
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    let bytes = fs::read(&manifest_path).unwrap_or_else(|error| fail(error.to_string()));
    let mut manifest: Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|error| fail(error.to_string()));
    if string_field(&manifest, "schema_version") != MANIFEST_SCHEMA {
        fail("manifest schema version is invalid");
    }
    let digest = string_field(&manifest, "digest").to_owned();
    manifest
        .as_object_mut()
        .unwrap_or_else(|| fail("manifest must be an object"))
        .remove("digest");
    let expected_digest = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&manifest).expect("manifest serializes"))
    );
    if digest != expected_digest {
        fail("manifest self digest is invalid");
    }

    let product = string_field(&manifest, "product");
    let version = string_field(&manifest, "product_version");
    let supported_ipc_ids = manifest
        .get("supported_ipc_ids")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("supported_ipc_ids must be an array"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| fail("supported_ipc_ids entries must be strings"))
        })
        .collect::<Vec<_>>();
    if supported_ipc_ids.is_empty() {
        fail("supported_ipc_ids must not be empty");
    }
    let target = env::var("TARGET").unwrap_or_else(|error| fail(error.to_string()));
    let profile = env::var("PROFILE").unwrap_or_else(|error| fail(error.to_string()));
    let source_commit = source_commit(&workspace);
    let build_preimage = json!({
        "contract_manifest_digest": digest,
        "product": product,
        "profile": profile,
        "source_commit": source_commit,
        "target": target,
        "version": version,
    });
    let build_identity = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&build_preimage).expect("build identity serializes"))
    );
    let ipc_ids = supported_ipc_ids
        .iter()
        .map(|value| rust_string(value))
        .collect::<Vec<_>>()
        .join(",");
    let source = source_commit
        .as_deref()
        .map(|value| format!("Some({})", rust_string(value)))
        .unwrap_or_else(|| "None".to_owned());
    let generated = format!(
        "pub const PRODUCT_V1: &str = {};\n\
         pub const PRODUCT_VERSION_V1: &str = {};\n\
         pub const BUILD_TARGET_V1: &str = {};\n\
         pub const BUILD_IDENTITY_V1: &str = {};\n\
         pub const SOURCE_COMMIT_V1: Option<&str> = {source};\n\
         pub const CONTRACT_MANIFEST_SCHEMA_V1: &str = {};\n\
         pub const CONTRACT_MANIFEST_DIGEST_V1: &str = {};\n\
         pub const CONTRACT_SUPPORTED_IPC_IDS_V1: &[&str] = &[{ipc_ids}];\n",
        rust_string(product),
        rust_string(version),
        rust_string(&target),
        rust_string(&build_identity),
        rust_string(MANIFEST_SCHEMA),
        rust_string(&digest),
    );
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("contract_identity.rs");
    fs::write(output, generated).unwrap_or_else(|error| fail(error.to_string()));
}
