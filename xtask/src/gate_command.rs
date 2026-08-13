//! Focused and partial gate command surfaces.

use std::io::Write;

use crate::GateArmError;

/// Execute the public partial gate-battery action.
///
/// This action owns a fresh capture and deliberately does not share the full
/// verifier's retained compiler evidence. The boolean is true for admission
/// and false for a policy refusal; the CLI owns their distinct exit codes.
///
/// # Errors
///
/// Returns gate acquisition or refusal-output failures.
pub fn verify_battery_child(output: &mut impl Write) -> Result<bool, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let report =
            spice::gate::run(gate::Selector::staged()).map_err(|error| error.to_string())?;
        if report.admits() {
            Ok(true)
        } else {
            output
                .write_all(report.refusal_fold().as_bytes())
                .map_err(|error| error.to_string())?;
            Ok(false)
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = output;
        Err("the canonical native Rust gate battery is unavailable on wasm32".to_owned())
    }
}

/// Execute the ordinary fail-closed staged-change gate.
///
/// Progress is buffered so a clean hook remains silent. A refusal publishes
/// the exact selection/test evidence or the compact focused-arm fold; the
/// complete battery remains available through [`verify_battery_child`].
///
/// # Errors
///
/// Returns selection, test, policy, or refusal-output failures.
pub fn verify_selection_child(output: &mut impl Write) -> Result<bool, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut progress = Vec::new();
        let report = match spice::change_gate::run(&mut progress) {
            Ok(report) => report,
            Err(error) => {
                output
                    .write_all(&progress)
                    .map_err(|fault| fault.to_string())?;
                if error.is_fault() {
                    return Err(error.to_string());
                }
                writeln!(output, "{}", error).map_err(|fault| fault.to_string())?;
                return Ok(false);
            }
        };
        if report.admits() {
            return Ok(true);
        }
        output
            .write_all(&progress)
            .map_err(|error| error.to_string())?;
        output
            .write_all(report.refusal_fold().as_bytes())
            .map_err(|error| error.to_string())?;
        Ok(false)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = output;
        Err("the ordinary native Rust change gate is unavailable on wasm32".to_owned())
    }
}

/// Run one canonical gate arm and emit only its bounded diagnostic on refusal.
///
/// # Errors
///
/// Returns acquisition, output, or non-admitting arm failures.
pub fn gate_arm(gate_id: gate::GateId, output: &mut impl Write) -> Result<(), GateArmError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let report = spice::gate::run_arm_report(gate::Selector::staged(), gate_id)
            .map_err(GateArmError::Run)?;
        if !report.admits() {
            output
                .write_all(report.refusal_fold().as_bytes())
                .map_err(GateArmError::Output)?;
            return Err(GateArmError::Refused(gate_id));
        }
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (gate_id, output);
        Err(GateArmError::Unavailable)
    }
}

/// Run one canonical gate arm and emit its complete canonical focused report.
///
/// # Errors
///
/// Returns acquisition, encoding, output, or non-admitting arm failures.
pub fn gate_arm_report(gate_id: gate::GateId, output: &mut impl Write) -> Result<(), GateArmError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let report = spice::gate::run_arm_report(gate::Selector::staged(), gate_id)
            .map_err(GateArmError::Run)?;
        let document = report.encode().map_err(GateArmError::Encode)?;
        output
            .write_all(&document)
            .and_then(|()| output.write_all(b"\n"))
            .map_err(GateArmError::Output)?;
        if !report.admits() {
            return Err(GateArmError::Refused(gate_id));
        }
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (gate_id, output);
        Err(GateArmError::Unavailable)
    }
}
