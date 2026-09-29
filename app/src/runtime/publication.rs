//! Constant-cost publication of the mutable Canvas runtime to Yew.

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
use super::CanvasRuntime;

/// One shared allocation whose handle is the value published to Yew.
///
/// The connection owns the mutation path and the component owns a clone of
/// this handle. Publishing therefore clones one `Rc`, never the retained
/// runtime behind it. `Shared` is generic so its native regression can put
/// clone-counting payloads behind the exact production ownership primitive.
struct Shared<T>(Rc<RefCell<T>>);

impl<T: Default> Default for Shared<T> {
    fn default() -> Self {
        Self(Rc::new(RefCell::new(T::default())))
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(Rc::clone(&self.0))
    }
}

impl<T> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl<T> Shared<T> {
    fn read(&self) -> Ref<'_, T> {
        self.0.borrow()
    }

    fn write(&self) -> RefMut<'_, T> {
        self.0.borrow_mut()
    }
}

/// The runtime cell and the constant-cost handle published after a mutation.
///
/// Canvas stores this in Yew's `use_state`, whose setter always rerenders; it
/// must not move to `use_state_eq`, because two publications intentionally
/// point at the same allocation.
#[derive(Clone, Default, PartialEq)]
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub(crate) struct CanvasSurface(Shared<Option<CanvasRuntime>>);

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
impl CanvasSurface {
    pub(crate) fn read(&self) -> Ref<'_, Option<CanvasRuntime>> {
        self.0.read()
    }

    pub(crate) fn write(&self) -> RefMut<'_, Option<CanvasRuntime>> {
        self.0.write()
    }

    /// Produce the next Yew state value without copying retained runtime data.
    pub(crate) fn publication(&self) -> Self {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::Shared;

    static PAYLOAD_CLONES: AtomicUsize = AtomicUsize::new(0);

    struct RetainedPayload(String);

    impl Clone for RetainedPayload {
        fn clone(&self) -> Self {
            PAYLOAD_CLONES.fetch_add(1, Ordering::Relaxed);
            Self(self.0.clone())
        }
    }

    #[derive(Default)]
    struct RuntimeFixture {
        cursor: usize,
        lanes: Vec<Vec<RetainedPayload>>,
    }

    fn publication_payload_clones(lanes: usize, rows_per_lane: usize) -> usize {
        let retained = (0..lanes)
            .map(|lane| {
                (0..rows_per_lane)
                    .map(|row| RetainedPayload(format!("lane-{lane}-row-{row}")))
                    .collect()
            })
            .collect();
        let runtime = Shared(Rc::new(RefCell::new(RuntimeFixture {
            cursor: 0,
            lanes: retained,
        })));
        PAYLOAD_CLONES.store(0, Ordering::Relaxed);

        // The same one-field frame mutation is applied to both fixtures.
        runtime.write().cursor += 1;
        let published = runtime.clone();

        assert!(Rc::ptr_eq(&runtime.0, &published.0));
        assert_eq!(published.read().cursor, 1);
        assert_eq!(published.read().lanes.len(), lanes);
        PAYLOAD_CLONES.load(Ordering::Relaxed)
    }

    #[test]
    fn publication_clones_no_payload_for_small_or_full_multilane_retention() {
        assert_eq!(publication_payload_clones(1, 1), 0);
        assert_eq!(publication_payload_clones(32, 256), 0);
    }
}
