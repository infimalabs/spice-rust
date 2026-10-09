//! Off-loop supervisor proof establishment and resident-owned continuations.

use tokio::sync::mpsc;
use wire::CanonicalUuid;

use super::{CommandLaunch, CommandLaunchCompletion, CommandResult, lifecycle, supervisor};

pub(super) enum Authority {
    Command {
        request: serve::axum::ControlRequest,
        completion: CommandLaunchCompletion,
        lane: CanonicalUuid,
        lifecycle_released: bool,
        command: CanonicalUuid,
        epoch: CanonicalUuid,
    },
    Automatic(lifecycle::PendingLaunch),
}

pub(super) struct Completion {
    pub(super) authority: Authority,
    pub(super) result: CommandResult<supervisor::Launch>,
}

pub(super) struct Runtime {
    sender: mpsc::UnboundedSender<Completion>,
    receiver: mpsc::UnboundedReceiver<Completion>,
}

impl Runtime {
    pub(super) fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        Self { sender, receiver }
    }

    pub(super) fn start_command(
        &self,
        launch: CommandLaunch,
        request: serve::axum::ControlRequest,
        lifecycle_released: bool,
        command: CanonicalUuid,
        epoch: CanonicalUuid,
    ) {
        self.start(
            launch.launch,
            Authority::Command {
                request,
                completion: launch.completion,
                lane: launch.lane,
                lifecycle_released,
                command,
                epoch,
            },
        );
    }

    pub(super) fn start_automatic(
        &self,
        launch: supervisor::PendingLaunch,
        pending: lifecycle::PendingLaunch,
    ) {
        self.start(launch, Authority::Automatic(pending));
    }

    fn start(&self, launch: supervisor::PendingLaunch, authority: Authority) {
        let completed = self.sender.clone();
        tokio::task::spawn_blocking(move || {
            let result = launch.establish();
            let _resident_stopped = completed.send(Completion { authority, result });
        });
    }

    pub(super) async fn recv(&mut self) -> Completion {
        self.receiver
            .recv()
            .await
            .expect("launch completion sender remains owned by its runtime")
    }
}
