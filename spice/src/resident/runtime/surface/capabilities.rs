use super::*;

pub(super) fn build(
    operator: CanonicalUuid,
    residency: &ResidencyHead,
) -> CommandResult<serve::axum::Capabilities> {
    let capabilities = serve::axum::Capabilities::default();
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
