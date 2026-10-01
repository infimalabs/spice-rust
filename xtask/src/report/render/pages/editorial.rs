//! Study-specific editorial pages, kept with the private reproducibility consumer.

use std::path::Path;
mod guide;
use super::{Publication, Result, q, table};

pub(super) fn pages(
    publication: &Publication,
    _public: &Path,
) -> Result<Vec<(&'static str, String)>> {
    let body = format!(
        "<p class=\"kicker\">Ordered correction circuit</p><h2>A finding is not the landing.</h2><p class=\"lede\">Correction work and parent re-review are separate transitions. The surviving order is the evidence.</p>{}<div class=\"columns\"><section><h3>What the record proves</h3><p>The command receipt proves an implementation-only correction request. The journal independently proves its resulting task and completion, then parent re-review.</p></section><section><h3>What it does not prove</h3><p>L014 authored implementation and review. Distinct roles are not independent judgment. A final clean label is not first-pass success.</p></section></div><p class=\"evidence\">E01 · Q12–Q16 · complete ordered support</p><details><summary>Closure route support</summary>{}</details>",
        super::super::charts::sequences::correction(publication, _public, "correction-circuit")?,
        table(q(publication, "Q02"), None)
    );
    guide::pages(publication, _public, body)
}
