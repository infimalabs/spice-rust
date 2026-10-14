use std::collections::BTreeMap;

use crate::Entry;

/// One hidden-stem group derived from admitted entry classifications.
#[derive(Clone, Debug)]
pub struct HiddenInventory<'a> {
    stem: &'a str,
    entries: Vec<&'a Entry>,
}

impl<'a> HiddenInventory<'a> {
    /// Resolved hidden stem without its leading dot.
    #[must_use]
    pub const fn stem(&self) -> &'a str {
        self.stem
    }

    /// Rows classified under this stem, in input order.
    #[must_use]
    pub fn entries(&self) -> &[&'a Entry] {
        &self.entries
    }
}

/// Active filed public routes, parked OOPS evidence, and hidden system rows.
#[derive(Clone, Debug)]
pub struct Inventory<'a> {
    public: Vec<&'a Entry>,
    oops: Vec<&'a Entry>,
    hidden: BTreeMap<&'a str, Vec<&'a Entry>>,
}

impl<'a> Inventory<'a> {
    /// Classify pending rows using each entry's stored answer.
    #[must_use]
    pub fn new(entries: &'a [Entry]) -> Self {
        let mut public = Vec::new();
        let mut oops = Vec::new();
        let mut hidden = BTreeMap::<&str, Vec<&Entry>>::new();
        for entry in entries {
            if entry.workflow().status != "pending" || entry.workflow().completion.is_some() {
                continue;
            }
            if entry.is_oops() {
                oops.push(entry);
            } else if let Some(stem) = entry.project().visibility().hidden_stem() {
                hidden.entry(stem).or_default().push(entry);
            } else {
                public.push(entry);
            }
        }
        Self {
            public,
            oops,
            hidden,
        }
    }

    /// Every ordinary route, excluding all hidden roots and descendants.
    #[must_use]
    pub fn public(&self) -> &[&'a Entry] {
        &self.public
    }

    /// Every parked OOPS row, independent of its public owner project.
    #[must_use]
    pub fn oops(&self) -> &[&'a Entry] {
        &self.oops
    }

    /// Hidden rows grouped beneath their first segment, never a public route.
    pub fn hidden(&self) -> impl Iterator<Item = HiddenInventory<'_>> {
        self.hidden.iter().map(|(stem, entries)| HiddenInventory {
            stem,
            entries: entries.clone(),
        })
    }
}
