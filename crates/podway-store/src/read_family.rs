//! Ordered acquisition shared by the existing Procedure v2 validators.
//!
//! Statements and their lazy cursors live in the caller's stack frame. A
//! boundary peek reads only integer routing keys; conversion and domain
//! validation happen in the owning loader's callback. TooBig retires only
//! acquisition, so the same callback can use the original owner selector.

use std::cell::LazyCell;

use rusqlite::fallible_streaming_iterator::FallibleStreamingIterator;
use rusqlite::{Connection, Params, Row, Rows, Statement};

use crate::query_accounting::{
    accounted_delivered_row, accounted_prepare, accounted_query_prepared,
};
use crate::{RusqliteErrorContextV1, StoreErrorV1, StoreRecordKindV1, map_rusqlite_error_v1};

pub(crate) struct FamilyCursor<'stmt, K> {
    rows: Option<Rows<'stmt>>,
    sql: &'static str,
    key: fn(&Row<'_>) -> rusqlite::Result<K>,
    record: StoreRecordKindV1,
    pending: Option<K>,
    done: bool,
}

impl<'stmt, K: Copy + PartialEq> FamilyCursor<'stmt, K> {
    pub(crate) fn new(
        rows: Rows<'stmt>,
        sql: &'static str,
        key: fn(&Row<'_>) -> rusqlite::Result<K>,
        record: StoreRecordKindV1,
    ) -> Self {
        Self {
            rows: Some(rows),
            sql,
            key,
            record,
            pending: None,
            done: false,
        }
    }

    fn error(&self, error: rusqlite::Error) -> StoreErrorV1 {
        map_rusqlite_error_v1(error, RusqliteErrorContextV1::Record(self.record.clone()))
    }

    fn collect<T>(
        &mut self,
        owner: K,
        mut convert: impl FnMut(&Row<'_>) -> Result<T, StoreErrorV1>,
    ) -> Result<Option<Vec<T>>, StoreErrorV1> {
        if self.rows.is_none() {
            return Ok(None);
        }
        let mut values = Vec::new();
        while !self.done {
            if self.pending.is_none() {
                let rows = self.rows.as_mut().expect("live family cursor");
                if let Err(error) = rows.advance() {
                    if matches!(&error, rusqlite::Error::SqliteFailure(code, _)
                        if code.code == rusqlite::ErrorCode::TooBig)
                    {
                        // The failing step produced no Row. Its physical owner
                        // is unknown; discard this unconfirmed collection and
                        // restart acquisition at the current conservative owner.
                        drop(self.rows.take());
                        return Ok(None);
                    }
                    return Err(self.error(error));
                }
                if let Some(row) = rows.get() {
                    accounted_delivered_row(self.sql, row);
                    self.pending = Some((self.key)(row).map_err(|error| self.error(error))?);
                } else {
                    self.done = true;
                    break;
                }
            }
            if self.pending != Some(owner) {
                break;
            }
            let row = self
                .rows
                .as_ref()
                .and_then(Rows::get)
                .expect("pending family row");
            values.push(convert(row)?);
            self.pending = None;
        }
        Ok(Some(values))
    }
}

/// `None` means this family's bulk cursor has retired. Callers discard any
/// callback-owned partial collection (booking releases) before owner acquisition.
pub(crate) trait ReadFamily<K> {
    fn collect<T>(
        &mut self,
        owner: K,
        convert: impl FnMut(&Row<'_>) -> Result<T, StoreErrorV1>,
    ) -> Result<Option<Vec<T>>, StoreErrorV1>;
}

impl<'stmt, K, F> ReadFamily<K> for LazyCell<Result<FamilyCursor<'stmt, K>, StoreErrorV1>, F>
where
    K: Copy + PartialEq,
    F: FnOnce() -> Result<FamilyCursor<'stmt, K>, StoreErrorV1>,
{
    fn collect<T>(
        &mut self,
        owner: K,
        convert: impl FnMut(&Row<'_>) -> Result<T, StoreErrorV1>,
    ) -> Result<Option<Vec<T>>, StoreErrorV1> {
        LazyCell::force_mut(self)
            .as_mut()
            .map_err(|error| error.clone())?
            .collect(owner, convert)
    }
}

pub(crate) fn collect_owner<T>(
    connection: &Connection,
    sql: &str,
    params: impl Params,
    record: StoreRecordKindV1,
    mut convert: impl FnMut(&Row<'_>) -> Result<T, StoreErrorV1>,
) -> Result<Vec<T>, StoreErrorV1> {
    let error =
        |error| map_rusqlite_error_v1(error, RusqliteErrorContextV1::Record(record.clone()));
    let mut statement = accounted_prepare(connection, sql).map_err(error)?;
    let mut rows = accounted_query_prepared(&mut statement, sql, params).map_err(error)?;
    let mut values = Vec::new();
    while let Some(row) = rows.next().map_err(error)? {
        accounted_delivered_row(sql, row);
        values.push(convert(row)?);
    }
    Ok(values)
}

/// Fixed 128-pair changed-slot chunk statement: exactly 256 numbered
/// identity binds (attempt UUID + rowid per pair) and zero other
/// parameters. Unused tail pairs are bound as NULL, which can never match
/// the NOT NULL identity columns. The metadata-only ordered subquery with
/// `LIMIT -1` forces the planner to sort identities (ord, sequence, UUID,
/// rowid) in an ephemeral record before `SeekRowid` reads payload columns,
/// so no payload-bearing sorter or repacking exists on the pinned plan.
pub(crate) const CHUNK_PAIRS: usize = 128;
pub(crate) const CHUNK_SQL: &str = "WITH wanted(ord, attempt_id, rowid) AS (VALUES (0, ?1, ?2), (1, ?3, ?4), (2, ?5, ?6), (3, ?7, ?8), (4, ?9, ?10), (5, ?11, ?12), (6, ?13, ?14), (7, ?15, ?16), (8, ?17, ?18), (9, ?19, ?20), (10, ?21, ?22), (11, ?23, ?24), (12, ?25, ?26), (13, ?27, ?28), (14, ?29, ?30), (15, ?31, ?32), (16, ?33, ?34), (17, ?35, ?36), (18, ?37, ?38), (19, ?39, ?40), (20, ?41, ?42), (21, ?43, ?44), (22, ?45, ?46), (23, ?47, ?48), (24, ?49, ?50), (25, ?51, ?52), (26, ?53, ?54), (27, ?55, ?56), (28, ?57, ?58), (29, ?59, ?60), (30, ?61, ?62), (31, ?63, ?64), (32, ?65, ?66), (33, ?67, ?68), (34, ?69, ?70), (35, ?71, ?72), (36, ?73, ?74), (37, ?75, ?76), (38, ?77, ?78), (39, ?79, ?80), (40, ?81, ?82), (41, ?83, ?84), (42, ?85, ?86), (43, ?87, ?88), (44, ?89, ?90), (45, ?91, ?92), (46, ?93, ?94), (47, ?95, ?96), (48, ?97, ?98), (49, ?99, ?100), (50, ?101, ?102), (51, ?103, ?104), (52, ?105, ?106), (53, ?107, ?108), (54, ?109, ?110), (55, ?111, ?112), (56, ?113, ?114), (57, ?115, ?116), (58, ?117, ?118), (59, ?119, ?120), (60, ?121, ?122), (61, ?123, ?124), (62, ?125, ?126), (63, ?127, ?128), (64, ?129, ?130), (65, ?131, ?132), (66, ?133, ?134), (67, ?135, ?136), (68, ?137, ?138), (69, ?139, ?140), (70, ?141, ?142), (71, ?143, ?144), (72, ?145, ?146), (73, ?147, ?148), (74, ?149, ?150), (75, ?151, ?152), (76, ?153, ?154), (77, ?155, ?156), (78, ?157, ?158), (79, ?159, ?160), (80, ?161, ?162), (81, ?163, ?164), (82, ?165, ?166), (83, ?167, ?168), (84, ?169, ?170), (85, ?171, ?172), (86, ?173, ?174), (87, ?175, ?176), (88, ?177, ?178), (89, ?179, ?180), (90, ?181, ?182), (91, ?183, ?184), (92, ?185, ?186), (93, ?187, ?188), (94, ?189, ?190), (95, ?191, ?192), (96, ?193, ?194), (97, ?195, ?196), (98, ?197, ?198), (99, ?199, ?200), (100, ?201, ?202), (101, ?203, ?204), (102, ?205, ?206), (103, ?207, ?208), (104, ?209, ?210), (105, ?211, ?212), (106, ?213, ?214), (107, ?215, ?216), (108, ?217, ?218), (109, ?219, ?220), (110, ?221, ?222), (111, ?223, ?224), (112, ?225, ?226), (113, ?227, ?228), (114, ?229, ?230), (115, ?231, ?232), (116, ?233, ?234), (117, ?235, ?236), (118, ?237, ?238), (119, ?239, ?240), (120, ?241, ?242), (121, ?243, ?244), (122, ?245, ?246), (123, ?247, ?248), (124, ?249, ?250), (125, ?251, ?252), (126, ?253, ?254), (127, ?255, ?256)) SELECT s.value_json, s.created_at_ms, s.updated_at_ms, s.rowid FROM (SELECT ord, attempt_id, rowid FROM wanted ORDER BY ord LIMIT -1) w CROSS JOIN v2_item_slots s ON s.attempt_id = w.attempt_id AND s.rowid = w.rowid ORDER BY w.ord";

/// One chunk cursor step in the proven advance/get shape: the caller reads
/// the current row through its own short `Rows::get` borrow after
/// `Available`. `End` exhausts the chunk; `Retired` records a
/// deterministic TooBig — the cursor is dropped here and the caller must
/// never arm another chunk.
pub(crate) enum ChunkStep {
    Available,
    End,
    Retired,
}

/// Arms one chunk from the caller-frame lazy statement cells: drops any
/// live cursor, forces the next cell (prepare on first use) and executes
/// it with the chunk's borrowed identity binds. Returns `false` when every
/// cell is exhausted.
pub(crate) fn arm_chunk<'a, 'conn, F>(
    iter: &mut std::slice::IterMut<'a, LazyCell<Result<Statement<'conn>, StoreErrorV1>, F>>,
    rows: &mut Option<Rows<'a>>,
    binds: &[rusqlite::types::ToSqlOutput<'_>],
    error: impl Fn(rusqlite::Error) -> StoreErrorV1,
) -> Result<bool, StoreErrorV1>
where
    F: FnOnce() -> Result<Statement<'conn>, StoreErrorV1>,
{
    let Some(cell) = iter.next() else {
        return Ok(false);
    };
    drop(rows.take());
    let statement = LazyCell::force_mut(cell)
        .as_mut()
        .map_err(|failure| failure.clone())?;
    *rows = Some(
        crate::query_accounting::accounted_query_prepared(
            statement,
            CHUNK_SQL,
            rusqlite::params_from_iter(binds.iter()),
        )
        .map_err(error)?,
    );
    Ok(true)
}

/// Steps the single live chunk cursor, accounting the delivered row. The
/// TooBig retirement drops the cursor; the returned marker tells the caller
/// to fall back without arming further chunks.
pub(crate) fn chunk_step(
    rows: &mut Option<Rows<'_>>,
    error: impl Fn(rusqlite::Error) -> StoreErrorV1,
) -> Result<ChunkStep, StoreErrorV1> {
    let Some(live) = rows.as_mut() else {
        return Ok(ChunkStep::End);
    };
    match live.next() {
        Ok(Some(row)) => {
            crate::query_accounting::accounted_delivered_row(CHUNK_SQL, row);
            Ok(ChunkStep::Available)
        }
        Ok(None) => Ok(ChunkStep::End),
        Err(failure) => {
            if matches!(&failure, rusqlite::Error::SqliteFailure(code, _)
                if code.code == rusqlite::ErrorCode::TooBig)
            {
                drop(rows.take());
                Ok(ChunkStep::Retired)
            } else {
                Err(error(failure))
            }
        }
    }
}
