//! Pointer ownership for one lifted primary composer.

use wire::CanonicalUuid;

const COMPOSER_DRAG_LIFT_PX: u32 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ComposerDragPointer {
    pub(crate) source: CanonicalUuid,
    pub(crate) pointer_id: i32,
    pub(crate) start_x: i32,
    pub(crate) start_y: i32,
    pub(crate) dragging: bool,
    topology_revision: u64,
}

#[derive(Debug, Default)]
pub(crate) struct ComposerDragSession {
    active: Option<ComposerDragPointer>,
}

impl ComposerDragSession {
    pub(crate) fn begin(
        &mut self,
        enabled: bool,
        source: CanonicalUuid,
        pointer_id: i32,
        start_x: i32,
        start_y: i32,
        topology_revision: u64,
    ) -> bool {
        if !enabled || self.active.is_some() {
            return false;
        }
        self.active = Some(ComposerDragPointer {
            source,
            pointer_id,
            start_x,
            start_y,
            dragging: false,
            topology_revision,
        });
        true
    }

    pub(crate) fn movement(
        &mut self,
        source: CanonicalUuid,
        pointer_id: i32,
        client_x: i32,
        client_y: i32,
        topology_revision: u64,
    ) -> Option<&mut ComposerDragPointer> {
        let active = self.active.as_mut().filter(|active| {
            active.source == source
                && active.pointer_id == pointer_id
                && active.topology_revision == topology_revision
        })?;
        if !active.dragging
            && client_x.abs_diff(active.start_x) < COMPOSER_DRAG_LIFT_PX
            && client_y.abs_diff(active.start_y) < COMPOSER_DRAG_LIFT_PX
        {
            return None;
        }
        Some(active)
    }

    pub(crate) fn finish(
        &mut self,
        source: CanonicalUuid,
        pointer_id: i32,
        topology_revision: u64,
    ) -> Option<ComposerDragPointer> {
        self.active
            .as_ref()
            .is_some_and(|active| {
                active.source == source
                    && active.pointer_id == pointer_id
                    && active.topology_revision == topology_revision
            })
            .then(|| self.active.take())
            .flatten()
    }

    pub(crate) fn cancel(&mut self, source: CanonicalUuid, pointer_id: i32) -> bool {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.source == source && active.pointer_id == pointer_id)
        {
            self.active = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn reconcile(&mut self, enabled: bool, topology_revision: Option<u64>) -> bool {
        if self
            .active
            .as_ref()
            .is_some_and(|active| !enabled || Some(active.topology_revision) != topology_revision)
        {
            self.active = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn clear(&mut self) -> bool {
        self.active.take().is_some()
    }
    pub(crate) fn source(&self) -> Option<CanonicalUuid> {
        self.active.map(|active| active.source)
    }

    pub(crate) fn dragging_source(&self) -> Option<CanonicalUuid> {
        self.active
            .filter(|active| active.dragging)
            .map(|active| active.source)
    }
}

#[cfg(test)]
mod tests {
    use super::ComposerDragSession;

    fn id(suffix: u8) -> wire::CanonicalUuid {
        format!("00000000-0000-4000-8000-{suffix:012}")
            .parse()
            .expect("fixture identity is canonical")
    }

    #[test]
    fn composer_drag_session_survives_same_topology_and_cancels_stale_context() {
        let mut session = ComposerDragSession::default();
        assert!(session.begin(true, id(1), 17, 100, 200, 7));
        assert!(session.movement(id(1), 17, 105, 205, 7).is_none());
        assert!(!session.reconcile(true, Some(7)));

        let lifted = session
            .movement(id(1), 17, 106, 200, 7)
            .expect("the lift threshold starts the same drag after a rerender");
        lifted.dragging = true;
        assert_eq!(session.dragging_source(), Some(id(1)));
        assert!(!session.reconcile(true, Some(7)));
        assert!(
            session
                .finish(id(1), 17, 7)
                .expect("the surviving drag finishes")
                .dragging
        );

        assert!(session.begin(true, id(1), 18, 0, 0, 7));
        assert!(session.reconcile(true, Some(8)));
        assert!(session.finish(id(1), 18, 8).is_none());
        assert!(session.begin(true, id(1), 19, 0, 0, 8));
        assert_eq!(session.source(), Some(id(1)));
        assert!(!session.cancel(id(2), 19));
        assert!(session.cancel(id(1), 19));
        assert!(session.begin(true, id(1), 20, 0, 0, 8));
        assert!(session.clear());
        assert!(session.begin(true, id(1), 21, 0, 0, 8));
        assert!(session.reconcile(false, Some(8)));
        assert_eq!(session.source(), None);
        assert!(!session.begin(false, id(1), 22, 0, 0, 8));
    }
}
