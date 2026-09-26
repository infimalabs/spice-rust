use std::future::Future;

#[derive(Clone, Copy)]
pub(super) enum Lead {
    Supervisor,
    Control,
}

impl Lead {
    const fn next(self) -> Self {
        match self {
            Self::Supervisor => Self::Control,
            Self::Control => Self::Supervisor,
        }
    }
}

pub(super) enum Input<Supervisor, Control> {
    Supervisor(Supervisor),
    Control(Control),
}

/// Select between the two live sources that can remain continuously ready.
///
/// A fixed supervisor-first selection strands admitted Canvas commands for as
/// long as lanes keep publishing. Rotating the lead bounds either source to
/// one intervening turn without disabling the supervisor door.
pub(super) async fn next<Supervisor, Control>(
    lead: &mut Lead,
    supervisor: impl Future<Output = Supervisor>,
    control: impl Future<Output = Control>,
) -> Input<Supervisor, Control> {
    let leading = *lead;
    let selected = match leading {
        Lead::Supervisor => tokio::select! {
            biased;
            report = supervisor => Input::Supervisor(report),
            request = control => Input::Control(request),
        },
        Lead::Control => tokio::select! {
            biased;
            request = control => Input::Control(request),
            report = supervisor => Input::Supervisor(report),
        },
    };
    *lead = leading.next();
    selected
}

#[cfg(test)]
mod tests;
