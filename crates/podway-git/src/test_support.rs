//! Test-only descriptor-backed artifact read observation and no-follow metadata
//! presence checks for candidate fixtures.

use std::{cell::RefCell, path::Path, rc::Rc};

/// Observe an artifact from its candidate fixture root without reading file data or following
/// symlinks. A missing path component is distinguishable from an inaccessible or unsafe path.
pub fn observe_candidate_artifact_presence(
    root: &Path,
    relative: &Path,
) -> Result<crate::LocalPathPresenceV1, crate::GitResolverErrorV1> {
    crate::native::observe_candidate_artifact_presence(root, relative)
}

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
