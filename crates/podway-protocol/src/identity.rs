use serde::Serialize;

include!(concat!(env!("OUT_DIR"), "/contract_identity.rs"));

/// Static build and contract identity embedded in a Podway executable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuildIdentityV1 {
    schema: &'static str,
    product: &'static str,
    version: &'static str,
    target: &'static str,
    build_identity: &'static str,
    source_commit: Option<&'static str>,
    contract_manifest_schema: &'static str,
    contract_manifest_digest: &'static str,
    supported_ipc_ids: &'static [&'static str],
}

impl BuildIdentityV1 {
    pub const fn schema(&self) -> &'static str {
        self.schema
    }

    pub const fn product(&self) -> &'static str {
        self.product
    }

    pub const fn version(&self) -> &'static str {
        self.version
    }

    pub const fn target(&self) -> &'static str {
        self.target
    }

    pub const fn build_identity(&self) -> &'static str {
        self.build_identity
    }

    pub const fn source_commit(&self) -> Option<&'static str> {
        self.source_commit
    }

    pub const fn contract_manifest_schema(&self) -> &'static str {
        self.contract_manifest_schema
    }

    pub const fn contract_manifest_digest(&self) -> &'static str {
        self.contract_manifest_digest
    }

    pub const fn supported_ipc_ids(&self) -> &'static [&'static str] {
        self.supported_ipc_ids
    }
}

pub const fn build_identity_v1() -> BuildIdentityV1 {
    BuildIdentityV1 {
        schema: "podway.version-result/v1",
        product: PRODUCT_V1,
        version: PRODUCT_VERSION_V1,
        target: BUILD_TARGET_V1,
        build_identity: BUILD_IDENTITY_V1,
        source_commit: SOURCE_COMMIT_V1,
        contract_manifest_schema: CONTRACT_MANIFEST_SCHEMA_V1,
        contract_manifest_digest: CONTRACT_MANIFEST_DIGEST_V1,
        supported_ipc_ids: CONTRACT_SUPPORTED_IPC_IDS_V1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_identity_is_complete_and_canonical() {
        let identity = build_identity_v1();
        assert_eq!(identity.schema(), "podway.version-result/v1");
        assert_eq!(identity.product(), "podway");
        assert_eq!(identity.version(), env!("CARGO_PKG_VERSION"));
        assert!(!identity.target().is_empty());
        assert_eq!(identity.supported_ipc_ids(), ["podway.ipc/v1"]);
        for digest in [
            identity.build_identity(),
            identity.contract_manifest_digest(),
        ] {
            assert_eq!(digest.len(), 71);
            assert!(digest.starts_with("sha256:"));
            assert!(digest[7..].bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
        assert_eq!(
            identity.contract_manifest_schema(),
            "podway.contract-manifest/v1"
        );
    }
}
