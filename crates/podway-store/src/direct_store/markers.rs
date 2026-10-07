use std::{
    io::{Read, Write},
    path::PathBuf,
};

use super::{
    DirectStoreErrorV1, DirectStoreResultV1, DirectWorkspaceLayoutV1, ExclusiveMaintenanceLeaseV1,
};
use crate::{CanonicalExecutionJsonV1, IdempotencyKeyV1};
use podway_core::{Sha256Digest, WorkspaceId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectMaintenanceMarkerKindV1 {
    Intent,
    Reset,
    Removal,
}

impl DirectMaintenanceMarkerKindV1 {
    fn path(self, layout: &DirectWorkspaceLayoutV1) -> PathBuf {
        match self {
            Self::Intent => layout.intent_marker(),
            Self::Reset => layout.reset_marker(),
            Self::Removal => layout.removal_marker(),
        }
    }
    fn schema(self) -> &'static str {
        match self {
            Self::Intent => "podway.workspace-maintenance-intent-marker/v1",
            Self::Reset => "podway.workspace-reset-marker/v1",
            Self::Removal => "podway.workspace-removal-marker/v2",
        }
    }
}

/// Bounded canonical marker content. Store validates its identity fields; engine
/// validates the complete public schema, matching request/root and phase protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectMaintenanceMarkerV1 {
    kind: DirectMaintenanceMarkerKindV1,
    document: CanonicalExecutionJsonV1,
}

impl DirectMaintenanceMarkerV1 {
    pub fn new(
        kind: DirectMaintenanceMarkerKindV1,
        document: CanonicalExecutionJsonV1,
    ) -> DirectStoreResultV1<Self> {
        let value: serde_json::Value =
            serde_json::from_str(document.as_str()).map_err(|_| DirectStoreErrorV1::Unreadable)?;
        let invalid = || DirectStoreErrorV1::InvalidInput {
            reason: "invalid maintenance marker identity",
        };
        let string = |key| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(invalid)
        };
        if string("schema")? != kind.schema() {
            return Err(invalid());
        }
        for field in ["operation_id", "response_request_id"] {
            WorkspaceId::new(string(field)?.to_owned()).map_err(|_| invalid())?;
        }
        if !value
            .get("workspace_uuid")
            .is_some_and(serde_json::Value::is_null)
        {
            WorkspaceId::new(string("workspace_uuid")?.to_owned()).map_err(|_| invalid())?;
        }
        IdempotencyKeyV1::new(string("idempotency_key")?.to_owned()).map_err(|_| invalid())?;
        for field in ["request_digest", "contract_manifest_digest"] {
            Sha256Digest::new(string(field)?.to_owned()).map_err(|_| invalid())?;
        }
        let command = string("command")?;
        if !matches!(
            (kind, command),
            (
                DirectMaintenanceMarkerKindV1::Intent,
                "workspace.init" | "workspace.reset_all" | "workspace.remove" | "workspace.migrate"
            ) | (DirectMaintenanceMarkerKindV1::Reset, "workspace.reset_all")
                | (DirectMaintenanceMarkerKindV1::Removal, "workspace.remove")
        ) {
            return Err(invalid());
        }
        let root = value
            .get("root_identity")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(invalid)?;
        for field in ["common_dir_identity", "worktree_admin_identity"] {
            let fingerprint = root
                .get(field)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(invalid)?;
            Sha256Digest::new(fingerprint.to_owned()).map_err(|_| invalid())?;
        }
        let bytes = root
            .get("root_path_bytes_base64url")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(invalid)?;
        if !(2..=5462).contains(&bytes.len())
            || !bytes
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(invalid());
        }
        string("created_at")?;
        if kind != DirectMaintenanceMarkerKindV1::Intent {
            let phase = string("phase")?;
            if !matches!(
                (kind, phase),
                (
                    DirectMaintenanceMarkerKindV1::Reset,
                    "prepared" | "runtime_removed" | "store_created"
                ) | (
                    DirectMaintenanceMarkerKindV1::Removal,
                    "prepared" | "runtime_removed" | "configuration_removed"
                )
            ) {
                return Err(invalid());
            }
        }
        Ok(Self { kind, document })
    }
    pub fn kind(&self) -> DirectMaintenanceMarkerKindV1 {
        self.kind
    }
    pub fn document(&self) -> &CanonicalExecutionJsonV1 {
        &self.document
    }
}

impl ExclusiveMaintenanceLeaseV1 {
    pub fn read_marker(
        &self,
        layout: &DirectWorkspaceLayoutV1,
        kind: DirectMaintenanceMarkerKindV1,
    ) -> DirectStoreResultV1<Option<DirectMaintenanceMarkerV1>> {
        self.revalidate(layout)?;
        let file = match super::classification::open_read_nofollow(&kind.path(layout)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(1_048_577).read_to_end(&mut bytes)?;
        let text = String::from_utf8(bytes).map_err(|_| DirectStoreErrorV1::Unreadable)?;
        let document =
            CanonicalExecutionJsonV1::new(text).map_err(|_| DirectStoreErrorV1::Unreadable)?;
        self.revalidate(layout)?;
        Ok(Some(DirectMaintenanceMarkerV1::new(kind, document)?))
    }

    /// No-clobber creation or exact-content replacement, always under the native lease.
    pub fn write_marker(
        &self,
        layout: &DirectWorkspaceLayoutV1,
        next: &DirectMaintenanceMarkerV1,
        expected: Option<&DirectMaintenanceMarkerV1>,
    ) -> DirectStoreResultV1<()> {
        self.revalidate(layout)?;
        if self.read_marker(layout, next.kind())?.as_ref() != expected {
            return Err(DirectStoreErrorV1::Maintenance);
        }
        let mut temporary = tempfile::Builder::new()
            .prefix(".maintenance-marker-")
            .tempfile_in(layout.runtime_directory())?;
        temporary.write_all(next.document.as_str().as_bytes())?;
        temporary.as_file().sync_all()?;
        self.revalidate(layout)?;
        if expected.is_none() {
            temporary
                .persist_noclobber(next.kind.path(layout))
                .map_err(|error| DirectStoreErrorV1::Storage(error.error))?;
        } else {
            temporary
                .persist(next.kind.path(layout))
                .map_err(|error| DirectStoreErrorV1::Storage(error.error))?;
        }
        super::maintenance::open_directory(&layout.runtime_directory())?.sync_all()?;
        Ok(())
    }

    pub fn remove_marker(
        &self,
        layout: &DirectWorkspaceLayoutV1,
        expected: &DirectMaintenanceMarkerV1,
    ) -> DirectStoreResultV1<()> {
        self.revalidate(layout)?;
        if self.read_marker(layout, expected.kind())?.as_ref() != Some(expected) {
            return Err(DirectStoreErrorV1::Maintenance);
        }
        std::fs::remove_file(expected.kind.path(layout))?;
        super::maintenance::open_directory(&layout.runtime_directory())?.sync_all()?;
        Ok(())
    }
}

/// Observe a published marker without creating locks, opening SQLite or resuming work.
/// This is recovery guidance only; mutation authority still requires an exclusive lease.
pub fn observe_direct_maintenance_marker_v1(
    layout: &DirectWorkspaceLayoutV1,
) -> DirectStoreResultV1<Option<DirectMaintenanceMarkerV1>> {
    let directories = super::classification::ClassificationDirectories::open(layout)?;
    let result = (|| {
        for kind in [
            DirectMaintenanceMarkerKindV1::Removal,
            DirectMaintenanceMarkerKindV1::Reset,
            DirectMaintenanceMarkerKindV1::Intent,
        ] {
            let file = match super::classification::open_read_nofollow(&kind.path(layout)) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let mut bytes = Vec::new();
            file.take(1_048_577).read_to_end(&mut bytes)?;
            let text = String::from_utf8(bytes).map_err(|_| DirectStoreErrorV1::Unreadable)?;
            let document =
                CanonicalExecutionJsonV1::new(text).map_err(|_| DirectStoreErrorV1::Unreadable)?;
            return Ok(Some(DirectMaintenanceMarkerV1::new(kind, document)?));
        }
        Ok(None)
    })();
    directories.revalidate(layout)?;
    result
}
