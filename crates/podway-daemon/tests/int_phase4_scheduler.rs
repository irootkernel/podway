//! Phase 4 durable-identity scheduler lifecycle contracts.
//!

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use podway_core::{Sha256Digest, WorkspaceId};
use podway_daemon::DaemonCompositionErrorV1;
use podway_daemon::scheduler::{
    WorkspaceSchedulerKeyV1, WorkspaceSchedulerRegistryV1, WorkspaceSchedulerRetirementErrorV1,
    WorkspaceSchedulerRetirementStartErrorV1,
};
use podway_store::DurableWorktreeIdentityV1;

fn identity(number: u64) -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        Sha256Digest::new(format!("sha256:{}", "a".repeat(64)))
            .expect("fixture common-directory digest is valid"),
        WorkspaceId::new(format!("00000000-0000-0000-0000-{number:012x}"))
            .expect("fixture workspace UUID is valid"),
        Sha256Digest::new(format!("sha256:{}", "b".repeat(64)))
            .expect("fixture worktree-administration digest is valid"),
    )
}

fn key(number: u64) -> WorkspaceSchedulerKeyV1 {
    WorkspaceSchedulerKeyV1::from_durable_identity(&identity(number))
}

#[test]
fn thirty_two_durable_identity_aliases_share_one_scheduler_and_run_factories_unlocked() {
    let registry = Arc::new(WorkspaceSchedulerRegistryV1::new());
    let durable_identity = Arc::new(identity(1));
    let nested_key = key(2);
    let factory_calls = Arc::new(AtomicUsize::new(0));
    let start = Arc::new(Barrier::new(33));
    let (sender, receiver) = mpsc::channel();
    let mut calls = Vec::new();

    for _ in 0..32 {
        let registry = Arc::clone(&registry);
        let durable_identity = Arc::clone(&durable_identity);
        let nested_key = nested_key.clone();
        let factory_calls = Arc::clone(&factory_calls);
        let start = Arc::clone(&start);
        let sender = sender.clone();
        calls.push(thread::spawn(move || {
            start.wait();
            let alias = WorkspaceSchedulerKeyV1::from_durable_identity(&durable_identity);
            let factory_registry = Arc::clone(&registry);
            let scheduler = registry
                .get_or_create(alias, move || {
                    factory_calls.fetch_add(1, Ordering::SeqCst);
                    let nested = factory_registry
                        .get_or_create(nested_key, || 99_usize)
                        .expect("factory may re-enter the registry for another durable key");
                    assert_eq!(*nested.context_snapshot(), 99);
                    7_usize
                })
                .expect("initial scheduler generation is valid");
            sender
                .send(scheduler)
                .expect("main test thread receives every alias scheduler");
        }));
    }
    drop(sender);

    start.wait();
    let schedulers: Vec<_> = receiver.iter().collect();
    for call in calls {
        call.join().expect("alias caller does not panic");
    }

    assert_eq!(factory_calls.load(Ordering::SeqCst), 1);
    assert_eq!(schedulers.len(), 32);
    assert!(
        schedulers
            .iter()
            .all(|scheduler| Arc::ptr_eq(scheduler, &schedulers[0]))
    );
    assert_eq!(schedulers[0].generation().get(), 1);
    assert_eq!(
        schedulers[0].key().workspace_uuid(),
        durable_identity.workspace_uuid()
    );
    assert_eq!(
        schedulers[0].key().common_directory_digest(),
        durable_identity.common_dir_identity()
    );
    assert_eq!(
        schedulers[0].key().worktree_administration_digest(),
        durable_identity.worktree_admin_identity()
    );
}

#[test]
fn progress_wait_rechecks_the_authoritative_predicate_after_every_hint() {
    let registry = WorkspaceSchedulerRegistryV1::new();
    let scheduler = registry
        .get_or_create(key(4), || ())
        .expect("initial scheduler generation is valid");
    let observed = scheduler.progress_version();
    let should_keep_waiting = Arc::new(AtomicBool::new(true));
    let checks = Arc::new(AtomicUsize::new(0));
    let (first_check_sender, first_check_receiver) = mpsc::channel();
    let (recheck_sender, recheck_receiver) = mpsc::channel();
    let (final_check_sender, final_check_receiver) = mpsc::channel();
    let (final_check_release_sender, final_check_release_receiver) = mpsc::channel();
    let waiter = {
        let scheduler = Arc::clone(&scheduler);
        let should_keep_waiting = Arc::clone(&should_keep_waiting);
        let checks = Arc::clone(&checks);
        thread::spawn(move || {
            scheduler.wait_for_progress_while(observed, || {
                match checks.fetch_add(1, Ordering::SeqCst) {
                    0 => first_check_sender
                        .send(())
                        .expect("main test thread observes the initial predicate check"),
                    1 => {
                        recheck_sender
                            .send(())
                            .expect("main test thread observes the hint-driven predicate recheck");
                        final_check_release_receiver
                            .recv()
                            .expect("main test thread releases the final predicate check");
                        final_check_sender
                            .send(())
                            .expect("main test thread observes the final predicate check");
                    }
                    _ => {}
                }
                should_keep_waiting.load(Ordering::SeqCst)
            })
        })
    };

    first_check_receiver
        .recv()
        .expect("waiter evaluates its predicate before blocking");
    assert_eq!(
        scheduler
            .notify_progress()
            .expect("first progress version advances")
            .get(),
        1
    );
    recheck_receiver
        .recv()
        .expect("progress hint causes another predicate check");
    should_keep_waiting.store(false, Ordering::SeqCst);
    assert_eq!(
        scheduler
            .notify_progress()
            .expect("second progress version advances")
            .get(),
        2
    );
    final_check_release_sender
        .send(())
        .expect("waiter remains available for its final predicate check");
    final_check_receiver
        .recv()
        .expect("final notification causes the predicate's completion check");

    assert_eq!(
        waiter.join().expect("waiter does not panic").get(),
        2,
        "the returned version is only observed after the final predicate recheck"
    );
    assert!(checks.load(Ordering::SeqCst) >= 2);
}

#[test]
fn same_key_serializes_while_different_keys_overlap() {
    let registry = WorkspaceSchedulerRegistryV1::new();
    let same_key_scheduler = registry
        .get_or_create(key(5), || ())
        .expect("initial scheduler generation is valid");
    let (entered_sender, entered_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let held_scheduler = Arc::clone(&same_key_scheduler);
    let holder = thread::spawn(move || {
        held_scheduler.with_serialized(|_| {
            entered_sender
                .send(())
                .expect("main test thread observes the held serialization mutex");
            release_receiver
                .recv()
                .expect("main test thread releases the held serialization mutex");
        });
    });

    entered_receiver
        .recv()
        .expect("holder acquires the same-key serialization mutex");
    assert_eq!(same_key_scheduler.try_with_serialized(|_| ()), None);
    release_sender
        .send(())
        .expect("holder remains available to release");
    holder.join().expect("holder does not panic");
    assert_eq!(same_key_scheduler.try_with_serialized(|_| 1), Some(1));

    let first_key_scheduler = registry
        .get_or_create(key(6), || ())
        .expect("first distinct scheduler generation is valid");
    let second_key_scheduler = registry
        .get_or_create(key(7), || ())
        .expect("second distinct scheduler generation is valid");
    let entered = Arc::new(AtomicUsize::new(0));
    let observed_overlap = Arc::new(AtomicBool::new(false));
    let overlap_barrier = Arc::new(Barrier::new(2));
    let first = {
        let entered = Arc::clone(&entered);
        let observed_overlap = Arc::clone(&observed_overlap);
        let overlap_barrier = Arc::clone(&overlap_barrier);
        thread::spawn(move || {
            first_key_scheduler.with_serialized(|_| {
                if entered.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                    observed_overlap.store(true, Ordering::SeqCst);
                }
                overlap_barrier.wait();
                entered.fetch_sub(1, Ordering::SeqCst);
            });
        })
    };
    let second = {
        let entered = Arc::clone(&entered);
        let observed_overlap = Arc::clone(&observed_overlap);
        let overlap_barrier = Arc::clone(&overlap_barrier);
        thread::spawn(move || {
            second_key_scheduler.with_serialized(|_| {
                if entered.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                    observed_overlap.store(true, Ordering::SeqCst);
                }
                overlap_barrier.wait();
                entered.fetch_sub(1, Ordering::SeqCst);
            });
        })
    };
    first.join().expect("first distinct key does not panic");
    second.join().expect("second distinct key does not panic");

    assert!(
        observed_overlap.load(Ordering::SeqCst),
        "different durable keys overlap instead of sharing one serialization mutex"
    );
}

#[test]
fn retirement_blocks_recreation_until_close_then_advances_generation() {
    let registry = Arc::new(WorkspaceSchedulerRegistryV1::new());
    let durable_key = key(8);
    let scheduler = registry
        .get_or_create(durable_key.clone(), || 1_usize)
        .expect("initial scheduler generation is valid");
    let nested_key = key(9);
    let closing = Arc::new(AtomicBool::new(false));
    let factory_calls = Arc::new(AtomicUsize::new(0));
    let (close_started_sender, close_started_receiver) = mpsc::channel();
    let (close_release_sender, close_release_receiver) = mpsc::channel();
    let retirement = {
        let registry = Arc::clone(&registry);
        let callback_registry = Arc::clone(&registry);
        let scheduler = Arc::clone(&scheduler);
        let closing = Arc::clone(&closing);
        thread::spawn(move || {
            registry.retire(&scheduler, move |_| {
                let nested = callback_registry
                    .get_or_create(nested_key, || 3_usize)
                    .expect("close/drain callback re-enters the registry without a map lock");
                assert_eq!(*nested.context_snapshot(), 3);
                closing.store(true, Ordering::SeqCst);
                close_started_sender
                    .send(())
                    .expect("main test thread observes the retiring slot");
                close_release_receiver
                    .recv()
                    .expect("main test thread releases close/drain");
                closing.store(false, Ordering::SeqCst);
                Ok::<(), &'static str>(())
            })
        })
    };

    close_started_receiver
        .recv()
        .expect("close/drain starts after the retiring mark is visible");
    let (started_sender, started_receiver) = mpsc::channel();
    let recreation = {
        let registry = Arc::clone(&registry);
        let closing = Arc::clone(&closing);
        let factory_calls = Arc::clone(&factory_calls);
        let durable_key = durable_key.clone();
        thread::spawn(move || {
            started_sender
                .send(())
                .expect("main test thread observes the recreation request");
            registry.get_or_create(durable_key, move || {
                assert!(
                    !closing.load(Ordering::SeqCst),
                    "recreation factory must not run while the prior generation is retiring"
                );
                factory_calls.fetch_add(1, Ordering::SeqCst);
                2_usize
            })
        })
    };

    started_receiver
        .recv()
        .expect("recreation request begins while the prior generation is retiring");
    close_release_sender
        .send(())
        .expect("retiring callback remains available to complete");
    retirement
        .join()
        .expect("retirement thread does not panic")
        .expect("close/drain completion removes the exact retiring generation");
    let replacement = recreation
        .join()
        .expect("recreation thread does not panic")
        .expect("replacement scheduler generation is valid");

    assert_eq!(factory_calls.load(Ordering::SeqCst), 1);
    assert_eq!(scheduler.generation().get(), 1);
    assert_eq!(replacement.generation().get(), 2);
    assert!(!Arc::ptr_eq(&scheduler, &replacement));
}

#[test]
fn failed_close_stays_retiring_until_its_typed_retry_succeeds() {
    let registry = WorkspaceSchedulerRegistryV1::new();
    let durable_key = key(10);
    let scheduler = registry
        .get_or_create(durable_key.clone(), || ())
        .expect("initial scheduler generation is valid");

    let retry = match registry.retire(&scheduler, |_| Err::<(), _>("drain failed")) {
        Err(WorkspaceSchedulerRetirementErrorV1::CloseFailed { source, retry }) => {
            assert_eq!(source, "drain failed");
            retry
        }
        _ => panic!("failed close/drain must return a typed retry capability"),
    };
    match registry.retire(&scheduler, |_| Ok::<(), &'static str>(())) {
        Err(WorkspaceSchedulerRetirementErrorV1::Start(
            WorkspaceSchedulerRetirementStartErrorV1::AlreadyRetiring { retry: duplicate },
        )) => assert_eq!(duplicate.generation(), scheduler.generation()),
        _ => panic!("failed close/drain must remain fail-closed and retiring"),
    }
    assert!(matches!(
        registry.get_or_create(durable_key.clone(), || ()),
        Err(DaemonCompositionErrorV1::SchedulerRetiring { generation })
            if generation == scheduler.generation()
    ));

    retry
        .retry(|_| Ok::<(), &'static str>(()))
        .expect("typed retry closes and removes the still-retiring generation");
    let replacement = registry
        .get_or_create(durable_key, || ())
        .expect("replacement generation is valid after retry completion");
    assert_eq!(replacement.generation().get(), 2);
}
#[test]
fn panicking_close_releases_waiters_and_preserves_public_retry() {
    let registry = Arc::new(WorkspaceSchedulerRegistryV1::new());
    let durable_key = key(12);
    let scheduler = registry
        .get_or_create(durable_key.clone(), || ())
        .expect("initial scheduler generation is valid");
    let (close_started_sender, close_started_receiver) = mpsc::channel();
    let (close_release_sender, close_release_receiver) = mpsc::channel();
    let retirement = {
        let registry = Arc::clone(&registry);
        let scheduler = Arc::clone(&scheduler);
        thread::spawn(move || {
            catch_unwind(AssertUnwindSafe(|| {
                registry.retire(&scheduler, |_| -> Result<(), &'static str> {
                    close_started_sender
                        .send(())
                        .expect("main test thread observes the close/drain callback");
                    close_release_receiver
                        .recv()
                        .expect("main test thread releases the panicking callback");
                    panic!("close/drain panicked");
                })
            }))
        })
    };
    close_started_receiver
        .recv()
        .expect("close/drain starts before the waiter requests recreation");

    let (waiter_started_sender, waiter_started_receiver) = mpsc::channel();
    let (waiter_result_sender, waiter_result_receiver) = mpsc::channel();
    let waiter = {
        let registry = Arc::clone(&registry);
        thread::spawn(move || {
            waiter_started_sender
                .send(())
                .expect("main test thread observes the recreation request");
            waiter_result_sender
                .send(registry.get_or_create(durable_key, || ()))
                .expect("main test thread receives the bounded recreation result");
        })
    };
    waiter_started_receiver
        .recv()
        .expect("recreation request begins while close/drain is in progress");
    close_release_sender
        .send(())
        .expect("panicking callback remains available to release");
    assert!(
        retirement
            .join()
            .expect("retirement thread catches the callback panic")
            .is_err(),
        "the callback panic must resume only after retirement state is repaired"
    );

    let waiter_result = waiter_result_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("notified waiter must return after the failed close");
    waiter.join().expect("waiter thread does not panic");
    assert!(matches!(
        waiter_result,
        Err(DaemonCompositionErrorV1::SchedulerRetiring { generation })
            if generation == scheduler.generation()
    ));

    let retry = match registry.retire(&scheduler, |_| Ok::<(), &'static str>(())) {
        Err(WorkspaceSchedulerRetirementErrorV1::Start(
            WorkspaceSchedulerRetirementStartErrorV1::AlreadyRetiring { retry },
        )) => retry,
        _ => panic!("a panicking close must leave an explicit public retry capability"),
    };
    retry
        .retry(|_| Ok::<(), &'static str>(()))
        .expect("public retry removes the repaired retiring generation");
    let replacement = registry
        .get_or_create(scheduler.key().clone(), || ())
        .expect("public retry permits a replacement generation");
    assert_eq!(replacement.generation().get(), 2);
    assert!(!Arc::ptr_eq(&replacement, &scheduler));
}

#[test]
fn stale_retirement_cannot_remove_a_replacement_generation() {
    let registry = WorkspaceSchedulerRegistryV1::new();
    let durable_key = key(11);
    let retiring_scheduler = registry
        .get_or_create(durable_key.clone(), || ())
        .expect("initial scheduler generation is valid");
    registry
        .retire(&retiring_scheduler, |_| Ok::<(), &'static str>(()))
        .expect("initial retirement removes its exact generation");
    let replacement = registry
        .get_or_create(durable_key.clone(), || ())
        .expect("replacement generation is valid");
    let close_called = Arc::new(AtomicBool::new(false));

    match registry.retire(&retiring_scheduler, {
        let close_called = Arc::clone(&close_called);
        move |_| {
            close_called.store(true, Ordering::SeqCst);
            Ok::<(), &'static str>(())
        }
    }) {
        Err(WorkspaceSchedulerRetirementErrorV1::Start(
            WorkspaceSchedulerRetirementStartErrorV1::NotCurrent { .. },
        )) => {}
        _ => panic!("a stale retirement must not start close/drain for the replacement"),
    }

    assert!(!close_called.load(Ordering::SeqCst));
    let current = registry
        .get_or_create(durable_key, || ())
        .expect("replacement remains registered");
    assert!(Arc::ptr_eq(&current, &replacement));
    assert_eq!(replacement.generation().get(), 2);
}
