//! Resident enforcement that intentionally remains below connected wire laws.

use super::*;

#[test]
fn a_lane_cannot_recover_operator_direction() {
    let fixture = Fixture::new("steering-authority");
    let mut initial = RunningResident::spawn(&fixture);
    initial.assert_ready(&fixture);
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "initial down failed: {:?}",
        down.stderr
    );
    assert!(initial.wait().status.success(), "initial resident failed");

    let lane = uuid("00000000-0000-4000-8000-000000000071");
    let capability = uuid("00000000-0000-4000-8000-000000000072");
    let directive = uuid("00000000-0000-4000-8000-000000000073");
    let request = uuid("00000000-0000-4000-8000-000000000074");
    seed_lane(&fixture, lane, capability);

    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let discovery = fixture.discovery();
    let (mut socket, accepted) = authenticate(&discovery, capability);
    assert_eq!(accepted.actor, Actor::Lane(lane));
    let command = wire::Command {
        v: wire::WireVersion::CURRENT,
        id: request,
        epoch: discovery.epoch,
        kind: wire::CommandKind::SteerResend,
        expected: vec![
            wire::Witness::Directive {
                steer: directive,
                disposition: wire::Disposition::Lapsed,
            },
            wire::Witness::Lineage { steer: directive },
        ],
        body: wire::intent::DirectiveRef { directive },
    };
    socket
        .send(Message::Binary(
            wire::encode(&command)
                .expect("lane recovery command encodes")
                .into(),
        ))
        .expect("lane recovery command sends");
    let frame = socket
        .read()
        .expect("lane recovery response arrives")
        .into_data();
    let response: wire::Response<serde_json::Value> =
        wire::decode(&frame).expect("lane recovery response decodes");
    assert_eq!((response.id, response.epoch), (request, discovery.epoch));
    assert!(matches!(
        response.outcome,
        Outcome::Refusal(wire::Refusal { ref reason, .. })
            if reason == "steer::operator-required"
    ));

    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "final down failed: {:?}",
        down.stderr
    );
    assert!(resident.wait().status.success(), "final resident failed");
}
