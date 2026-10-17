//! Scoped query and retention accounting for READS data-access characterization.
//!
//! The helpers below are always compiled. Without the `test-support` feature
//! (and outside this crate's own unit-test build) they are inline
//! pass-throughs to the identical rusqlite calls with the same SQL text and
//! arguments, so release builds carry no accounting state and no behavior
//! change. When enabled they count, on the registered
//! read/preflight/prepared-admission routes only: one prepare attempt per
//! statement compilation (including the implicit prepare inside
//! `Connection::query_row`), one execution initiation per `query_row`,
//! `query_map` or statement `query` call, and one delivered row per row
//! mapper invocation, including rows whose conversion subsequently fails.
//! They also track logical item-slot payload retention inside the slot-family
//! loader. Accounting never changes SQL, argument order, results or errors,
//! and every accounting operation is infallible.
//!
//! These are scoped measurements over registered call sites, not a total SQL
//! counter: open-time schema inventory, open admission PRAGMAs, integrity
//! verification, maintenance and mutation-transaction reads stay unrouted and
//! are registered as uncovered in the READS qualification documentation.

#[inline(always)]
pub(crate) fn accounted_prepare<'a>(
    connection: &'a rusqlite::Connection,
    sql: &str,
) -> rusqlite::Result<rusqlite::Statement<'a>> {
    #[cfg(any(test, feature = "test-support"))]
    ledger::record_prepare(sql);
    connection.prepare(sql)
}

#[inline(always)]
pub(crate) fn accounted_query_row<T, P, F>(
    connection: &rusqlite::Connection,
    sql: &str,
    params: P,
    mapper: F,
) -> rusqlite::Result<T>
where
    P: rusqlite::Params,
    F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    #[cfg(any(test, feature = "test-support"))]
    {
        ledger::record_prepare(sql);
        ledger::record_execute(sql);
    }
    let counting = |row: &rusqlite::Row<'_>| {
        #[cfg(any(test, feature = "test-support"))]
        ledger::observe_delivered_row(sql, row);
        mapper(row)
    };
    connection.query_row(sql, params, counting)
}

/// Counts one execution initiation (the statement was already counted as
/// prepared by [`accounted_prepare`]) plus one delivered row per mapper call.
#[inline(always)]
#[cfg_attr(not(any(test, feature = "test-support")), allow(unused_variables))]
pub(crate) fn accounted_query_map_prepared<'stmt, 'sql, T, P, F>(
    statement: &'stmt mut rusqlite::Statement<'_>,
    sql: &'sql str,
    params: P,
    mut mapper: F,
) -> rusqlite::Result<
    rusqlite::MappedRows<
        'stmt,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T> + use<'stmt, 'sql, T, P, F>,
    >,
>
where
    P: rusqlite::Params,
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    #[cfg(any(test, feature = "test-support"))]
    ledger::record_execute(sql);
    let counting = move |row: &rusqlite::Row<'_>| {
        #[cfg(any(test, feature = "test-support"))]
        ledger::observe_delivered_row(sql, row);
        mapper(row)
    };
    statement.query_map(params, counting)
}

/// Guards one slot-family loader invocation. Closing the window is exact even
/// on error paths: raw slot payload strings are local to the loader call, so
/// any bytes not yet released are released at scope end, and decoded payloads
/// of an uncommitted (failed) scope are dropped with the partially built
/// result. A committed scope's decoded payloads stay counted as retained in
/// the caller's partially built result.
#[cfg(any(test, feature = "test-support"))]
pub(crate) struct RawSlotScope;

#[cfg(any(test, feature = "test-support"))]
impl RawSlotScope {
    #[inline]
    pub(crate) fn enter() -> Self {
        ledger::raw_scope_enter();
        Self
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for RawSlotScope {
    fn drop(&mut self) {
        ledger::raw_scope_exit();
    }
}

/// Guards one whole graph reconstruction. Committed slot-scope payloads
/// transfer into the returned graph; if the outer reconstruction fails, only
/// its own contribution above the entry baseline is rolled back, and
/// balances owned by earlier returned graphs stay counted.
#[cfg(any(test, feature = "test-support"))]
pub(crate) struct GraphLoadScope;

#[cfg(any(test, feature = "test-support"))]
impl GraphLoadScope {
    #[inline]
    pub(crate) fn enter() -> Self {
        ledger::graph_scope_enter();
        Self
    }
    /// Marks the guarded reconstruction as successfully returned.
    #[inline]
    pub(crate) fn commit(&self) {
        ledger::graph_scope_commit();
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for GraphLoadScope {
    fn drop(&mut self) {
        ledger::graph_scope_exit();
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) mod ledger {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(crate) struct SqlCounts {
        pub prepares: u64,
        pub executes: u64,
        pub rows: u64,
        /// Variable-length (TEXT/BLOB) column bytes delivered through row
        /// mappers on this SQL text. A per-family raw delivery measure,
        /// distinct from the slot payload-decode retention units.
        pub column_bytes: u64,
    }

    impl SqlCounts {
        fn add_prepare(&mut self) {
            self.prepares = self.prepares.saturating_add(1);
        }
        fn add_execute(&mut self) {
            self.executes = self.executes.saturating_add(1);
        }
        fn add_row(&mut self) {
            self.rows = self.rows.saturating_add(1);
        }
    }

    /// Logical, deterministic retention units. These are not allocator or
    /// heap measurements: they count serialized payload byte lengths and
    /// payload pointer identities observed at the loader boundaries.
    #[derive(Clone, Debug)]
    pub(crate) struct RetentionState {
        pub slot_scopes: u64,
        pub raw_current: u64,
        pub raw_scope_peak: u64,
        pub raw_window_peak: u64,
        /// Bytes still unreleased when a scope ended; nonzero only on the
        /// error/early-return path, proving exact release accounting.
        pub raw_released_at_scope_end: u64,
        /// Cumulative fresh serialized payload bytes decoded in this window.
        pub fresh_bytes_cumulative: u64,
        /// Freshly decoded payload bytes held in graphs this window has
        /// successfully reconstructed (or still-building partial results).
        /// Falls when a slot scope or whole graph reconstruction fails;
        /// later caller-side drops of returned graphs are not observed.
        pub decoded_live: u64,
        pub decoded_live_peak: u64,
        /// Fresh bytes dropped with failed scopes' partial results.
        pub decoded_released_at_scope_end: u64,
        /// Fresh bytes added by the innermost open scope.
        scope_fresh_bytes: u64,
        scope_committed: bool,
        /// Whole-graph reconstruction guards in this window.
        pub graph_scopes: u64,
        graph_scope_committed: bool,
        /// Live decoded balance owned by earlier results when the current
        /// graph scope opened; a failed reconstruction rolls back only its
        /// own contribution above this baseline.
        graph_scope_baseline: u64,
        /// Slot payload decode attempts, successful or failed.
        pub decode_attempts: u64,
        pub fresh_payloads: u64,
        pub reused_payloads: u64,
        pub reused_bytes: u64,
        pub unknown_reuse_payloads: u64,
        pub container_copies: u64,
        /// Measured payload bytes of the caller-supplied cached input graph,
        /// declared by the test before the measured call.
        pub cached_input_bytes: u64,
        pub combined_peak: u64,
    }

    impl Default for RetentionState {
        fn default() -> Self {
            Self {
                slot_scopes: 0,
                raw_current: 0,
                raw_scope_peak: 0,
                raw_window_peak: 0,
                raw_released_at_scope_end: 0,
                fresh_bytes_cumulative: 0,
                decoded_live: 0,
                decoded_live_peak: 0,
                decoded_released_at_scope_end: 0,
                scope_fresh_bytes: 0,
                scope_committed: false,
                graph_scopes: 0,
                graph_scope_committed: false,
                graph_scope_baseline: 0,
                decode_attempts: 0,
                fresh_payloads: 0,
                reused_payloads: 0,
                reused_bytes: 0,
                unknown_reuse_payloads: 0,
                container_copies: 0,
                cached_input_bytes: 0,
                combined_peak: 0,
            }
        }
    }

    impl RetentionState {
        fn probe_combined(&mut self) {
            let combined = self
                .raw_current
                .saturating_add(self.decoded_live)
                .saturating_add(self.cached_input_bytes);
            self.combined_peak = self.combined_peak.max(combined);
        }
    }

    thread_local! {
        static PAYLOAD_REGISTRY: RefCell<BTreeMap<usize, usize>> =
            const { RefCell::new(BTreeMap::new()) };
    }

    struct Ledger {
        sql: BTreeMap<String, SqlCounts>,
        totals: SqlCounts,
        retention: RetentionState,
    }

    thread_local! {
        static LEDGER: RefCell<Option<Ledger>> = const { RefCell::new(None) };
    }

    fn with_ledger<R>(visit: impl FnOnce(&mut Ledger) -> R) -> Option<R> {
        LEDGER.with(|ledger| {
            let mut borrow = ledger.borrow_mut();
            borrow.as_mut().map(visit)
        })
    }

    pub(crate) fn record_prepare(sql: &str) {
        with_ledger(|ledger| {
            ledger.sql.entry(sql.to_owned()).or_default().add_prepare();
            ledger.totals.add_prepare();
        });
    }

    pub(crate) fn record_execute(sql: &str) {
        with_ledger(|ledger| {
            ledger.sql.entry(sql.to_owned()).or_default().add_execute();
            ledger.totals.add_execute();
        });
    }

    /// Observes one delivered row at mapper entry: counts the row and the
    /// variable-length column bytes it carries, borrowing each column value
    /// without decoding, copying or erroring. The counted unit is TEXT/BLOB
    /// bytes only; fixed-width scalars and NULLs count zero payload bytes.
    pub(crate) fn observe_delivered_row(sql: &str, row: &rusqlite::Row<'_>) {
        let mut delivered_bytes = 0u64;
        for index in 0..row.as_ref().column_count() {
            if let Ok(value) = row.get_ref(index) {
                let bytes = match value {
                    rusqlite::types::ValueRef::Text(text) => text.len() as u64,
                    rusqlite::types::ValueRef::Blob(blob) => blob.len() as u64,
                    _ => 0,
                };
                delivered_bytes = delivered_bytes.saturating_add(bytes);
            }
        }
        with_ledger(|ledger| {
            let counts = ledger.sql.entry(sql.to_owned()).or_default();
            counts.add_row();
            counts.column_bytes = counts.column_bytes.saturating_add(delivered_bytes);
            ledger.totals.add_row();
            ledger.totals.column_bytes = ledger.totals.column_bytes.saturating_add(delivered_bytes);
        });
    }

    pub(crate) fn raw_scope_enter() {
        with_ledger(|ledger| {
            let retention = &mut ledger.retention;
            retention.slot_scopes += 1;
            retention.scope_fresh_bytes = 0;
            retention.scope_committed = false;
        });
    }

    pub(crate) fn graph_scope_enter() {
        with_ledger(|ledger| {
            let retention = &mut ledger.retention;
            retention.graph_scopes += 1;
            // Snapshot the balance owned by earlier results so this scope's
            // rollback cannot release their bytes.
            retention.graph_scope_baseline = retention.decoded_live;
        });
    }

    pub(crate) fn graph_scope_commit() {
        with_ledger(|ledger| {
            ledger.retention.graph_scope_committed = true;
        });
    }

    /// On a failed outer reconstruction only this graph's contribution —
    /// its committed slot bytes above the entry baseline, after the failing
    /// attempt's own slot-scope rollback already ran — is released; balances
    /// owned by earlier returned graphs stay counted.
    pub(crate) fn graph_scope_exit() {
        with_ledger(|ledger| {
            let retention = &mut ledger.retention;
            if !retention.graph_scope_committed {
                let dropped = retention
                    .decoded_live
                    .saturating_sub(retention.graph_scope_baseline);
                retention.decoded_released_at_scope_end = retention
                    .decoded_released_at_scope_end
                    .saturating_add(dropped);
                retention.decoded_live = retention.graph_scope_baseline;
                retention.probe_combined();
            }
            retention.graph_scope_baseline = 0;
            retention.graph_scope_committed = false;
        });
    }

    /// Marks the innermost scope as committed: its decoded payloads transfer
    /// into the caller-owned live result and stay counted as retained.
    pub(crate) fn commit_slot_scope() {
        with_ledger(|ledger| {
            ledger.retention.scope_committed = true;
        });
    }

    pub(crate) fn raw_scope_exit() {
        with_ledger(|ledger| {
            let retention = &mut ledger.retention;
            retention.raw_released_at_scope_end = retention
                .raw_released_at_scope_end
                .saturating_add(retention.raw_current);
            retention.raw_current = 0;
            if !retention.scope_committed {
                let dropped = retention.scope_fresh_bytes;
                retention.decoded_live = retention.decoded_live.saturating_sub(dropped);
                retention.decoded_released_at_scope_end = retention
                    .decoded_released_at_scope_end
                    .saturating_add(dropped);
                retention.probe_combined();
            }
            retention.scope_fresh_bytes = 0;
            retention.scope_committed = false;
        });
    }

    pub(crate) fn acquire_raw(len: usize) {
        with_ledger(|ledger| {
            let retention = &mut ledger.retention;
            retention.raw_current = retention.raw_current.saturating_add(len as u64);
            retention.raw_scope_peak = retention.raw_scope_peak.max(retention.raw_current);
            retention.raw_window_peak = retention.raw_window_peak.max(retention.raw_current);
            retention.probe_combined();
        });
    }

    pub(crate) fn release_raw(len: usize) {
        with_ledger(|ledger| {
            let retention = &mut ledger.retention;
            retention.raw_current = retention.raw_current.saturating_sub(len as u64);
            retention.probe_combined();
        });
    }

    /// Counts one slot payload decode attempt, successful or not.
    pub(crate) fn record_decode_attempt() {
        with_ledger(|ledger| {
            ledger.retention.decode_attempts += 1;
        });
    }

    /// Accounts a freshly decoded payload's logical serialized bytes while
    /// its raw string is still alive, so the combined peak observes the true
    /// raw+decoded overlap. Byte accounting is separate from pointer
    /// registration.
    pub(crate) fn record_decoded_bytes(len: usize) {
        with_ledger(|ledger| {
            let retention = &mut ledger.retention;
            retention.fresh_bytes_cumulative =
                retention.fresh_bytes_cumulative.saturating_add(len as u64);
            retention.decoded_live = retention.decoded_live.saturating_add(len as u64);
            retention.decoded_live_peak = retention.decoded_live_peak.max(retention.decoded_live);
            retention.scope_fresh_bytes = retention.scope_fresh_bytes.saturating_add(len as u64);
            retention.fresh_payloads += 1;
            retention.probe_combined();
        });
    }

    /// Registers a constructed slot payload's identity and logical size for
    /// later reuse accounting. Adds no bytes. Only meaningful inside an
    /// accounting window; entries stay valid only while the graphs that
    /// produced them remain alive, so scenarios clear the registry between
    /// independent measurements.
    pub(crate) fn register_payload_pointer(pointer: usize, len: usize) {
        let window_active = LEDGER.with(|ledger| ledger.borrow().is_some());
        if !window_active {
            return;
        }
        PAYLOAD_REGISTRY.with(|registry| {
            registry.borrow_mut().insert(pointer, len);
        });
    }

    /// Drops every registered payload identity; scenarios call this before
    /// measuring with fresh fixture graphs.
    pub(crate) fn clear_payload_registry() {
        PAYLOAD_REGISTRY.with(|registry| registry.borrow_mut().clear());
    }

    /// Records reuse of an already-decoded payload through pointer identity.
    pub(crate) fn record_reuse(pointer: usize) {
        let known = PAYLOAD_REGISTRY.with(|registry| registry.borrow().get(&pointer).copied());
        with_ledger(|ledger| {
            let retention = &mut ledger.retention;
            match known {
                Some(len) => {
                    retention.reused_payloads += 1;
                    retention.reused_bytes = retention.reused_bytes.saturating_add(len as u64);
                }
                None => retention.unknown_reuse_payloads += 1,
            }
        });
    }

    pub(crate) fn record_container_copy() {
        with_ledger(|ledger| {
            ledger.retention.container_copies += 1;
        });
    }

    pub(crate) fn declare_cached_input_bytes(bytes: u64) {
        with_ledger(|ledger| {
            ledger.retention.cached_input_bytes = bytes;
            ledger.retention.probe_combined();
        });
    }

    pub(crate) fn lookup_payload_bytes(pointer: usize) -> Option<usize> {
        PAYLOAD_REGISTRY.with(|registry| registry.borrow().get(&pointer).copied())
    }

    pub(crate) fn start() {
        LEDGER.with(|ledger| {
            *ledger.borrow_mut() = Some(Ledger {
                sql: BTreeMap::new(),
                totals: SqlCounts::default(),
                retention: RetentionState::default(),
            });
        });
    }

    pub(crate) fn finish() -> Option<(BTreeMap<String, SqlCounts>, SqlCounts, RetentionState)> {
        LEDGER.with(|ledger| {
            ledger
                .borrow_mut()
                .take()
                .map(|finished| (finished.sql, finished.totals, finished.retention))
        })
    }

    pub(crate) fn payload_pointer(slot: &crate::ItemSlotStateV2) -> Option<usize> {
        slot.value()
            .map(|value| value as *const podway_core::RecordedItemValueV2 as usize)
    }
}
