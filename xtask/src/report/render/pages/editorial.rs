//! Study-specific editorial pages, kept with the private reproducibility consumer.

use std::path::Path;
mod guide;
use super::{Publication, Result, case_html, q, table};

pub(super) fn pages(
    publication: &Publication,
    _public: &Path,
) -> Result<Vec<(&'static str, String)>> {
    let correction = publication
        .cases
        .iter()
        .find(|case| case.id == "E01")
        .ok_or("explicit-flow correction case missing")?;
    let body = format!(
        "<p class=\"kicker\">Ordered correction circuit</p><h2>A finding is not the landing.</h2><p class=\"lede\">Correction work and parent re-review are separate transitions. The surviving order is the evidence—not the final clean label alone.</p>{}<div class=\"columns\"><section><h3>What the record proves</h3><p>The correction flow was explicitly requested as implementation-only. A retained command receipt proves that operand; the journal separately proves the resulting task and phase completion.</p></section><section><h3>What it does not prove</h3><p>The same lane authored implementation and review. Filing, authorship and review remain separate roles; a clean verdict is not evidence of independent scrutiny.</p></section></div><p class=\"scope\">This page belongs to the fixed study of 1,500 distinct completed births. Cases use hour-coarsened public instants. Canonical sequence order and original identifiers remain in the private evidence.</p><p class=\"evidence\">E01 · Q12–Q16 · Complete definitions, denominators and ordered support: tables.html</p><details><summary>Closure route support</summary>{}</details>",
        case_html(correction),
        table(q(publication, "Q02"), None)
    );
    guide::pages(publication, _public, body)
}
