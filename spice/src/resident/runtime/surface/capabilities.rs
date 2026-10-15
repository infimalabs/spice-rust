use super::*;

/// The generation's capabilities, deciding authority by the journal's
/// current principal role heads.
pub(super) fn build(
    bus: &Bus,
    operator: CanonicalUuid,
    residency: &ResidencyHead,
) -> CommandResult<serve::axum::Capabilities> {
    let capabilities = serve::axum::Capabilities::default();
    capabilities
        .roles()
        .publish(crate::resident::principal_surface::roles(bus)?)
        .map_err(|error| Outcome::Fault(error.fault()))?;
    capabilities
        .insert(operator, Actor::Operator)
        .map_err(|error| environment_fault("serve::capability-invalid", &error.to_string()))?;
    for lane in &residency.lanes {
        for capability in &lane.capabilities {
            capabilities
                .insert(*capability, Actor::Lane(lane.lane))
                .map_err(|error| {
                    environment_fault("serve::capability-invalid", &error.to_string())
                })?;
        }
    }
    Ok(capabilities)
}
