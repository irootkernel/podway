//! Test-only observation of the actual descriptor-backed artifact read boundary.
//! Enabled only by explicit test-support consumers; no product setting selects it.

use std::{cell::RefCell, path::Path, rc::Rc};

type ArtifactReadObserver = Rc<dyn Fn(&Path, usize)>;

thread_local! {
    static ARTIFACT_READ_OBSERVER: RefCell<Option<ArtifactReadObserver>> = const { RefCell::new(None) };
}

/// Observes every successful native artifact file read, including EOF, on this thread only.
/// The prior observer is restored on return or unwind; parallel fixture threads are unaffected.
pub fn with_artifact_read_observer_v1<T>(
    observer: impl Fn(&Path, usize) + 'static,
    operation: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<ArtifactReadObserver>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ARTIFACT_READ_OBSERVER.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _restore =
        Restore(ARTIFACT_READ_OBSERVER.with(|slot| slot.borrow_mut().replace(Rc::new(observer))));
    operation()
}

pub(crate) fn observe_artifact_read(path: &Path, bytes: usize) {
    let observer = ARTIFACT_READ_OBSERVER.with(|slot| slot.borrow().clone());
    if let Some(observer) = observer {
        observer(path, bytes);
    }
}
