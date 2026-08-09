use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

mod build_support;

use build_support::{canonical_json_bytes, git_rerun_paths};

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

fn emit_git_rerun_paths(workspace: &Path) {
    for path in git_rerun_paths(workspace) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn generate_embedded_schemas(workspace: &Path, output_dir: &Path) {
    let schema_dir = workspace.join("assets/schemas");
    let mut paths = fs::read_dir(&schema_dir)
        .unwrap_or_else(|error| fail(error.to_string()))
        .map(|entry| entry.unwrap_or_else(|error| fail(error.to_string())).path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();

    let mut entries = Vec::with_capacity(paths.len());
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        let source = fs::read_to_string(&path).unwrap_or_else(|error| fail(error.to_string()));
        let schema: Value =
            serde_json::from_str(&source).unwrap_or_else(|error| fail(error.to_string()));
        let id = string_field(&schema, "$id");
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_else(|| fail("schema filename is not UTF-8"));
        entries.push(format!(
            "({}, {}, {})",
            rust_string(id),
            rust_string(filename),
            rust_string(&source)
        ));
    }
    let generated = format!(
        "pub const EMBEDDED_JSON_SCHEMAS_V1: &[(&str, &str, &str)] = &[{}];\n",
        entries.join(",")
    );
    fs::write(output_dir.join("embedded_json_schemas.rs"), generated)
        .unwrap_or_else(|error| fail(error.to_string()));
}

fn main() {
    println!("cargo:rerun-if-env-changed=PODWAY_SOURCE_COMMIT");
    let crate_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let workspace = crate_dir.join("../..");
    let manifest_path = workspace.join("contracts/contract-manifest-v1.json");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    emit_git_rerun_paths(&workspace);

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
        Sha256::digest(
            canonical_json_bytes(&manifest)
                .unwrap_or_else(|error| fail(format!("manifest is not canonicalizable: {error}")))
        )
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
        Sha256::digest(
            canonical_json_bytes(&build_preimage).unwrap_or_else(|error| {
                fail(format!("build identity is not canonicalizable: {error}"))
            })
        )
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
    let output_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    generate_embedded_schemas(&workspace, &output_dir);
    let output = output_dir.join("contract_identity.rs");
    fs::write(output, generated).unwrap_or_else(|error| fail(error.to_string()));
}
