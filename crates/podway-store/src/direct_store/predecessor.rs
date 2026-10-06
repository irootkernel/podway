use std::time::Instant;

use podway_core::UnixMillis;
use rusqlite::{Transaction, TransactionBehavior, params};

use super::{DirectStoreErrorV1, DirectStoreResultV1, DirectStoreV1, ExclusiveMaintenanceLeaseV1};

/// Release/open-development commits alone add entries and their reviewed migration.
/// This feature commit intentionally declares no production predecessor.
struct DeclaredPredecessor<'contract> {
    manifest: &'contract str,
    generation: u32,
    ddl: &'contract str,
    migration_id: &'static str,
    migrate: fn(&Transaction<'_>) -> DirectStoreResultV1<()>,
}

const DECLARED_PREDECESSORS: &[DeclaredPredecessor<'static>] = &[];

pub fn writer_predecessor_manifests_v1() -> impl Iterator<Item = &'static str> {
    DECLARED_PREDECESSORS.iter().map(|entry| entry.manifest)
}

impl DirectStoreV1 {
    /// Forward migration runs only under an exclusive lease, after a declared
    /// predecessor's coherent read-only identity/schema check. Unknown writers
    /// never reach a read-write open. Engine owns its persistent intent/resumption.
    pub fn migrate_declared_writer(
        &self,
        deadline: Instant,
        clock: impl FnOnce() -> UnixMillis,
    ) -> DirectStoreResultV1<bool> {
        let lease = ExclusiveMaintenanceLeaseV1::acquire(&self.layout, deadline, false)?;
        let _connection_gate = super::connection::connection_gate(deadline)?;
        self.migrate_writer_with_declarations(&lease, deadline, clock, DECLARED_PREDECESSORS)
    }

    fn migrate_writer_with_declarations(
        &self,
        lease: &ExclusiveMaintenanceLeaseV1,
        deadline: Instant,
        clock: impl FnOnce() -> UnixMillis,
        predecessors: &[DeclaredPredecessor<'_>],
    ) -> DirectStoreResultV1<bool> {
        lease.revalidate(&self.layout)?;
        let classification = super::classification::classify_unmarked_store_v1(&self.layout)?;
        match classification {
            super::DirectStoreClassificationV1::Current => {}
            super::DirectStoreClassificationV1::Legacy {
                generation: Some(generation),
                marker: None,
            } if generation >= 14
                && predecessors
                    .iter()
                    .any(|entry| entry.generation == generation) => {}
            classification => classification.require_current()?,
        }
        let database = super::classification::open_read_nofollow(&self.layout.database())?;
        let mut readonly = self.open_readonly_connection(
            deadline,
            !super::classification::path_exists(&self.layout.shm())?,
        )?;
        let snapshot = readonly.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let actual: String = snapshot.query_row(
            "SELECT contract_manifest_digest FROM writer_contract WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        if actual == self.manifest.as_str() {
            self.validate_connection(&snapshot)?;
            super::connection::ensure_deadline(deadline)?;
            lease.revalidate(&self.layout)?;
            if !super::maintenance::same_file(&database, &self.layout.database())? {
                return Err(DirectStoreErrorV1::Maintenance);
            }
            return Ok(false);
        }
        let predecessor = predecessors
            .iter()
            .find(|entry| entry.manifest == actual)
            .ok_or_else(|| DirectStoreErrorV1::WriterMismatch {
                expected: self.manifest.clone(),
                actual: actual.clone(),
            })?;
        self.validate_predecessor(&snapshot, predecessor)?;
        drop(snapshot);
        drop(readonly);
        lease.revalidate(&self.layout)?;
        if !super::maintenance::same_file(&database, &self.layout.database())? {
            return Err(DirectStoreErrorV1::Maintenance);
        }
        let mut connection = self.open_connection(false, deadline)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.validate_predecessor(&transaction, predecessor)?;
        lease.revalidate(&self.layout)?;
        if !super::maintenance::same_file(&database, &self.layout.database())? {
            return Err(DirectStoreErrorV1::Maintenance);
        }
        let now = i64::try_from(clock().get()).map_err(|_| DirectStoreErrorV1::InvalidInput {
            reason: "migration clock exceeds SQLite range",
        })?;
        (predecessor.migrate)(&transaction)?;
        transaction.pragma_update(None, "user_version", super::DIRECT_SCHEMA_GENERATION_V1)?;
        transaction.execute(
            "UPDATE workspace_state SET last_validated_root=?1,updated_at_ms=?2 WHERE singleton=1",
            params![self.layout.root().as_encoded(), now],
        )?;
        transaction.execute("UPDATE writer_contract SET contract_manifest_digest=?1,recorded_at_ms=?2 WHERE singleton=1 AND contract_manifest_digest=?3",
            params![self.manifest.as_str(),now,predecessor.manifest])?;
        transaction.execute("INSERT INTO operational_journal(recorded_at_ms,level,event_name,summary) VALUES(?1,'info','writer_migrated',?2)",
            params![now,predecessor.migration_id])?;
        self.validate_connection(&transaction)?;
        super::connection::ensure_deadline(deadline)?;
        lease.revalidate(&self.layout)?;
        if !super::maintenance::same_file(&database, &self.layout.database())? {
            return Err(DirectStoreErrorV1::Maintenance);
        }
        // Engine retries its durable migration intent and checks writer identity;
        // this internal maintenance step is not a public mutation outcome.
        transaction.commit()?;
        Ok(true)
    }

    fn validate_predecessor(
        &self,
        connection: &rusqlite::Connection,
        predecessor: &DeclaredPredecessor<'_>,
    ) -> DirectStoreResultV1<()> {
        if predecessor.generation < 14
            || predecessor.generation > super::DIRECT_SCHEMA_GENERATION_V1
        {
            return Err(DirectStoreErrorV1::SchemaMismatch);
        }
        let generation: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let mode: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        let (manifest,id,time):(String,String,i64)=connection.query_row(
            "SELECT contract_manifest_digest,generation_id,recorded_at_ms FROM writer_contract WHERE singleton=1",[],
            |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
        if generation != predecessor.generation
            || mode != "wal"
            || manifest != predecessor.manifest
            || id.is_empty()
            || id.len() > 64
            || time < 0
        {
            return Err(DirectStoreErrorV1::SchemaMismatch);
        }
        super::connection::verify_declared_schema(connection, predecessor.ddl)?;
        let binding = super::connection::read_binding(connection)?;
        if binding.identity().common_dir_identity() != &self.common
            || binding.identity().worktree_admin_identity() != &self.admin
        {
            return Err(DirectStoreErrorV1::BindingMismatch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CanonicalExecutionJsonV1, IdempotencyKeyV1, ValidatedWorkspaceRootV1};
    use podway_core::{Sha256Digest, WorkspaceId, canonicalize_json_v1};
    use serde_json::json;

    const OLD: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    fn deadline() -> Instant {
        Instant::now() + std::time::Duration::from_secs(5)
    }
    fn digest(byte: char) -> Sha256Digest {
        Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
    }
    fn migration(transaction: &Transaction<'_>) -> DirectStoreResultV1<()> {
        transaction.execute_batch("DROP TABLE predecessor_only;")?;
        Ok(())
    }
    fn rejected_migration(transaction: &Transaction<'_>) -> DirectStoreResultV1<()> {
        transaction.execute_batch("DROP TABLE predecessor_only;")?;
        Err(DirectStoreErrorV1::InvalidInput {
            reason: "test migration rejection",
        })
    }

    #[test]
    fn empty_production_registry_and_declared_schema_migration_are_atomic() {
        assert_eq!(writer_predecessor_manifests_v1().count(), 0);
        let root = tempfile::TempDir::new().unwrap();
        let layout = super::super::DirectWorkspaceLayoutV1::new(
            ValidatedWorkspaceRootV1::from_path(root.path()).unwrap(),
        )
        .unwrap();
        let old = DirectStoreV1::new(
            layout.clone(),
            Sha256Digest::new(OLD).unwrap(),
            digest('a'),
            digest('b'),
        );
        let request = super::super::DirectMutationRequestV1::new(
            IdempotencyKeyV1::new("init").unwrap(),
            digest('d'),
            "workspace.init",
            "00000000-0000-4000-8000-000000000002",
            super::super::DirectOutcomeScopeV1::Workspace,
        )
        .unwrap();
        let lease = ExclusiveMaintenanceLeaseV1::acquire(&layout, deadline(), true).unwrap();
        old.bootstrap(&lease,WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap(),"migration-test",&request,deadline(),
            ||UnixMillis::new(10),|_|Ok(CanonicalExecutionJsonV1::new(canonicalize_json_v1(&json!({
                "schema":"podway.output/v4","request_id":request.response_id(),"command":"workspace.init",
                "generated_at":"1970-01-01T00:00:00.010Z","result":{"initialized":true},"warnings":[]})).unwrap()).unwrap())).unwrap();
        drop(lease);
        // Construct a genuine differing predecessor inventory in this isolated fixture.
        let connection = old.open_connection(false, deadline()).unwrap();
        connection
            .execute_batch("CREATE TABLE predecessor_only(value TEXT);")
            .unwrap();
        drop(connection);
        let new = DirectStoreV1::new(layout.clone(), digest('e'), digest('a'), digest('b'));
        let before_main = std::fs::read(layout.database()).unwrap();
        let before_wal = std::fs::read(layout.wal()).unwrap();
        assert!(matches!(
            new.migrate_declared_writer(deadline(), || panic!("unknown predecessor has no clock")),
            Err(DirectStoreErrorV1::WriterMismatch { .. })
        ));
        assert_eq!(std::fs::read(layout.database()).unwrap(), before_main);
        assert_eq!(std::fs::read(layout.wal()).unwrap(), before_wal);
        let ddl = format!(
            "{}\nCREATE TABLE predecessor_only(value TEXT);",
            super::super::DIRECT_STORE_DDL_V1
        );
        let lease = ExclusiveMaintenanceLeaseV1::acquire(&layout, deadline(), false).unwrap();
        let _gate = super::super::connection::connection_gate(deadline()).unwrap();
        let rejected = DeclaredPredecessor {
            manifest: OLD,
            generation: 14,
            ddl: &ddl,
            migration_id: "test-rejection",
            migrate: rejected_migration,
        };
        assert!(matches!(
            new.migrate_writer_with_declarations(
                &lease,
                deadline(),
                || UnixMillis::new(20),
                &[rejected]
            ),
            Err(DirectStoreErrorV1::InvalidInput { .. })
        ));
        let accepted = DeclaredPredecessor {
            manifest: OLD,
            generation: 14,
            ddl: &ddl,
            migration_id: "test-forward",
            migrate: migration,
        };
        let mut clocks = 0;
        assert!(
            new.migrate_writer_with_declarations(
                &lease,
                deadline(),
                || {
                    clocks += 1;
                    UnixMillis::new(30)
                },
                &[accepted]
            )
            .unwrap()
        );
        assert_eq!(clocks, 1);
        assert!(
            !new.migrate_writer_with_declarations(
                &lease,
                deadline(),
                || panic!("current writer has no migration clock"),
                &[]
            )
            .unwrap()
        );
        drop(_gate);
        drop(lease);
        new.verify_integrity(deadline()).unwrap();
    }
}
