use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::PathBuf,
    process::Command,
};

use podway_core::{
    AttemptId, DomainError, ItemId, JobId, MAX_PROCEDURE_IDENTIFIER_BYTES, Revision, SessionId,
    Sha256Digest, StageId, WorkspaceId,
};

const CANONICAL_UUID: &str = "123e4567-e89b-12d3-a456-426614174000";

// ARC-007: Core contracts remain pure, deterministic, and independent of infrastructure.
#[test]
fn arc_007_uuid_newtypes_accept_canonical_values_and_reject_noncanonical_values() {
    assert_eq!(
        WorkspaceId::new(CANONICAL_UUID).unwrap().as_str(),
        CANONICAL_UUID
    );
    assert_eq!(
        SessionId::new(CANONICAL_UUID).unwrap().as_str(),
        CANONICAL_UUID
    );
    assert_eq!(
        AttemptId::new(CANONICAL_UUID).unwrap().as_str(),
        CANONICAL_UUID
    );
    assert_eq!(JobId::new(CANONICAL_UUID).unwrap().as_str(), CANONICAL_UUID);

    assert_eq!(
        WorkspaceId::new("123E4567-e89b-12d3-a456-426614174000"),
        Err(DomainError::InvalidUuid {
            field: "WorkspaceId",
        })
    );
    assert_eq!(
        SessionId::new("123e4567e89b-12d3-a456-426614174000"),
        Err(DomainError::InvalidUuid { field: "SessionId" })
    );
}

// ARC-007: Core has no infrastructure dependencies. This resolves podway-core's real
// Cargo dependency graph (mirroring the daemon's closed-world network-surface proof at
// crates/podway-daemon/tests/phase4_endpoint.rs::pac063_*) and proves, by executing
// `cargo metadata` rather than by convention, that both the crate's declared manifest
// dependencies and its full resolved transitive closure are a frozen, closed pure
// world. Adding an infrastructure/I/O crate (tokio, rusqlite, mio, hyper, reqwest, ...)
// to podway-core's `[dependencies]` fails this test.
#[test]
fn arc_007_dependency_graph_is_a_closed_pure_world_with_no_infrastructure_crates() {
    let (manifest_dependencies, resolved_packages) = resolved_core_packages();

    assert_eq!(
        manifest_dependencies,
        ["serde", "serde_json", "sha2"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        "ARC-007: podway-core's manifest dependency set is a frozen, closed-world policy"
    );

    assert_eq!(
        resolved_packages,
        [
            "block-buffer",
            "cfg-if",
            "cpufeatures",
            "crypto-common",
            "digest",
            "generic-array",
            "itoa",
            "libc",
            "memchr",
            "podway-core",
            "proc-macro2",
            "quote",
            "serde",
            "serde_core",
            "serde_derive",
            "serde_json",
            "sha2",
            "syn",
            "typenum",
            "unicode-ident",
            "version_check",
            "zmij",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        "ARC-007: podway-core's full resolved dependency graph is a frozen, closed pure world"
    );

    for &forbidden in ARC_007_FORBIDDEN_INFRASTRUCTURE_CRATES {
        assert!(
            !resolved_packages.contains(forbidden),
            "ARC-007 rejects infrastructure package {forbidden} anywhere in podway-core's resolved dependency graph"
        );
    }
}

// DOM-001: Stage and item identifiers are stable lowercase kebab-case procedure keys.
#[test]
fn dom_001_stage_and_item_identifiers_enforce_the_frozen_contract() {
    let maximum_length_identifier = format!("a{}", "b".repeat(MAX_PROCEDURE_IDENTIFIER_BYTES - 1));

    assert_eq!(
        StageId::new("prepare-release").unwrap().as_str(),
        "prepare-release"
    );
    assert_eq!(
        ItemId::new(maximum_length_identifier.clone())
            .unwrap()
            .as_str(),
        maximum_length_identifier
    );
    assert_eq!(
        StageId::new("prepare--release"),
        Err(DomainError::InvalidIdentifier { field: "StageId" })
    );
    assert_eq!(
        ItemId::new(""),
        Err(DomainError::EmptyValue { field: "ItemId" })
    );
    assert_eq!(
        ItemId::new("A-valid-looking-item"),
        Err(DomainError::InvalidIdentifier { field: "ItemId" })
    );
}

// STO-004: Revisions support deterministic optimistic-concurrency increments and overflow failure.
#[test]
fn sto_004_revision_newtype_preserves_zero_increment_and_overflow_contracts() {
    assert_eq!(Revision::ZERO.get(), 0);
    assert_eq!(Revision::new(41).checked_next(), Ok(Revision::new(42)));
    assert_eq!(
        Revision::new(u64::MAX).checked_next(),
        Err(DomainError::RevisionOverflow {
            revision: Revision::new(u64::MAX),
        })
    );
}

// API-001: SHA-256 digests use one stable, serializable lowercase representation.
#[test]
fn api_001_sha256_digest_newtype_accepts_only_canonical_sha256_values() {
    let valid_digest = format!("sha256:{}", "a".repeat(64));

    assert_eq!(
        Sha256Digest::new(valid_digest.clone()).unwrap().as_str(),
        valid_digest
    );
    assert_eq!(
        Sha256Digest::new(format!("sha256:{}A", "a".repeat(63))),
        Err(DomainError::InvalidSha256Digest)
    );
    assert_eq!(
        Sha256Digest::new(format!("sha512:{}", "a".repeat(64))),
        Err(DomainError::InvalidSha256Digest)
    );
}

/// ARC-007 defense-in-depth denylist: infrastructure/I/O crate names that must never
/// appear anywhere in podway-core's resolved dependency graph. The exact closed-world
/// assertions above already reject any unlisted package, named infrastructure or not;
/// this list documents, by name, the specific infrastructure families ARC-007 exists
/// to keep out (async runtimes, network stacks, TLS, process/signal control, and
/// embedded databases) so a future reviewer can see the intent even if the exact
/// allow-list is ever loosened to a subset check.
const ARC_007_FORBIDDEN_INFRASTRUCTURE_CRATES: &[&str] = &[
    "tokio",
    "tokio-util",
    "tokio-stream",
    "mio",
    "polling",
    "rusqlite",
    "libsqlite3-sys",
    "sqlx",
    "diesel",
    "redis",
    "postgres",
    "hyper",
    "hyper-util",
    "reqwest",
    "socket2",
    "ureq",
    "curl",
    "ssh2",
    "rustls",
    "rustls-pemfile",
    "native-tls",
    "openssl",
    "openssl-sys",
    "webpki",
    "ring",
    "tonic",
    "prost",
    "h2",
    "quinn",
    "axum",
    "warp",
    "actix-web",
    "tower",
    "async-trait",
    "async-channel",
    "async-io",
    "async-std",
    "futures",
    "futures-util",
    "futures-core",
    "futures-executor",
    "smol",
    "nix",
    "signal-hook",
];

/// Resolves podway-core's real Cargo dependency graph via `cargo metadata`, mirroring
/// `resolved_daemon_packages` in crates/podway-daemon/tests/phase4_endpoint.rs. Returns
/// the crate's manifest-declared dependency names (every kind: normal, build, and dev,
/// exactly as podway-core's own internal-DAG check treats all three uniformly) and the
/// full package-name closure of the resolved, activated dependency graph reachable from
/// podway-core's root node.
fn resolved_core_packages() -> (BTreeSet<String>, BTreeSet<String>) {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(&manifest)
        .output()
        .expect("Cargo metadata must execute");
    assert!(
        output.status.success(),
        "Cargo metadata must resolve the podway-core dependency graph: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("Cargo metadata must be valid JSON");
    let root = metadata["resolve"]["root"]
        .as_str()
        .expect("resolved core graph must identify its root package");
    let packages = metadata["packages"]
        .as_array()
        .expect("Cargo metadata must list resolved packages");

    let root_package = packages
        .iter()
        .find(|package| package["id"].as_str() == Some(root))
        .expect("resolved core graph must list its own root package");
    let manifest_dependencies = root_package["dependencies"]
        .as_array()
        .expect("root package manifest dependencies must be an array")
        .iter()
        .map(|dependency| {
            dependency["name"]
                .as_str()
                .expect("every manifest dependency must name a package")
                .to_owned()
        })
        .collect::<BTreeSet<_>>();

    let names = packages
        .iter()
        .filter_map(|package| {
            package["id"]
                .as_str()
                .zip(package["name"].as_str())
                .map(|(id, name)| (id.to_owned(), name.to_owned()))
        })
        .collect::<BTreeMap<_, _>>();
    let dependencies = metadata["resolve"]["nodes"]
        .as_array()
        .expect("resolved core graph must contain nodes")
        .iter()
        .filter_map(|node| {
            node["id"].as_str().map(|id| {
                (
                    id.to_owned(),
                    node["dependencies"]
                        .as_array()
                        .expect("resolved core node dependencies must be an array")
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>(),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();

    let mut pending = vec![root.to_owned()];
    let mut visited = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if visited.insert(id.clone()) {
            pending.extend(
                dependencies
                    .get(&id)
                    .expect("every reachable resolved package must have a node")
                    .iter()
                    .cloned(),
            );
        }
    }
    let resolved_packages = visited
        .into_iter()
        .map(|id| {
            names
                .get(&id)
                .expect("resolved node must name a package")
                .clone()
        })
        .collect::<BTreeSet<_>>();

    (manifest_dependencies, resolved_packages)
}
