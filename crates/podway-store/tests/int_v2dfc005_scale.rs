//! Mandatory production direct-Store G1/G2 workloads at grown L and 1,024 nodes.

use podway_core::{AttemptId, ItemId, Revision, Sha256Digest, UnixMillis};
use podway_store::{
    ActiveItemMutationV2, IdempotencyKeyV1,
    direct_store::{
        DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1,
        DirectTransactionBoundaryV1,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use super::int_v2dfc005_direct_store::{deadline, fixture, grown_graph, request, store, success};

const CHILD_ENV: &str = "PODWAY_DIRECT_SCALE_CHILD";
const CHILD_TEST: &str = "int_v2dfc005_scale::scale_process_child";

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
                    let sum: u64 = if timed {
                        let item_revisions: u64 = state
                            .workflow_memory()
                            .attempts()
                            .iter()
                            .flat_map(|attempt| attempt.item_slots())
                            .map(|slot| slot.revision().get())
                            .sum();
                        item_revisions - initial_item_revisions
                            + (state.workflow_memory().attempts().len() - initial_attempts) as u64
                    } else {
                        state
                            .workflow_memory()
                            .attempts()
                            .last()
                            .unwrap()
                            .item_slots()
                            .iter()
                            .map(|slot| slot.revision().get())
                            .sum()
                    };
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
    let mut stale_preparation = 0;
    let mut clocks = 0;
    let mut completed = 0;
    let mut completed_actions = 0;
    for index in 0..count {
        if timed && Path::new(input["stop"].as_str().unwrap()).exists() {
            break;
        }
        let value = format!(
            "{writer}:{index:0width$}",
            width = if timed { 4094 } else { 62 }
        );
        assert_eq!(value.len(), if timed { 4096 } else { 64 });
        let completing = timed && (index + 1) % 192 == 0 && completed_actions < 36;
        let command_name = if completing {
            "session.complete"
        } else {
            "item.set"
        };
        let digest =
            Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(value.as_bytes()))).unwrap();
        let command = DirectMutationRequestV1::new(
            IdempotencyKeyV1::new(format!("writer-{writer}-{index}")).unwrap(),
            digest,
            command_name,
            format!("00000000-0000-4000-8{writer:03x}-{index:012x}"),
            DirectOutcomeScopeV1::Session(initial.graph().trace().session_id().clone()),
        )
        .unwrap();
        let limit = Instant::now() + Duration::from_secs(5);
        loop {
            let prepared = direct.prepare_graph(limit).unwrap().unwrap();
            events.lock().unwrap().clear();
            let result = direct.mutate(
                &command,
                limit,
                || {
                    clocks += 1;
                    UnixMillis::new(1000 + prepared.sequence())
                },
                |transaction| {
                    transaction.use_prepared_graph(&prepared)?;
                    let prior = prepared.graph();
                    let now = transaction.now();
                    let next = if completing {
                        prior
                            .complete_active_action_v2(
                                prior.trace().revision(),
                                prior.trace().active_attempt().unwrap().attempt_id(),
                                Some(AttemptId::new(format!(
                                    "00000000-0000-4000-a{writer:03x}-{index:012x}"
                                )).unwrap()),
                                now,
                            )?
                            .into_state()
                    } else {
                        let item = ItemId::new(format!("item-{writer}")).unwrap();
                        let slot = prior
                            .workflow_memory()
                            .attempts()
                            .last()
                            .unwrap()
                            .item_slots()
                            .iter()
                            .find(|slot| slot.item_id() == &item)
                            .unwrap();
                        if !timed {
                            assert_eq!(
                                slot.revision(),
                                Revision::new(index),
                                "a rejected attempt changed item state"
                            );
                        }
                        prior
                            .mutate_active_item_v2(
                                prior.trace().active_attempt().unwrap().attempt_id(),
                                &item,
                                slot.revision(),
                                ActiveItemMutationV2::Set {
                                    value: value.clone(),
                                },
                                now,
                            )?
                            .into_state()
                    };
                    transaction.replace_graph(
                        prior.workspace_revision(),
                        prior.trace().revision(),
                        &next,
                    )?;
                    Ok(success(
                        &command,
                        json!({"changed":true,"writer":writer,"operation":index,"kind":command_name}),
                        transaction.now(),
                    ))
                },
            );
            let finished = Instant::now();
            let event = events.lock().unwrap();
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
                "hold_us":hold.duration_since(event[1].1).as_micros(),"committed":result.is_ok()}),
            );
            drop(event);
            match result {
                Ok(committed) => {
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
                        "hold_us":finished.duration_since(event[1].1).as_micros(),"committed":false,"replayed":true}));
                    drop(event);
                    assert!(replay.replayed());
                    assert_eq!(replay.outcome(), committed.outcome());
                    completed += 1;
                    if completing {
                        completed_actions += 1;
                    }
                    break;
                }
                Err(DirectStoreErrorV1::PreparedStateChanged { .. }) => {
                    stale_preparation += 1;
                    assert!(Instant::now() < limit);
                }
                Err(error) => panic!("direct writer {writer} operation {index} failed: {error}"),
            }
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
        "clock_reads":clocks,"stale_preparation":stale_preparation,"samples":samples,
        "set_successes":completed-completed_actions,"complete_successes":completed_actions,
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
    let (root, direct) = fixture();
    let graph = grown_graph(300, 7);
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
    for index in 0..12 {
        let output = case.join(format!("worker-{index}.json"));
        let input = case.join(format!("input-{index}.json"));
        let ready = case.join(format!("ready-{index}"));
        fs::write(&input,serde_json::to_vec(&json!({"root":root.path(),"ready":ready,"start":start,"stop":stop,
            "output":output,"role":if index<8 {"writer"}else{"observer"},"writer":index,"count":50000,"timed":true})).unwrap()).unwrap();
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
    fs::write(&stop, b"stop").unwrap();
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
            assert_eq!(completions, (count / 192).min(36));
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
    let finished = Instant::now();
    let idle_limit = finished + Duration::from_secs(5);
    let final_wal = loop {
        let size = fs::metadata(direct.layout().wal()).unwrap().len();
        if size <= 4 * 1024 * 1024 || Instant::now() >= idle_limit {
            break size;
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
        "initial_completed_nodes":7,"completed_nodes":7+completed_actions,
        "writers":8,"readers":4,"duration_seconds":seconds,"active_elapsed_ms":active_elapsed_ms,"measurement_samples":samples,"successes":successes,
        "set_successes":successes-completed_actions,"complete_successes":completed_actions,
        "payload_characters":4096,"completion_every_attempts":192,"completion_cap_per_writer":36,
        "idle_wal_connection_held":true,"idle_connection_autocommit":idle.is_autocommit(),
        "maximum_active_wal_bytes":maximum_wal,"post_load_wal_bytes":final_wal,
        "post_load_observation_ms":finished.elapsed().as_millis()})).unwrap()).unwrap();
    assert!(
        final_wal <= 4 * 1024 * 1024,
        "WAL remained above 4 MiB five seconds after load: {final_wal}"
    );
    drop(idle);
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
    let evidence =
        std::env::var_os("PODWAY_DIRECT_TEST_EVIDENCE_ROOT").expect("explicit owned evidence root");
    let evidence = Path::new(&evidence);
    assert!(evidence.is_absolute() && evidence.is_dir());
    let count = std::env::var("PODWAY_DIRECT_SCALE_COUNT")
        .map_or(1000, |count| count.parse::<u64>().unwrap());
    assert!(count > 0 && count <= 1000);
    for nodes in [300, 1024] {
        let (root, direct) = fixture();
        let graph = grown_graph(nodes, 7);
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
        let case = evidence.join(format!("g1-g2-{nodes}"));
        fs::create_dir(&case).unwrap();
        let start = case.join("start");
        let stop = case.join("stop");
        let mut workers = Vec::new();
        let mut observers = Vec::new();
        let mut ready = Vec::new();
        for index in 0..10 {
            let output = case.join(format!("worker-{index}.json"));
            let input = case.join(format!("input-{index}.json"));
            let marker = case.join(format!("ready-{index}"));
            fs::write(&input,serde_json::to_vec_pretty(&json!({"root":root.path(),"ready":marker,"start":start,"stop":stop,
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
        for child in &mut workers {
            assert!(
                child.child.wait().unwrap().success(),
                "writer failed; inspect owned worker log"
            );
            let report: Value = serde_json::from_slice(&fs::read(&child.output).unwrap()).unwrap();
            assert_eq!(report["successes"].as_u64(), Some(count));
            assert_eq!(report["replays"].as_u64(), Some(count));
            durations.push(report["duration_ms"].as_u64().unwrap());
            for sample in report["samples"].as_array().unwrap() {
                wait.push(sample["wait_us"].as_u64().unwrap());
                hold.push(sample["hold_us"].as_u64().unwrap());
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
                        let key =
                            IdempotencyKeyV1::new(format!("writer-{writer}-{index}")).unwrap();
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
                                .insert(outcome.sequence(), (writer, index, value))
                                .is_none()
                        );
                    }
                }
                let mut revisions = [0_u64; 8];
                let mut values: [Option<String>; 8] = std::array::from_fn(|_| None);
                for (ordinal, (sequence, (writer, index, value))) in serial.into_iter().enumerate()
                {
                    assert_eq!(sequence, ordinal as u64 + 3);
                    assert_eq!(index, revisions[writer as usize]);
                    revisions[writer as usize] += 1;
                    values[writer as usize] = Some(value);
                }
                for writer in 0..8 {
                    let item = ItemId::new(format!("item-{writer}")).unwrap();
                    let slot = state
                        .workflow_memory()
                        .attempts()
                        .last()
                        .unwrap()
                        .item_slots()
                        .iter()
                        .find(|slot| slot.item_id() == &item)
                        .unwrap();
                    assert_eq!(slot.revision(), Revision::new(count));
                    assert_eq!(slot.revision().get(), revisions[writer]);
                    assert_eq!(
                        slot.value().and_then(|value| value.as_text()),
                        values[writer].as_deref()
                    );
                }
                Ok(())
            })
            .unwrap();
        direct.verify_integrity(deadline()).unwrap();
        wait.sort_unstable();
        hold.sort_unstable();
        durations.sort_unstable();
        let summary = json!({"path":"production DirectStoreV1","profile":if count==1000 {"required"} else {"diagnostic"},
            "nodes":nodes,"completed_nodes":7,"writers":8,"observers":2,"successes_per_writer":count,
            "samples":wait.len(),"wait_p99_us":percentile(&wait,99),"wait_max_us":wait.last(),
            "hold_p99_us":percentile(&hold,99),"slowest_writer_ms":durations.last(),"fastest_writer_ms":durations.first()});
        fs::write(
            case.join("summary.json"),
            serde_json::to_vec_pretty(&summary).unwrap(),
        )
        .unwrap();
        if count == 1000 {
            assert!(percentile(&wait, 99) <= 250_000, "wait p99 gate failed");
            assert!(*wait.last().unwrap() <= 1_000_000, "wait max gate failed");
            assert!(percentile(&hold, 99) <= 50_000, "hold p99 gate failed");
            assert!(
                *durations.last().unwrap() <= 2 * durations.first().unwrap(),
                "writer fairness gate failed"
            );
        }
    }
}
