use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    error::Error,
    fmt, fs, io,
    path::{Component, Path, PathBuf},
    process::Command,
    sync::Arc,
};

use jsonschema::{Retrieve, Uri, Validator};
use podway_core::canonicalize_json_v1;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ResponseEnvelopeV1;

const MANIFEST_SCHEMA_V1: &str = "podway.contract-manifest/v1";
const VERSION_RESULT_SCHEMA_V1: &str = "podway.version-result/v1";
const OUTPUT_SCHEMA_PATH: &str = "schemas/output-v1.schema.json";
const VERSION_SCHEMA_PATH: &str = "schemas/version-result-v1.schema.json";
const MANIFEST_SCHEMA_PATH: &str = "schemas/contract-manifest-v1.schema.json";
const MAX_IDENTITY_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct ReleaseContractVerifierConfigV1 {
    contract_root: PathBuf,
    podway: PathBuf,
    podwayd: PathBuf,
    expected_target: String,
    expected_source_commit: String,
}

impl ReleaseContractVerifierConfigV1 {
    pub fn new(
        contract_root: impl Into<PathBuf>,
        podway: impl Into<PathBuf>,
        podwayd: impl Into<PathBuf>,
        expected_target: impl Into<String>,
        expected_source_commit: impl Into<String>,
    ) -> Self {
        Self {
            contract_root: contract_root.into(),
            podway: podway.into(),
            podwayd: podwayd.into(),
            expected_target: expected_target.into(),
            expected_source_commit: expected_source_commit.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VerificationReceiptV1 {
    schema: &'static str,
    product: String,
    version: String,
    target: String,
    source_commit: String,
    contract_manifest_schema: String,
    contract_manifest_digest: String,
    build_identity: String,
    supported_ipc_ids: Vec<String>,
    schema_count: usize,
    asset_count: usize,
    binary_results_equal: bool,
}

impl VerificationReceiptV1 {
    pub fn build_identity(&self) -> &str {
        &self.build_identity
    }

    pub fn contract_manifest_digest(&self) -> &str {
        &self.contract_manifest_digest
    }

    pub fn contract_manifest_schema(&self) -> &str {
        &self.contract_manifest_schema
    }

    pub fn product(&self) -> &str {
        &self.product
    }

    pub fn source_commit(&self) -> &str {
        &self.source_commit
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseContractErrorV1(String);

impl ReleaseContractErrorV1 {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ReleaseContractErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ReleaseContractErrorV1 {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractManifestV1 {
    schema_version: String,
    product: String,
    product_version: String,
    supported_ipc_ids: Vec<String>,
    assets: Vec<ManifestAssetV1>,
    digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestAssetV1 {
    kind: String,
    path: String,
    sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContractRootLayoutV1 {
    Source,
    Packaged,
}

struct ContractSetV1 {
    manifest_value: Value,
    manifest: ContractManifestV1,
    schemas: BTreeMap<String, Value>,
    retriever: ManifestSchemaRetrieverV1,
}

#[derive(Clone)]
struct ManifestSchemaRetrieverV1(Arc<HashMap<String, Value>>);

impl Retrieve for ManifestSchemaRetrieverV1 {
    fn retrieve(&self, uri: &Uri<String>) -> Result<Value, Box<dyn Error + Send + Sync>> {
        self.0.get(uri.as_str()).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("unregistered contract schema URI: {uri}"),
            )
            .into()
        })
    }
}

pub fn verify_release_contract_v1(
    config: &ReleaseContractVerifierConfigV1,
) -> Result<VerificationReceiptV1, ReleaseContractErrorV1> {
    if config.expected_target.is_empty() || config.expected_source_commit.is_empty() {
        return Err(ReleaseContractErrorV1::new(
            "expected target and source commit must be non-empty",
        ));
    }
    let contracts = ContractSetV1::load(&config.contract_root)?;
    let cli = contracts.probe_identity(
        &config.podway,
        "podway",
        &config.expected_target,
        &config.expected_source_commit,
    )?;
    let daemon = contracts.probe_identity(
        &config.podwayd,
        "podwayd",
        &config.expected_target,
        &config.expected_source_commit,
    )?;
    if cli != daemon {
        return Err(ReleaseContractErrorV1::new(
            "podway and podwayd identity results differ",
        ));
    }
    let build_identity = required_string(&cli, "build_identity", "shared identity")?;
    Ok(VerificationReceiptV1 {
        schema: "podway.contract-verification/v1",
        product: contracts.manifest.product.clone(),
        version: contracts.manifest.product_version.clone(),
        target: config.expected_target.clone(),
        source_commit: config.expected_source_commit.clone(),
        contract_manifest_schema: contracts.manifest.schema_version.clone(),
        contract_manifest_digest: contracts.manifest.digest.clone(),
        build_identity,
        supported_ipc_ids: contracts.manifest.supported_ipc_ids.clone(),
        schema_count: contracts.schemas.len(),
        asset_count: contracts.manifest.assets.len(),
        binary_results_equal: true,
    })
}

impl ContractSetV1 {
    fn load(root: &Path) -> Result<Self, ReleaseContractErrorV1> {
        let root = checked_root(root)?;
        let source_schemas = root.join("assets/schemas").is_dir();
        let packaged_schemas = root.join("schemas").is_dir();
        let layout = match (source_schemas, packaged_schemas) {
            (true, false) => ContractRootLayoutV1::Source,
            (false, true) => ContractRootLayoutV1::Packaged,
            (true, true) => {
                return Err(ReleaseContractErrorV1::new(
                    "contract root is ambiguous between source and packaged layouts",
                ));
            }
            (false, false) => {
                return Err(ReleaseContractErrorV1::new(
                    "contract root has no canonical schema directory",
                ));
            }
        };
        let manifest_path = checked_member_path(
            &root,
            Path::new("contracts/contract-manifest-v1.json"),
            "contract manifest",
        )?;
        let manifest_bytes = fs::read(&manifest_path).map_err(|error| {
            ReleaseContractErrorV1::new(format!("cannot read contract manifest: {error}"))
        })?;
        let manifest_value: Value = serde_json::from_slice(&manifest_bytes).map_err(|error| {
            ReleaseContractErrorV1::new(format!("contract manifest is not valid JSON: {error}"))
        })?;
        let manifest: ContractManifestV1 =
            serde_json::from_value(manifest_value.clone()).map_err(|error| {
                ReleaseContractErrorV1::new(format!("contract manifest shape is invalid: {error}"))
            })?;
        validate_manifest_identity(&manifest, &manifest_value)?;

        let mut prior_path = None;
        let mut logical_paths = BTreeSet::new();
        let mut schema_paths = BTreeSet::new();
        let mut schemas = BTreeMap::new();
        let mut schema_ids = BTreeMap::new();
        let mut resources = HashMap::new();
        for asset in &manifest.assets {
            validate_asset_shape(asset)?;
            if prior_path
                .as_deref()
                .is_some_and(|prior| prior >= asset.path.as_str())
            {
                return Err(ReleaseContractErrorV1::new(
                    "contract manifest asset paths must be sorted and unique",
                ));
            }
            prior_path = Some(asset.path.clone());
            if !logical_paths.insert(asset.path.clone()) {
                return Err(ReleaseContractErrorV1::new(
                    "contract manifest contains a duplicate asset path",
                ));
            }
            let logical = normalized_relative_path(&asset.path, "contract asset")?;
            let physical = physical_asset_path(&root, layout, &logical)?;
            let bytes = fs::read(&physical).map_err(|error| {
                ReleaseContractErrorV1::new(format!(
                    "cannot read contract asset {}: {error}",
                    asset.path
                ))
            })?;
            let observed_digest = sha256_identity(&bytes);
            if observed_digest != asset.sha256 {
                return Err(ReleaseContractErrorV1::new(format!(
                    "contract asset digest mismatch: {}",
                    asset.path
                )));
            }
            if asset.kind == "schema" {
                if !is_versioned_schema_path(&asset.path) {
                    return Err(ReleaseContractErrorV1::new(format!(
                        "schema asset has an invalid logical path: {}",
                        asset.path
                    )));
                }
                let schema: Value = serde_json::from_slice(&bytes).map_err(|error| {
                    ReleaseContractErrorV1::new(format!(
                        "schema asset {} is not valid JSON: {error}",
                        asset.path
                    ))
                })?;
                let identifier = schema
                    .get("$id")
                    .and_then(Value::as_str)
                    .filter(|value| value.starts_with("urn:podway:schema:") && !value.contains('#'))
                    .ok_or_else(|| {
                        ReleaseContractErrorV1::new(format!(
                            "schema asset {} has an invalid $id",
                            asset.path
                        ))
                    })?
                    .to_owned();
                if schema_identifier_count(&schema) != 1 {
                    return Err(ReleaseContractErrorV1::new(format!(
                        "schema asset {} must contain exactly one top-level $id",
                        asset.path
                    )));
                }
                if schema.get("$schema").and_then(Value::as_str)
                    != Some("https://json-schema.org/draft/2020-12/schema")
                {
                    return Err(ReleaseContractErrorV1::new(format!(
                        "schema asset {} has an unsupported draft",
                        asset.path
                    )));
                }
                if schema_ids
                    .insert(identifier.clone(), asset.path.clone())
                    .is_some()
                {
                    return Err(ReleaseContractErrorV1::new(format!(
                        "schema asset {} duplicates $id {identifier}",
                        asset.path
                    )));
                }
                let path_uri = schema_path_uri(&asset.path);
                resources.insert(identifier, schema.clone());
                resources.insert(path_uri, schema.clone());
                schema_paths.insert(asset.path.clone());
                schemas.insert(asset.path.clone(), schema);
            }
        }

        let observed_schemas = collect_schema_inventory(&root, layout)?;
        if observed_schemas != schema_paths {
            return Err(ReleaseContractErrorV1::new(format!(
                "manifest schema inventory mismatch: missing={:?}, extra={:?}",
                schema_paths
                    .difference(&observed_schemas)
                    .collect::<Vec<_>>(),
                observed_schemas
                    .difference(&schema_paths)
                    .collect::<Vec<_>>()
            )));
        }
        for required in [
            OUTPUT_SCHEMA_PATH,
            VERSION_SCHEMA_PATH,
            MANIFEST_SCHEMA_PATH,
        ] {
            if !schemas.contains_key(required) {
                return Err(ReleaseContractErrorV1::new(format!(
                    "required contract schema is missing: {required}"
                )));
            }
        }
        for (path, schema) in &schemas {
            validate_schema_references(path, schema, &schema_ids, &schema_paths)?;
        }
        let retriever = ManifestSchemaRetrieverV1(Arc::new(resources));
        let contracts = Self {
            manifest_value,
            manifest,
            schemas,
            retriever,
        };
        for path in contracts.schemas.keys() {
            contracts.validator(path)?;
        }
        contracts.validate_schema(
            MANIFEST_SCHEMA_PATH,
            &contracts.manifest_value,
            "contract manifest",
        )?;
        Ok(contracts)
    }

    fn validator(&self, logical_path: &str) -> Result<Validator, ReleaseContractErrorV1> {
        let schema = self.schemas.get(logical_path).ok_or_else(|| {
            ReleaseContractErrorV1::new(format!(
                "schema is not manifest-registered: {logical_path}"
            ))
        })?;
        jsonschema::draft202012::options()
            .with_retriever(self.retriever.clone())
            .should_validate_formats(true)
            .build(schema)
            .map_err(|error| {
                ReleaseContractErrorV1::new(format!(
                    "schema {logical_path} does not compile against the manifest registry: {error}"
                ))
            })
    }

    fn validate_schema(
        &self,
        logical_path: &str,
        instance: &Value,
        label: &str,
    ) -> Result<(), ReleaseContractErrorV1> {
        let validator = self.validator(logical_path)?;
        let errors = validator
            .iter_errors(instance)
            .take(8)
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ReleaseContractErrorV1::new(format!(
                "{label} failed {logical_path}: {errors:?}"
            )))
        }
    }

    fn probe_identity(
        &self,
        binary: &Path,
        role: &str,
        expected_target: &str,
        expected_source_commit: &str,
    ) -> Result<Value, ReleaseContractErrorV1> {
        let binary = checked_binary(binary, role)?;
        let output = Command::new(&binary)
            .args(["version", "--json", "--identity"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .map_err(|error| {
                ReleaseContractErrorV1::new(format!(
                    "cannot execute {role} identity probe: {error}"
                ))
            })?;
        if !output.status.success() || !output.stderr.is_empty() {
            return Err(ReleaseContractErrorV1::new(format!(
                "{role} identity probe did not exit cleanly"
            )));
        }
        if output.stdout.len() > MAX_IDENTITY_OUTPUT_BYTES
            || !output.stdout.ends_with(b"\n")
            || output.stdout[..output.stdout.len() - 1].contains(&b'\n')
        {
            return Err(ReleaseContractErrorV1::new(format!(
                "{role} identity probe must emit one bounded newline-terminated document"
            )));
        }
        let document: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
            ReleaseContractErrorV1::new(format!("{role} identity probe is not valid JSON: {error}"))
        })?;
        self.validate_schema(
            OUTPUT_SCHEMA_PATH,
            &document,
            &format!("{role} identity envelope"),
        )?;
        let response: ResponseEnvelopeV1 =
            serde_json::from_value(document.clone()).map_err(|error| {
                ReleaseContractErrorV1::new(format!(
                    "{role} identity envelope fails the runtime protocol: {error}"
                ))
            })?;
        let ResponseEnvelopeV1::Output(output) = response else {
            return Err(ReleaseContractErrorV1::new(format!(
                "{role} identity probe returned an error envelope"
            )));
        };
        if output.command().as_str() != "version" {
            return Err(ReleaseContractErrorV1::new(format!(
                "{role} identity envelope has the wrong command"
            )));
        }
        let result = Value::Object(output.result().clone());
        self.validate_schema(
            VERSION_SCHEMA_PATH,
            &result,
            &format!("{role} identity result"),
        )?;
        let expected = [
            ("schema", VERSION_RESULT_SCHEMA_V1),
            ("product", self.manifest.product.as_str()),
            ("version", self.manifest.product_version.as_str()),
            ("target", expected_target),
            ("source_commit", expected_source_commit),
            (
                "contract_manifest_schema",
                self.manifest.schema_version.as_str(),
            ),
            ("contract_manifest_digest", self.manifest.digest.as_str()),
        ];
        for (field, expected) in expected {
            if result.get(field).and_then(Value::as_str) != Some(expected) {
                return Err(ReleaseContractErrorV1::new(format!(
                    "{role} identity field {field} does not match the expected contract"
                )));
            }
        }
        let supported_ipc_ids = serde_json::to_value(&self.manifest.supported_ipc_ids)
            .expect("manifest IPC identifiers always serialize");
        if result.get("supported_ipc_ids") != Some(&supported_ipc_ids) {
            return Err(ReleaseContractErrorV1::new(format!(
                "{role} supported IPC identity does not match the contract manifest"
            )));
        }
        Ok(result)
    }
}

fn is_versioned_schema_path(path: &str) -> bool {
    let Some(relative) = path.strip_prefix("schemas/") else {
        return false;
    };
    let Some(name) = relative.rsplit('/').next() else {
        return false;
    };
    let Some(stem) = name.strip_suffix(".schema.json") else {
        return false;
    };
    let Some((basename, version)) = stem.rsplit_once("-v") else {
        return false;
    };
    !basename.is_empty()
        && !version.is_empty()
        && !version.starts_with('0')
        && version.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_manifest_identity(
    manifest: &ContractManifestV1,
    value: &Value,
) -> Result<(), ReleaseContractErrorV1> {
    if manifest.schema_version != MANIFEST_SCHEMA_V1
        || manifest.product != "podway"
        || manifest.product_version.is_empty()
        || manifest.supported_ipc_ids.is_empty()
        || manifest.supported_ipc_ids.iter().any(String::is_empty)
        || manifest
            .supported_ipc_ids
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != manifest.supported_ipc_ids.len()
        || manifest.assets.is_empty()
        || !is_sha256_identity(&manifest.digest)
    {
        return Err(ReleaseContractErrorV1::new(
            "contract manifest identity is invalid",
        ));
    }
    let mut unsigned = value.clone();
    unsigned
        .as_object_mut()
        .ok_or_else(|| ReleaseContractErrorV1::new("contract manifest must be an object"))?
        .remove("digest");
    let canonical = canonicalize_json_v1(&unsigned).map_err(|error| {
        ReleaseContractErrorV1::new(format!("contract manifest cannot canonicalize: {error}"))
    })?;
    if manifest.digest != sha256_identity(canonical.as_bytes()) {
        return Err(ReleaseContractErrorV1::new(
            "contract manifest self digest mismatch",
        ));
    }
    Ok(())
}

fn validate_asset_shape(asset: &ManifestAssetV1) -> Result<(), ReleaseContractErrorV1> {
    if !matches!(
        asset.kind.as_str(),
        "schema"
            | "catalog"
            | "transition_matrix"
            | "canonicalization_rules"
            | "known_answer_fixture"
    ) || !is_sha256_identity(&asset.sha256)
    {
        return Err(ReleaseContractErrorV1::new(format!(
            "contract manifest asset shape is invalid: {}",
            asset.path
        )));
    }
    Ok(())
}

fn checked_root(root: &Path) -> Result<PathBuf, ReleaseContractErrorV1> {
    let metadata = fs::symlink_metadata(root).map_err(|error| {
        ReleaseContractErrorV1::new(format!("cannot inspect contract root: {error}"))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ReleaseContractErrorV1::new(
            "contract root must be a real directory",
        ));
    }
    fs::canonicalize(root).map_err(|error| {
        ReleaseContractErrorV1::new(format!("cannot canonicalize contract root: {error}"))
    })
}

fn normalized_relative_path(value: &str, label: &str) -> Result<PathBuf, ReleaseContractErrorV1> {
    if value.is_empty()
        || value.contains('\\')
        || value
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(ReleaseContractErrorV1::new(format!(
            "{label} is not a normalized relative path: {value}"
        )));
    }
    let path = Path::new(value);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ReleaseContractErrorV1::new(format!(
            "{label} is not a normalized relative path: {value}"
        )));
    }
    Ok(path.to_path_buf())
}

fn checked_member_path(
    root: &Path,
    relative: &Path,
    label: &str,
) -> Result<PathBuf, ReleaseContractErrorV1> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(ReleaseContractErrorV1::new(format!(
                "{label} path is not normalized"
            )));
        };
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            ReleaseContractErrorV1::new(format!("cannot inspect {label}: {error}"))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ReleaseContractErrorV1::new(format!(
                "{label} path contains a symlink"
            )));
        }
    }
    let metadata = fs::symlink_metadata(&current)
        .map_err(|error| ReleaseContractErrorV1::new(format!("cannot inspect {label}: {error}")))?;
    if !metadata.is_file() {
        return Err(ReleaseContractErrorV1::new(format!(
            "{label} must be a regular file"
        )));
    }
    let canonical = fs::canonicalize(&current).map_err(|error| {
        ReleaseContractErrorV1::new(format!("cannot canonicalize {label}: {error}"))
    })?;
    if !canonical.starts_with(root) {
        return Err(ReleaseContractErrorV1::new(format!(
            "{label} escapes the contract root"
        )));
    }
    Ok(canonical)
}

fn physical_asset_path(
    root: &Path,
    layout: ContractRootLayoutV1,
    logical: &Path,
) -> Result<PathBuf, ReleaseContractErrorV1> {
    let mapped = if layout == ContractRootLayoutV1::Source {
        match logical.components().next() {
            Some(Component::Normal(prefix)) if prefix == "schemas" => {
                PathBuf::from("assets/schemas")
                    .join(logical.strip_prefix("schemas").expect("prefix"))
            }
            Some(Component::Normal(prefix)) if prefix == "presets" => {
                PathBuf::from("assets/presets")
                    .join(logical.strip_prefix("presets").expect("prefix"))
            }
            Some(Component::Normal(prefix)) if prefix == "spec" => {
                PathBuf::from("assets/specifications")
                    .join(logical.strip_prefix("spec").expect("prefix"))
            }
            _ => logical.to_path_buf(),
        }
    } else {
        logical.to_path_buf()
    };
    checked_member_path(
        root,
        &mapped,
        &format!("contract asset {}", logical.display()),
    )
}

fn collect_schema_inventory(
    root: &Path,
    layout: ContractRootLayoutV1,
) -> Result<BTreeSet<String>, ReleaseContractErrorV1> {
    let relative = if layout == ContractRootLayoutV1::Source {
        Path::new("assets/schemas")
    } else {
        Path::new("schemas")
    };
    let directory = root.join(relative);
    let mut pending = vec![directory];
    let mut inventory = BTreeSet::new();
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current).map_err(|error| {
            ReleaseContractErrorV1::new(format!("cannot read schema inventory: {error}"))
        })? {
            let entry = entry.map_err(|error| {
                ReleaseContractErrorV1::new(format!("cannot read schema inventory: {error}"))
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                ReleaseContractErrorV1::new(format!("cannot inspect schema inventory: {error}"))
            })?;
            if metadata.file_type().is_symlink() {
                return Err(ReleaseContractErrorV1::new(
                    "schema inventory contains a symlink",
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".schema.json"))
            {
                let suffix = path.strip_prefix(root.join(relative)).map_err(|_| {
                    ReleaseContractErrorV1::new("schema inventory escaped its root")
                })?;
                inventory.insert(format!("schemas/{}", suffix.to_string_lossy()));
            } else if !metadata.is_file() {
                return Err(ReleaseContractErrorV1::new(
                    "schema inventory contains an unsupported node",
                ));
            }
        }
    }
    Ok(inventory)
}

fn validate_schema_references(
    schema_path: &str,
    value: &Value,
    schema_ids: &BTreeMap<String, String>,
    schema_paths: &BTreeSet<String>,
) -> Result<(), ReleaseContractErrorV1> {
    match value {
        Value::Object(object) => {
            for keyword in ["$ref", "$dynamicRef"] {
                if let Some(reference) = object.get(keyword).and_then(Value::as_str) {
                    validate_schema_reference(schema_path, reference, schema_ids, schema_paths)?;
                }
            }
            for nested in object.values() {
                validate_schema_references(schema_path, nested, schema_ids, schema_paths)?;
            }
        }
        Value::Array(values) => {
            for nested in values {
                validate_schema_references(schema_path, nested, schema_ids, schema_paths)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn schema_identifier_count(value: &Value) -> usize {
    match value {
        Value::Object(object) => {
            usize::from(object.contains_key("$id"))
                + object.values().map(schema_identifier_count).sum::<usize>()
        }
        Value::Array(values) => values.iter().map(schema_identifier_count).sum(),
        _ => 0,
    }
}

fn validate_schema_reference(
    schema_path: &str,
    reference: &str,
    schema_ids: &BTreeMap<String, String>,
    schema_paths: &BTreeSet<String>,
) -> Result<(), ReleaseContractErrorV1> {
    if reference.starts_with('#') {
        return Ok(());
    }
    let base = reference.split('#').next().unwrap_or(reference);
    if schema_ids.contains_key(base) {
        return Ok(());
    }
    if let Some(path) = base.strip_prefix("podway:///") {
        if schema_paths.contains(path) {
            return Ok(());
        }
    } else if !base.contains(':') {
        let parent = Path::new(schema_path)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        let candidate = parent.join(normalized_relative_path(base, "schema reference")?);
        let candidate = candidate.to_string_lossy().to_string();
        if schema_paths.contains(&candidate) {
            return Ok(());
        }
    }
    Err(ReleaseContractErrorV1::new(format!(
        "schema {schema_path} contains an external or unknown $ref: {reference}"
    )))
}

fn checked_binary(binary: &Path, role: &str) -> Result<PathBuf, ReleaseContractErrorV1> {
    let metadata = fs::symlink_metadata(binary).map_err(|error| {
        ReleaseContractErrorV1::new(format!("cannot inspect {role} binary: {error}"))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ReleaseContractErrorV1::new(format!(
            "{role} binary must be a regular non-symlink file"
        )));
    }
    fs::canonicalize(binary).map_err(|error| {
        ReleaseContractErrorV1::new(format!("cannot canonicalize {role} binary: {error}"))
    })
}

fn required_string(
    value: &Value,
    field: &str,
    label: &str,
) -> Result<String, ReleaseContractErrorV1> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ReleaseContractErrorV1::new(format!("{label} has no valid {field} field")))
}

fn schema_path_uri(logical_path: &str) -> String {
    format!("podway:///{logical_path}")
}

fn sha256_identity(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn is_sha256_identity(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
