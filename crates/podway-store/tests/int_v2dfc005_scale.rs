//! Mandatory production direct-Store G1/G2 workloads at grown L and 1,024 nodes.

use podway_core::{AttemptId, ItemId, Revision, Sha256Digest, UnixMillis};
use podway_store::{
    ActiveItemMutationRequestV2, ActiveItemMutationV2, GraphMutationErrorV2, GraphSessionStateV2,
    IdempotencyKeyV1,
    direct_store::{
        DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1,
        DirectTransactionBoundaryV1,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::int_v2dfc005_direct_store::{deadline, fixture, grown_graph, request, store, success};

const CHILD_ENV: &str = "PODWAY_DIRECT_SCALE_CHILD";
const CHILD_TEST: &str = "int_v2dfc005_scale::scale_process_child";
const L_COMPLETED_NODES: usize = 50;
const L_MINIMUM_BYTES: u64 = 43_384_832;
const G9_COMPLETION_CAP: u64 = (300 - L_COMPLETED_NODES as u64 - 1) / 8;

fn wall_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

fn item_revision(graph: &GraphSessionStateV2, writer: u64) -> Revision {
    let item = ItemId::new(format!("item-{writer}")).unwrap();
    graph
        .workflow_memory()
        .attempts()
        .last()
        .unwrap()
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item)
        .unwrap()
        .revision()
}

// Mirror the public command fences, retaining the caller's preparation-time
// values rather than substituting transaction-current revisions.
fn prepared_fences(
    current: &GraphSessionStateV2,
    prepared: &GraphSessionStateV2,
    writer: u64,
    session_fenced: bool,
) -> Result<(), GraphMutationErrorV2> {
    if session_fenced && current.trace().revision() != prepared.trace().revision() {
        return Err(GraphMutationErrorV2::SessionRevisionConflict {
            expected: prepared.trace().revision(),
            actual: current.trace().revision(),
        });
    }
    let expected = prepared.trace().active_attempt().unwrap().attempt_id();
    if current
        .trace()
        .active_attempt()
        .map(|attempt| attempt.attempt_id())
        != Some(expected)
    {
        return Err(GraphMutationErrorV2::AttemptNotCurrent {
            expected: expected.clone(),
            actual: current
                .trace()
                .active_attempt()
                .map(|attempt| attempt.attempt_id().clone()),
        });
    }
    if item_revision(current, writer) != item_revision(prepared, writer) {
        return Err(GraphMutationErrorV2::ItemRevisionConflict {
            expected: item_revision(prepared, writer),
            actual: item_revision(current, writer),
        });
    }
    Ok(())
}

#[test]
fn qualification_preserves_prepared_fences_and_rejects_without_outcomes() {
    let (_root, direct) = fixture();
    let graph = grown_graph(8, 0);
    let seed = request("qualification-fence-seed");
    direct
        .mutate(
            &seed,
            deadline(),
            || UnixMillis::new(200),
            |transaction| {
                transaction.now();
                transaction.create_graph(&graph)?;
                Ok(success(&seed, json!({"seeded":true}), transaction.now()))
            },
        )
        .unwrap();
    let stale = direct.prepare_graph(deadline()).unwrap().unwrap();
    let change = request("qualification-fence-change");
    direct
        .mutate(
            &change,
            deadline(),
            || UnixMillis::new(300),
            |transaction| {
                let prior = transaction.use_prepared_graph(Some(&stale))?.unwrap();
                prepared_fences(&prior, stale.graph(), 0, false)?;
                let now = transaction.now();
                let next = prior
                    .mutate_active_item_v2(
                        stale.graph().trace().active_attempt().unwrap().attempt_id(),
                        &ItemId::new("item-0").unwrap(),
                        item_revision(stale.graph(), 0),
                        ActiveItemMutationV2::Set {
                            value: "changed".into(),
                        },
                        now,
                    )?
                    .into_state();
                transaction.replace_graph(
                    prior.workspace_revision(),
                    prior.trace().revision(),
                    &next,
                )?;
                Ok(success(&change, json!({"changed":true}), now))
            },
        )
        .unwrap();
    let before = direct.prepare_graph(deadline()).unwrap().unwrap();
    for (writer, session_fenced) in [(0, false), (1, true)] {
        let command = request(&format!("qualification-stale-{writer}"));
        let rejected = direct.mutate(
            &command,
            deadline(),
            || panic!("rejection clock"),
            |transaction| {
                let current = transaction.use_prepared_graph(Some(&stale))?.unwrap();
                prepared_fences(&current, stale.graph(), writer, session_fenced)?;
                panic!("stale preparation was accepted")
            },
        );
        assert!(matches!(
            rejected,
            Err(DirectStoreErrorV1::Domain(
                GraphMutationErrorV2::ItemRevisionConflict { .. }
                    | GraphMutationErrorV2::SessionRevisionConflict { .. }
            ))
        ));
        direct
            .read(deadline(), |snapshot| {
                assert!(snapshot.outcome(command.key())?.is_none());
                assert_eq!(
                    snapshot.view(UnixMillis::new(400))?.graph(),
                    Some(before.graph())
                );
                Ok(())
            })
            .unwrap();
    }
    assert!(prepared_fences(before.graph(), before.graph(), 0, false).is_ok());
    assert!(prepared_fences(before.graph(), before.graph(), 1, true).is_ok());
}

#[test]
fn scale_process_child() {
    let Some(input) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let input: Value = serde_json::from_slice(&fs::read(input).unwrap()).unwrap();
    let root = Path::new(input["root"].as_str().unwrap());
    let ready = Path::new(input["ready"].as_str().unwrap());
    let start = Path::new(input["start"].as_str().unwrap());
    let output = Path::new(input["output"].as_str().unwrap());
    let direct = store(root);
    let initial = direct.prepare_graph(deadline()).unwrap().unwrap();
    let timed = input["timed"] == true;
    let initial_session_revision = initial.graph().trace().revision().get();
    let initial_workspace_revision = initial.graph().workspace_revision().get();
    fs::write(ready, b"ready").unwrap();
    let barrier = Instant::now() + Duration::from_secs(60);
    while !start.exists() {
        assert!(Instant::now() < barrier, "start barrier expired");
        std::thread::sleep(Duration::from_millis(2));
    }
    let began = Instant::now();
    if input["role"] == "observer" {
        let stop = Path::new(input["stop"].as_str().unwrap());
        let initial_item_revisions: u64 = initial
            .graph()
            .workflow_memory()
            .attempts()
            .iter()
            .flat_map(|attempt| attempt.item_slots())
            .map(|slot| slot.revision().get())
            .sum();
        let initial_attempts = initial.graph().workflow_memory().attempts().len();
        let mut reads = 0;
        while !stop.exists() {
            direct
                .read(deadline(), |snapshot| {
                    let view = snapshot.view(UnixMillis::new(1000))?;
                    let state = view.graph().unwrap();
                    let changes = view.sequence() - 2;
                    let item_revisions: u64 = state
                        .workflow_memory()
                        .attempts()
                        .iter()
                        .flat_map(|attempt| attempt.item_slots())
                        .map(|slot| slot.revision().get())
                        .sum();
                    let sum = item_revisions - initial_item_revisions
                        + (state.workflow_memory().attempts().len() - initial_attempts) as u64;
                    assert_eq!(sum, changes, "snapshot exposed a partial state effect");
                    assert_eq!(
                        state.trace().revision().get(),
                        initial_session_revision + changes
                    );
                    assert_eq!(
                        state.workspace_revision().get(),
                        initial_workspace_revision + changes
                    );
                    assert_eq!(
                        snapshot.latest_outcome()?.unwrap().sequence(),
                        view.sequence(),
                        "snapshot exposed a partial outcome"
                    );
                    Ok(())
                })
                .unwrap();
            reads += 1;
        }
        if let Some(delay) = input["observer_report_delay_ms"].as_u64() {
            std::thread::sleep(Duration::from_millis(delay));
        }
        fs::write(
            output,
            serde_json::to_vec_pretty(
                &json!({"reads":reads,"duration_ms":began.elapsed().as_millis()}),
            )
            .unwrap(),
        )
        .unwrap();
        return;
    }
    let writer = input["writer"].as_u64().unwrap();
    let count = input["count"].as_u64().unwrap();
    let events = Arc::new(Mutex::new(Vec::with_capacity(4)));
    let observer = Arc::clone(&events);
    let direct = direct.with_boundary_observer(move |boundary| {
        observer.lock().unwrap().push((boundary, Instant::now()))
    });
    let mut samples = Vec::new();
    let mut clocks = 0;
    let mut completed = 0;
    let mut completed_actions = 0;
    let mut recorded = 0;
    let mut rejections = 0;
    let mut accepted_keys = Vec::new();
    let mut trace = fs::File::create(output.with_extension("samples.jsonl")).unwrap();
    for index in 0..count {
        if timed && Path::new(input["stop"].as_str().unwrap()).exists() {
            break;
        }
        let value = format!(
            "{writer}:{index:0width$}",
            width = if timed { 4094 } else { 62 }
        );
        assert_eq!(value.len(), if timed { 4096 } else { 64 });
        let completing = if timed {
            (index + 1) % 192 == 0 && completed_actions < G9_COMPLETION_CAP
        } else {
            index % 50 == 49
        };
        let recording = !timed && (39..49).contains(&(index % 50));
        let command_name = if completing {
            "session.complete"
        } else if recording {
            "item.record_many"
        } else {
            "item.set"
        };
        let digest =
            Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(value.as_bytes()))).unwrap();
        let mut retry = 0;
        loop {
            let key = if retry == 0 {
                format!("writer-{writer}-{index}")
            } else {
                format!("writer-{writer}-{index}-retry-{retry}")
            };
            let command = DirectMutationRequestV1::new(
                IdempotencyKeyV1::new(key.clone()).unwrap(),
                digest.clone(),
                command_name,
                format!("00000000-0000-4000-8{writer:03x}-{index:012x}"),
                DirectOutcomeScopeV1::Session(initial.graph().trace().session_id().clone()),
            )
            .unwrap();
            let limit = Instant::now() + Duration::from_secs(5);
            let preparing = Instant::now();
            let prepared = direct.prepare_graph(limit).unwrap().unwrap();
            let prepared_at = Instant::now();
            let mut phases = Vec::with_capacity(4);
            events.lock().unwrap().clear();
            let result = direct.mutate(
                &command,
                limit,
                || {
                    clocks += 1;
                    // One shared fixture epoch keeps timestamps ordered across
                    // OS writers while preserving the deterministic output helper.
                    UnixMillis::new(
                        1000 + wall_millis() - input["clock_origin_ms"].as_u64().unwrap(),
                    )
                },
                |transaction| {
                    phases.push(Instant::now());
                    let prior = transaction.use_prepared_graph(Some(&prepared))?.unwrap();
                    phases.push(Instant::now());
                    prepared_fences(&prior, prepared.graph(), writer, recording || completing)?;
                    let now = transaction.now();
                    let next = if completing {
                        prior
                            .complete_active_action_v2(
                                prepared.graph().trace().revision(),
                                prepared
                                    .graph()
                                    .trace()
                                    .active_attempt()
                                    .unwrap()
                                    .attempt_id(),
                                Some(
                                    AttemptId::new(format!(
                                        "00000000-0000-4000-a{writer:03x}-{index:012x}"
                                    ))
                                    .unwrap(),
                                ),
                                now,
                            )?
                            .into_state()
                    } else {
                        let item = ItemId::new(format!("item-{writer}")).unwrap();
                        let revision = item_revision(prepared.graph(), writer);
                        let mutation = ActiveItemMutationV2::Set {
                            value: value.clone(),
                        };
                        if recording {
                            prior
                                .mutate_active_items_v2(
                                    prepared
                                        .graph()
                                        .trace()
                                        .active_attempt()
                                        .unwrap()
                                        .attempt_id(),
                                    &[ActiveItemMutationRequestV2::new(item, revision, mutation)],
                                    now,
                                )?
                                .into_state()
                        } else {
                            prior
                                .mutate_active_item_v2(
                                    prepared
                                        .graph()
                                        .trace()
                                        .active_attempt()
                                        .unwrap()
                                        .attempt_id(),
                                    &item,
                                    revision,
                                    mutation,
                                    now,
                                )?
                                .into_state()
                        }
                    };
                    phases.push(Instant::now());
                    transaction.replace_graph(
                        prior.workspace_revision(),
                        prior.trace().revision(),
                        &next,
                    )?;
                    phases.push(Instant::now());
                    Ok(success(
                        &command,
                        json!({"changed":true,"writer":writer,"operation":index,"kind":command_name,
                        "session_revision_before":prior.trace().revision().get(),
                        "attempt_before":prior.trace().active_attempt().unwrap().attempt_id(),
                        "session_revision_after":next.trace().revision().get(),
                        "attempt_after":next.trace().active_attempt().unwrap().attempt_id()}),
                        transaction.now(),
                    ))
                },
            );
            let finished = Instant::now();
            let event = events.lock().unwrap();
            writeln!(trace, "{}", json!({"operation":index,"command":command_name,"warmup":index<20,
            "success":result.is_ok(),"prepare_us":prepared_at.duration_since(preparing).as_micros(),
            "prebegin_us":event.first().map(|(_,time)|time.duration_since(prepared_at).as_micros()),
            "wait_us":event.get(1).map(|(_,time)|time.duration_since(event[0].1).as_micros()),
            "hold_us":event.get(1).map(|(_,time)|event.get(3).map_or(finished,|(_,time)|*time).duration_since(*time).as_micros()),
            "postcommit_us":event.get(3).map(|(_,time)|finished.duration_since(*time).as_micros()),
            "refresh_us":phases.get(1).map(|time|time.duration_since(phases[0]).as_micros()),
            "transition_us":phases.get(2).map(|time|time.duration_since(phases[1]).as_micros()),
            "persist_us":phases.get(3).map(|time|time.duration_since(phases[2]).as_micros())})).unwrap();
            assert!(
                event.len() >= 2,
                "writer {writer} operation {index}: {result:?}; boundaries={event:?}"
            );
            assert_eq!(event[0].0, DirectTransactionBoundaryV1::BeforeBegin);
            assert_eq!(event[1].0, DirectTransactionBoundaryV1::AfterBegin);
            let hold = if event.len() == 4 {
                event[3].1
            } else {
                finished
            };
            samples.push(
                json!({"wait_us":event[1].1.duration_since(event[0].1).as_micros(),
                "hold_us":hold.duration_since(event[1].1).as_micros(),"committed":result.is_ok(),
                "kind":if result.is_ok() {"commit"} else {"rejection"},"warmup":index<20}),
            );
            drop(event);
            if matches!(
                &result,
                Err(DirectStoreErrorV1::Domain(
                    GraphMutationErrorV2::SessionRevisionConflict { .. }
                        | GraphMutationErrorV2::AttemptNotCurrent { .. }
                        | GraphMutationErrorV2::ItemRevisionConflict { .. }
                ))
            ) {
                direct
                    .read(deadline(), |snapshot| {
                        assert!(
                            snapshot.outcome(command.key())?.is_none(),
                            "rejection recorded an outcome"
                        );
                        Ok(())
                    })
                    .unwrap();
                rejections += 1;
                retry += 1;
                continue;
            }
            let committed = result.unwrap_or_else(|error| {
                panic!("direct writer {writer} operation {index} failed: {error}")
            });
            assert!(!committed.replayed());
            events.lock().unwrap().clear();
            let replay_request = DirectMutationRequestV1::new(
                command.key().clone(),
                command.digest().clone(),
                command.command(),
                format!("00000000-0000-4000-9{writer:03x}-{index:012x}"),
                command.scope().clone(),
            )
            .unwrap();
            let replay = direct
                .mutate(
                    &replay_request,
                    deadline(),
                    || panic!("replay clock"),
                    |_| panic!("replay callback"),
                )
                .unwrap();
            let finished = Instant::now();
            let event = events.lock().unwrap();
            assert_eq!(event.len(), 2, "replay must not enter state/commit seams");
            samples.push(json!({"wait_us":event[1].1.duration_since(event[0].1).as_micros(),
                        "hold_us":finished.duration_since(event[1].1).as_micros(),"committed":false,"replayed":true,"kind":"replay","warmup":index<20}));
            drop(event);
            assert!(replay.replayed());
            assert_eq!(replay.outcome(), committed.outcome());
            accepted_keys.push(key);
            completed += 1;
            if completing {
                completed_actions += 1;
            }
            if recording {
                recorded += 1;
            }
            break;
        }
    }
    assert_eq!(
        clocks, completed,
        "rejections/replays read the clock or successes skipped it"
    );
    fs::write(
        output,
        serde_json::to_vec_pretty(
            &json!({"writer":writer,"successes":completed,"replays":completed,
        "clock_reads":clocks,"samples":samples,"rejections":rejections,"accepted_keys":accepted_keys,
        "set_successes":completed-completed_actions-recorded,"record_successes":recorded,"complete_successes":completed_actions,
        "payload_characters":if timed {4096}else{64},
        "duration_ms":began.elapsed().as_millis()}),
        )
        .unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "explicit ten-minute production G9 WAL-bound gate"]
fn direct_ten_minute_g9_wal_bound() {
    let evidence =
        std::env::var_os("PODWAY_DIRECT_TEST_EVIDENCE_ROOT").expect("explicit owned evidence root");
    let evidence = Path::new(&evidence);
    assert!(evidence.is_absolute() && evidence.is_dir());
    let seconds = std::env::var("PODWAY_DIRECT_SCALE_SECONDS")
        .map_or(600, |value| value.parse::<u64>().unwrap());
    assert!((1..=600).contains(&seconds));
    let observer_report_delay_ms = std::env::var("PODWAY_DIRECT_G9_OBSERVER_DELAY_MS")
        .map_or(0, |value| value.parse::<u64>().unwrap());
    assert!(observer_report_delay_ms == 0 || (seconds == 1 && observer_report_delay_ms == 6000));
    let (root, direct) = fixture();
    let graph = grown_graph(300, L_COMPLETED_NODES);
    let seed = request("seed-wal");
    direct
        .mutate(
            &seed,
            deadline(),
            || UnixMillis::new(200),
            |transaction| {
                transaction.now();
                transaction.create_graph(&graph)?;
                Ok(success(&seed, json!({"seeded":true}), transaction.now()))
            },
        )
        .unwrap();
    assert!(fs::metadata(direct.layout().database()).unwrap().len() >= L_MINIMUM_BYTES);
    let idle = rusqlite::Connection::open(direct.layout().database()).unwrap();
    assert_eq!(
        idle.query_row("PRAGMA journal_mode;", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    assert!(idle.is_autocommit());
    let case = evidence.join("g9-grown-l");
    fs::create_dir(&case).unwrap();
    let start = case.join("start");
    let stop = case.join("stop");
    let mut children = Vec::new();
    let mut readiness = Vec::new();
    let clock_origin_ms = wall_millis();
    for index in 0..12 {
        let output = case.join(format!("worker-{index}.json"));
        let input = case.join(format!("input-{index}.json"));
        let ready = case.join(format!("ready-{index}"));
        fs::write(&input,serde_json::to_vec(&json!({"clock_origin_ms":clock_origin_ms,"root":root.path(),"ready":ready,"start":start,"stop":stop,
            "output":output,"role":if index<8 {"writer"}else{"observer"},"writer":index,"count":50000,"timed":true,
            "observer_report_delay_ms":observer_report_delay_ms})).unwrap()).unwrap();
        children.push(spawn(
            &input,
            &case.join(format!("worker-{index}.log")),
            output,
        ));
        readiness.push(ready);
    }
    let ready_limit = Instant::now() + Duration::from_secs(60);
    while !readiness.iter().all(|path| path.exists()) {
        assert!(Instant::now() < ready_limit);
        std::thread::sleep(Duration::from_millis(2));
    }
    fs::write(&start, b"start").unwrap();
    let began = Instant::now();
    let end = began + Duration::from_secs(seconds);
    let mut maximum_wal = 0;
    let mut samples = 0;
    while Instant::now() < end {
        for child in &mut children {
            assert!(
                child.child.try_wait().unwrap().is_none(),
                "load child stopped prematurely: {:?}",
                child.output
            );
        }
        let size = fs::metadata(direct.layout().wal()).unwrap().len();
        maximum_wal = maximum_wal.max(size);
        samples += 1;
        assert!(
            size <= 64 * 1024 * 1024,
            "active WAL exceeded 64 MiB: {size}"
        );
        std::thread::sleep(
            Duration::from_millis(50).min(end.saturating_duration_since(Instant::now())),
        );
    }
    let active_elapsed_ms = began.elapsed().as_millis();
    assert!(active_elapsed_ms >= u128::from(seconds) * 1000);
    // Stop publication precedes every final in-flight commit. Measuring from
    // here is a conservative upper bound on time since actual load cessation,
    // and worker cleanup/report collection must not restart the allowance.
    let stopped_at = Instant::now();
    fs::write(&stop, b"stop").unwrap();
    let idle_limit = stopped_at + Duration::from_secs(5);
    let mut reports = Vec::new();
    let mut successes = 0;
    let mut completed_actions = 0;
    for (index, child) in children.iter_mut().enumerate() {
        assert!(
            child.child.wait().unwrap().success(),
            "load child failed: {:?}",
            child.output
        );
        let report: Value = serde_json::from_slice(&fs::read(&child.output).unwrap()).unwrap();
        if index < 8 {
            let count = report["successes"].as_u64().unwrap();
            assert!(count > 0);
            assert_eq!(report["replays"].as_u64(), Some(count));
            assert_eq!(report["payload_characters"], 4096);
            let completions = report["complete_successes"].as_u64().unwrap();
            assert_eq!(completions, (count / 192).min(G9_COMPLETION_CAP));
            if seconds == 600 {
                assert!(
                    completions > 0,
                    "full G9 writer never reached its required completion"
                );
            }
            completed_actions += completions;
            successes += count;
        } else {
            assert!(report["reads"].as_u64().unwrap() > 0);
        }
        reports.push(report);
    }
    let (final_wal, drain_observed_at) = loop {
        let size = fs::metadata(direct.layout().wal()).unwrap().len();
        let observed_at = Instant::now();
        if size <= 4 * 1024 * 1024 || observed_at >= idle_limit {
            break (size, observed_at);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    direct
        .read(deadline(), |snapshot| {
            let view = snapshot.view(UnixMillis::new(10000))?;
            assert_eq!(view.sequence(), 2 + successes);
            assert_eq!(
                snapshot.latest_outcome()?.unwrap().sequence(),
                view.sequence()
            );
            Ok(())
        })
        .unwrap();
    direct.verify_integrity(deadline()).unwrap();
    assert!(idle.is_autocommit());
    fs::write(case.join("summary.json"),serde_json::to_vec_pretty(&json!({"path":"production DirectStoreV1",
        "profile":if seconds==600 {"required"}else{"diagnostic"},"nodes":300,
        "initial_completed_nodes":L_COMPLETED_NODES,"completed_nodes":L_COMPLETED_NODES as u64+completed_actions,
        "writers":8,"readers":4,"duration_seconds":seconds,"active_elapsed_ms":active_elapsed_ms,"measurement_samples":samples,"successes":successes,
        "set_successes":successes-completed_actions,"complete_successes":completed_actions,
        "payload_characters":4096,"completion_every_attempts":192,"completion_cap_per_writer":G9_COMPLETION_CAP,
        "idle_wal_connection_held":true,"idle_connection_autocommit":idle.is_autocommit(),
        "maximum_active_wal_bytes":maximum_wal,"post_load_wal_bytes":final_wal,
        "drain_endpoint":"stop_marker_publication_conservative_upper_bound",
        "post_load_observation_ms":drain_observed_at.duration_since(stopped_at).as_millis(),
        "drain_within_bound":wal_drained_within_bound(final_wal, stopped_at, drain_observed_at)})).unwrap()).unwrap();
    assert!(
        wal_drained_within_bound(final_wal, stopped_at, drain_observed_at),
        "WAL remained above 4 MiB five seconds after load: {final_wal}"
    );
    drop(idle);
}

fn wal_drained_within_bound(bytes: u64, stopped_at: Instant, observed_at: Instant) -> bool {
    bytes <= 4 * 1024 * 1024 && observed_at.duration_since(stopped_at) <= Duration::from_secs(5)
}

#[test]
fn g9_delayed_cleanup_cannot_extend_the_stop_publication_deadline() {
    let evidence = tempfile::tempdir().unwrap();
    let output = evidence.path().join("controller.json");
    let log = fs::File::create(evidence.path().join("controller.log")).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "int_v2dfc005_scale::direct_ten_minute_g9_wal_bound",
            "--ignored",
            "--nocapture",
        ])
        .env("PODWAY_DIRECT_TEST_EVIDENCE_ROOT", evidence.path())
        .env("PODWAY_DIRECT_SCALE_SECONDS", "1")
        .env("PODWAY_DIRECT_G9_OBSERVER_DELAY_MS", "6000")
        .env_remove(CHILD_ENV)
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    let mut child = OwnedChild { child, output };
    let limit = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < limit,
            "delayed-cleanup sentinel exceeded its bound"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(!status.success(), "late cleanup reset the G9 drain clock");
    let summary: Value =
        serde_json::from_slice(&fs::read(evidence.path().join("g9-grown-l/summary.json")).unwrap())
            .unwrap();
    assert_eq!(summary["drain_within_bound"], false);
    assert!(summary["post_load_observation_ms"].as_u64().unwrap() >= 6000);
    assert!(summary["post_load_wal_bytes"].as_u64().unwrap() <= 4 * 1024 * 1024);
}

struct OwnedChild {
    child: Child,
    output: PathBuf,
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(input: &Path, log: &Path, output: PathBuf) -> OwnedChild {
    let stdout = fs::File::create(log).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_ENV, input)
        .stdout(Stdio::from(stdout.try_clone().unwrap()))
        .stderr(Stdio::from(stdout))
        .spawn()
        .unwrap();
    OwnedChild { child, output }
}

fn percentile(values: &[u64], numerator: usize) -> u64 {
    values[(values.len() * numerator).div_ceil(100) - 1]
}

#[test]
#[ignore = "explicit release-profile required production G1/G2 scale gate"]
fn direct_multi_process_g1_g2_l_and_maximum() {
    if cfg!(debug_assertions) {
        panic!("G1/G2 requires the release profile");
    }
    let evidence =
        std::env::var_os("PODWAY_DIRECT_TEST_EVIDENCE_ROOT").expect("explicit owned evidence root");
    let evidence = Path::new(&evidence);
    assert!(evidence.is_absolute() && evidence.is_dir());
    let count = std::env::var("PODWAY_DIRECT_SCALE_COUNT")
        .map_or(1000, |count| count.parse::<u64>().unwrap());
    assert!(count > 0 && count <= 1000 && count.is_multiple_of(50));
    for nodes in [300, 1024] {
        let (root, direct) = fixture();
        let graph = grown_graph(nodes, L_COMPLETED_NODES);
        let seed = request("seed-scale");
        direct
            .mutate(
                &seed,
                deadline(),
                || UnixMillis::new(200),
                |transaction| {
                    transaction.now();
                    transaction.create_graph(&graph)?;
                    Ok(success(&seed, json!({"seeded":true}), transaction.now()))
                },
            )
            .unwrap();
        let initial_bytes = fs::metadata(direct.layout().database()).unwrap().len();
        assert!(
            initial_bytes >= L_MINIMUM_BYTES,
            "Store fixture did not reach L: {initial_bytes} bytes"
        );
        let case = evidence.join(format!("g1-g2-{nodes}"));
        fs::create_dir(&case).unwrap();
        let start = case.join("start");
        let stop = case.join("stop");
        let mut workers = Vec::new();
        let mut observers = Vec::new();
        let mut ready = Vec::new();
        let clock_origin_ms = wall_millis();
        for index in 0..10 {
            let output = case.join(format!("worker-{index}.json"));
            let input = case.join(format!("input-{index}.json"));
            let marker = case.join(format!("ready-{index}"));
            fs::write(&input,serde_json::to_vec_pretty(&json!({"clock_origin_ms":clock_origin_ms,"root":root.path(),"ready":marker,"start":start,"stop":stop,
                "output":output,"role":if index<8 {"writer"} else {"observer"},"writer":index,"count":count})).unwrap()).unwrap();
            let child = spawn(&input, &case.join(format!("worker-{index}.log")), output);
            if index < 8 {
                workers.push(child);
            } else {
                observers.push(child);
            }
            ready.push(marker);
        }
        let limit = Instant::now() + Duration::from_secs(60);
        while !ready.iter().all(|path| path.exists()) {
            assert!(Instant::now() < limit, "child readiness deadline exceeded");
            std::thread::sleep(Duration::from_millis(2));
        }
        fs::write(&start, b"start").unwrap();
        let completion_deadline = Instant::now() + Duration::from_secs(1800);
        loop {
            let mut running = false;
            for child in workers.iter_mut().chain(observers.iter_mut()) {
                match child.child.try_wait().unwrap() {
                    Some(status) => assert!(
                        status.success(),
                        "child failed; inspect owned worker log: {:?}",
                        child.output
                    ),
                    None => running = true,
                }
            }
            if workers
                .iter_mut()
                .all(|child| child.child.try_wait().unwrap().is_some())
            {
                break;
            }
            assert!(
                running && Instant::now() < completion_deadline,
                "case exceeded its execution deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut reports = Vec::new();
        let mut wait = Vec::new();
        let mut hold = Vec::new();
        let mut durations = Vec::new();
        let mut rejections = 0;
        let mut transaction_counts = std::collections::BTreeMap::<String, u64>::new();
        for child in &mut workers {
            assert!(
                child.child.wait().unwrap().success(),
                "writer failed; inspect owned worker log"
            );
            let report: Value = serde_json::from_slice(&fs::read(&child.output).unwrap()).unwrap();
            assert_eq!(report["successes"].as_u64(), Some(count));
            assert_eq!(report["replays"].as_u64(), Some(count));
            assert_eq!(report["set_successes"].as_u64(), Some(count / 50 * 39));
            assert_eq!(report["record_successes"].as_u64(), Some(count / 50 * 10));
            assert_eq!(report["complete_successes"].as_u64(), Some(count / 50));
            rejections += report["rejections"].as_u64().unwrap();
            durations.push(report["duration_ms"].as_u64().unwrap());
            for sample in report["samples"].as_array().unwrap() {
                if sample["warmup"] == true {
                    continue;
                }
                wait.push(sample["wait_us"].as_u64().unwrap());
                hold.push(sample["hold_us"].as_u64().unwrap());
                *transaction_counts
                    .entry(sample["kind"].as_str().unwrap().to_owned())
                    .or_default() += 1;
            }
            reports.push(report);
        }
        fs::write(&stop, b"stop").unwrap();
        for child in &mut observers {
            assert!(
                child.child.wait().unwrap().success(),
                "observer failed; inspect owned worker log"
            );
            let report: Value = serde_json::from_slice(&fs::read(&child.output).unwrap()).unwrap();
            assert!(report["reads"].as_u64().unwrap() > 0);
            reports.push(report);
        }
        direct
            .read(deadline(), |snapshot| {
                let view = snapshot.view(UnixMillis::new(10000))?;
                assert_eq!(view.sequence(), 2 + 8 * count);
                let state = view.graph().unwrap();
                let mut serial = std::collections::BTreeMap::new();
                for writer in 0..8 {
                    for index in 0..count {
                        let key = IdempotencyKeyV1::new(
                            reports[writer as usize]["accepted_keys"][index as usize]
                                .as_str()
                                .unwrap(),
                        )
                        .unwrap();
                        let outcome = snapshot.outcome(&key)?.unwrap();
                        let document: Value =
                            serde_json::from_str(outcome.response().as_str()).unwrap();
                        assert_eq!(document["result"]["writer"].as_u64(), Some(writer));
                        assert_eq!(document["result"]["operation"].as_u64(), Some(index));
                        assert_eq!(
                            document["request_id"],
                            format!("00000000-0000-4000-8{writer:03x}-{index:012x}")
                        );
                        let value = format!("{writer}:{index:062}");
                        assert_eq!(
                            outcome.digest().as_str(),
                            format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
                        );
                        assert!(
                            serial
                                .insert(
                                    outcome.sequence(),
                                    (writer, index, value, document, outcome.recorded_at())
                                )
                                .is_none()
                        );
                    }
                }
                let mut replay = graph.clone();
                let mut operations = [0_u64; 8];
                for (ordinal, (sequence, (writer, index, value, document, now))) in
                    serial.into_iter().enumerate()
                {
                    assert_eq!(sequence, ordinal as u64 + 3);
                    assert_eq!(index, operations[writer as usize]);
                    operations[writer as usize] += 1;
                    let result = &document["result"];
                    assert_eq!(
                        result["session_revision_before"].as_u64(),
                        Some(replay.trace().revision().get())
                    );
                    let active = replay
                        .trace()
                        .active_attempt()
                        .unwrap()
                        .attempt_id()
                        .clone();
                    assert_eq!(result["attempt_before"].as_str(), Some(active.as_str()));
                    replay = match result["kind"].as_str().unwrap() {
                        "session.complete" => replay
                            .complete_active_action_v2(
                                replay.trace().revision(),
                                &active,
                                Some(
                                    AttemptId::new(result["attempt_after"].as_str().unwrap())
                                        .unwrap(),
                                ),
                                now,
                            )
                            .unwrap()
                            .into_state(),
                        "item.set" | "item.record_many" => {
                            let item = ItemId::new(format!("item-{writer}")).unwrap();
                            let revision = replay
                                .workflow_memory()
                                .attempts()
                                .last()
                                .unwrap()
                                .item_slots()
                                .iter()
                                .find(|slot| slot.item_id() == &item)
                                .unwrap()
                                .revision();
                            let mutation = ActiveItemMutationV2::Set { value };
                            if result["kind"] == "item.record_many" {
                                replay
                                    .mutate_active_items_v2(
                                        &active,
                                        &[ActiveItemMutationRequestV2::new(
                                            item, revision, mutation,
                                        )],
                                        now,
                                    )
                                    .unwrap()
                                    .into_state()
                            } else {
                                replay
                                    .mutate_active_item_v2(&active, &item, revision, mutation, now)
                                    .unwrap()
                                    .into_state()
                            }
                        }
                        other => panic!("unknown accepted mutation {other}"),
                    };
                    assert_eq!(
                        result["session_revision_after"].as_u64(),
                        Some(replay.trace().revision().get())
                    );
                }
                assert_eq!(
                    state, &replay,
                    "final state differs from serial accepted-outcome replay"
                );
                Ok(())
            })
            .unwrap();
        direct.verify_integrity(deadline()).unwrap();
        wait.sort_unstable();
        hold.sort_unstable();
        durations.sort_unstable();
        let summary = json!({"path":"production DirectStoreV1","profile":if count==1000 {"required"} else {"diagnostic"},
            "nodes":nodes,"initial_main_database_bytes":initial_bytes,"completed_nodes":L_COMPLETED_NODES,"writers":8,"observers":2,"successes_per_writer":count,
            "g1_gate":count==1000,"g2_gate":count==1000 && nodes==300,"serial_replay_matches":true,
            "stale_fence_rejections":rejections,"rejection_outcomes":0,"stale_fence_commits":0,
            "transaction_counts":transaction_counts,"hold_endpoint":"AfterCommitBeforeOutput_or_typed_API_return_upper_bound",
            "warmup_successes_per_writer":20,"samples":wait.len(),"wait_p50_us":percentile(&wait,50),"wait_p95_us":percentile(&wait,95),"wait_p99_us":percentile(&wait,99),"wait_max_us":wait.last(),
            "hold_p50_us":percentile(&hold,50),"hold_p95_us":percentile(&hold,95),"hold_max_us":hold.last(),"hold_p99_us":percentile(&hold,99),"slowest_writer_ms":durations.last(),"fastest_writer_ms":durations.first()});
        fs::write(
            case.join("summary.json"),
            serde_json::to_vec_pretty(&summary).unwrap(),
        )
        .unwrap();
        if count == 1000 && nodes == 300 {
            assert!(percentile(&wait, 99) <= 250_000, "wait p99 gate failed");
            assert!(*wait.last().unwrap() <= 1_000_000, "wait max gate failed");
            assert!(percentile(&hold, 99) <= 50_000, "hold p99 gate failed");
            assert!(
                *durations.last().unwrap() <= 2 * durations.first().unwrap(),
                "writer fairness gate failed"
            );
        }
        assert!(
            rejections > 0,
            "qualification did not exercise stale-fence rejection"
        );
    }
}
