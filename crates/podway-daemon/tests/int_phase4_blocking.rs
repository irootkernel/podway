#![forbid(unsafe_code)]

use std::{
    num::NonZeroUsize,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use podway_daemon::blocking::{BlockingExecutorErrorV1, BlockingExecutorV1, BlockingOperationV1};

fn capacity(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("test capacities are nonzero")
}

fn record_maximum(maximum: &AtomicUsize, candidate: usize) {
    let mut observed = maximum.load(Ordering::SeqCst);
    while candidate > observed {
        match maximum.compare_exchange(observed, candidate, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return,
            Err(current) => observed = current,
        }
    }
}

#[test]
fn capacity_two_admits_exactly_two_and_holds_a_third_before_entry() {
    let executor = Arc::new(BlockingExecutorV1::new(capacity(2)));
    let entered = Arc::new(AtomicUsize::new(0));
    let first_two_entered = Arc::new(Barrier::new(3));
    let release_first_two = Arc::new(Barrier::new(3));

    let mut first_two = Vec::new();
    for operation in [
        BlockingOperationV1::GitResolve,
        BlockingOperationV1::StoreRead,
    ] {
        let executor = Arc::clone(&executor);
        let entered = Arc::clone(&entered);
        let first_two_entered = Arc::clone(&first_two_entered);
        let release_first_two = Arc::clone(&release_first_two);
        first_two.push(thread::spawn(move || {
            executor.run(operation, || {
                entered.fetch_add(1, Ordering::SeqCst);
                first_two_entered.wait();
                release_first_two.wait();
            })
        }));
    }

    first_two_entered.wait();
    assert_eq!(entered.load(Ordering::SeqCst), 2);
    assert_eq!(executor.capacity(), 2);
    assert_eq!(executor.active(), Ok(2));

    let (third_attempting_sender, third_attempting_receiver) = mpsc::channel();
    let third_entered = Arc::new(AtomicBool::new(false));
    let third_executor = Arc::clone(&executor);
    let third_entered_for_thread = Arc::clone(&third_entered);
    let third = thread::spawn(move || {
        third_attempting_sender
            .send(())
            .expect("the test must observe the third submission");
        third_executor.run(BlockingOperationV1::StoreWrite, || {
            third_entered_for_thread.store(true, Ordering::SeqCst);
        })
    });

    third_attempting_receiver
        .recv()
        .expect("the third submission must be attempted");
    assert_eq!(executor.active(), Ok(2));
    assert!(!third_entered.load(Ordering::SeqCst));

    release_first_two.wait();
    for handle in first_two {
        assert_eq!(handle.join().expect("worker must not panic"), Ok(()));
    }
    assert_eq!(third.join().expect("third worker must not panic"), Ok(()));
    assert!(third_entered.load(Ordering::SeqCst));
    assert_eq!(executor.active(), Ok(0));
}

#[test]
fn every_blocking_operation_class_uses_the_same_budget() {
    let executor = Arc::new(BlockingExecutorV1::new(capacity(1)));
    let held_permit = executor
        .acquire(BlockingOperationV1::GitResolve)
        .expect("the initial permit must be admitted");
    let operations = [
        BlockingOperationV1::GitResolve,
        BlockingOperationV1::StoreInspect,
        BlockingOperationV1::StoreOpen,
        BlockingOperationV1::StoreRead,
        BlockingOperationV1::StoreWrite,
        BlockingOperationV1::ConfigRead,
        BlockingOperationV1::ProcedurePrepare,
        BlockingOperationV1::ArtifactHash,
        BlockingOperationV1::RegistryIo,
    ];
    let start = Arc::new(Barrier::new(operations.len() + 1));
    let active_inside_work = Arc::new(AtomicUsize::new(0));
    let maximum_inside_work = Arc::new(AtomicUsize::new(0));
    let (entered_sender, entered_receiver) = mpsc::channel();

    let mut workers = Vec::new();
    for operation in operations {
        let executor = Arc::clone(&executor);
        let start = Arc::clone(&start);
        let active_inside_work = Arc::clone(&active_inside_work);
        let maximum_inside_work = Arc::clone(&maximum_inside_work);
        let entered_sender = entered_sender.clone();
        workers.push(thread::spawn(move || {
            start.wait();
            executor.run(operation, || {
                let active = active_inside_work.fetch_add(1, Ordering::SeqCst) + 1;
                record_maximum(&maximum_inside_work, active);
                entered_sender
                    .send(operation)
                    .expect("the test must receive every entered operation");
                active_inside_work.fetch_sub(1, Ordering::SeqCst);
            })
        }));
    }
    drop(entered_sender);

    start.wait();
    assert_eq!(executor.active(), Ok(1));
    drop(held_permit);

    for worker in workers {
        assert_eq!(worker.join().expect("worker must not panic"), Ok(()));
    }

    let mut observed: Vec<_> = entered_receiver.iter().collect();
    let mut expected = operations.to_vec();
    observed.sort_unstable();
    expected.sort_unstable();
    assert_eq!(observed, expected);
    assert_eq!(maximum_inside_work.load(Ordering::SeqCst), 1);
    assert_eq!(executor.active(), Ok(0));
}

#[test]
fn permits_release_after_error_and_panic_unwind() {
    let executor = BlockingExecutorV1::new(capacity(1));

    assert_eq!(
        executor.run(BlockingOperationV1::ConfigRead, || Err::<(), _>(
            "expected error"
        )),
        Ok(Err("expected error"))
    );
    assert_eq!(executor.active(), Ok(0));
    assert_eq!(
        executor.run(BlockingOperationV1::ProcedurePrepare, || 17_usize),
        Ok(17)
    );
    assert_eq!(executor.active(), Ok(0));

    let panic_result = catch_unwind(AssertUnwindSafe(|| {
        let _ = executor.run(BlockingOperationV1::ArtifactHash, || -> () {
            panic!("expected panic")
        });
    }));
    assert!(panic_result.is_err());
    assert_eq!(executor.active(), Ok(0));
    assert_eq!(
        executor.run(BlockingOperationV1::RegistryIo, || 23_usize),
        Ok(23)
    );
    assert_eq!(executor.active(), Ok(0));
}

#[test]
fn shutdown_wakes_waiters_rejects_new_work_and_allows_active_work_to_finish() {
    let executor = Arc::new(BlockingExecutorV1::new(capacity(1)));
    let active_entered = Arc::new(Barrier::new(2));
    let finish_active_work = Arc::new(Barrier::new(2));
    let active_executor = Arc::clone(&executor);
    let active_entered_for_thread = Arc::clone(&active_entered);
    let finish_active_work_for_thread = Arc::clone(&finish_active_work);
    let active_worker = thread::spawn(move || {
        active_executor.run(BlockingOperationV1::StoreOpen, || {
            active_entered_for_thread.wait();
            finish_active_work_for_thread.wait();
        })
    });

    active_entered.wait();
    assert_eq!(executor.active(), Ok(1));

    let waiter_ready = Arc::new(Barrier::new(2));
    let waiter_executor = Arc::clone(&executor);
    let waiter_ready_for_thread = Arc::clone(&waiter_ready);
    let (waiter_result_sender, waiter_result_receiver) = mpsc::channel();
    let waiter = thread::spawn(move || {
        waiter_ready_for_thread.wait();
        waiter_result_sender
            .send(
                waiter_executor
                    .acquire(BlockingOperationV1::StoreRead)
                    .map(|_| ()),
            )
            .expect("the test must receive the waiting caller result");
    });

    waiter_ready.wait();
    executor.shutdown().expect("shutdown must succeed");
    assert_eq!(executor.is_shutdown(), Ok(true));
    assert_eq!(executor.active(), Ok(1));
    assert_eq!(
        waiter_result_receiver
            .recv()
            .expect("shutdown must release waiting callers"),
        Err(BlockingExecutorErrorV1::Shutdown)
    );
    assert_eq!(executor.active(), Ok(1));

    finish_active_work.wait();
    assert_eq!(
        active_worker.join().expect("active worker must not panic"),
        Ok(())
    );
    waiter.join().expect("waiting caller must not panic");
    assert_eq!(executor.active(), Ok(0));

    let new_work_entered = AtomicBool::new(false);
    assert_eq!(
        executor.run(BlockingOperationV1::RegistryIo, || {
            new_work_entered.store(true, Ordering::SeqCst);
        }),
        Err(BlockingExecutorErrorV1::Shutdown)
    );
    assert!(!new_work_entered.load(Ordering::SeqCst));
}
