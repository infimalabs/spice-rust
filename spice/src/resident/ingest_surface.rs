use super::*;

/// One durable accounting range, the wall-clock instant that observed it, and
/// every complete record the range covers in tape order.
#[derive(Clone, Debug)]
pub(super) struct TranscriptObservation {
    pub(super) batch: wire::body::JournalIngested,
    pub(super) at: Instant,
    pub(super) records: Vec<listen::Record>,
}

/// Declared value owned by one vendor, absent when the registry has none.
///
/// The map is closed and shared: a vendor that can be launched is a vendor
/// whose transcript can be read, so the launch door and the ingestion door must
/// never disagree about which spellings those are.
pub(super) fn ingest_vendor(vendor: &wire::Name) -> Option<&'static spice::extension::Vendor> {
    let registry = spice::extension::registry()
        .expect("compiled extension declarations are validated by the test gate");
    ingest_vendor_from(&registry, vendor)
}

fn ingest_vendor_from(
    registry: &spice::extension::Registry<'_>,
    vendor: &wire::Name,
) -> Option<&'static spice::extension::Vendor> {
    registry.vendor(vendor.as_str())
}

/// Recover transcript accounting, spoken interjection answers, and authored
/// MSG packets, then park peer directives whose lane a removal left behind.
///
/// Retirement and publication are replayed independently: a prior resident
/// may have committed the accounting checkpoint before it could retire the
/// captured answer or publish the captured packet.
pub(super) fn recover_transcripts(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    ingest_transcripts(
        bus,
        paths,
        None,
        lane::ingest::Bound::IMMEDIATE,
        lane::ingest::Coalescing::new(),
        diagnostic,
    )?;
    steering::dispose_at_startup(bus, paths, epoch, diagnostic)?;
    steering::publish_at_startup(bus, paths, epoch, config, diagnostic)?;
    steering::dead_letter_removed_lanes(bus, diagnostic)
}

/// Account every complete record the named lanes' tapes have gained.
///
/// Live capture passes the one lane whose record just landed; startup catch-up
/// passes nothing and sweeps them all. Both arrive here, so there is exactly
/// one accounting resume rule over the transcript checkpoint. Live display
/// may read ahead of that checkpoint, but only this guarded sweep advances it.
///
/// `bound` decides how much a stream may hold before the sweep accounts it,
/// and the returned coalescing carries whatever is still held to the next
/// sweep. It is passed by value because the writer runs the sweep on its own
/// thread, which cannot borrow the caller's state for the length of a
/// transaction.
///
/// The residency read and every accounting share one writer transaction, so a
/// lane cannot be seated into or out of the sweep midway through it.
pub(super) fn ingest_transcripts(
    bus: &Bus,
    paths: &RuntimePaths,
    only: Option<CanonicalUuid>,
    bound: lane::ingest::Bound,
    coalescing: lane::ingest::Coalescing,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<(Vec<TranscriptObservation>, lane::ingest::Coalescing)> {
    let at = Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("resident::clock-invalid", &error.to_string()))?;
    let tapes = paths.tape.clone();
    let (refusals, observations, coalescing) = bus
        .transact(move |journal| sweep(journal, &tapes, only, at, bound, coalescing))
        .map_err(|error| store_fault("resident::ingest-bus-closed", &error.to_string()))?;
    for refusal in refusals {
        let _ = writeln!(diagnostic, "{refusal}");
    }
    Ok((observations, coalescing))
}

/// Ingest each selected lane, keeping the journal range the sweep committed.
///
/// One refused lane does not end the sweep. A tape that drifted is a fact about
/// that lane alone, and stopping would leave every later lane unaccounted for a
/// reason that has nothing to do with it.
fn sweep(
    journal: &mut journal::Journal,
    tapes: &Path,
    only: Option<CanonicalUuid>,
    at: Instant,
    bound: lane::ingest::Bound,
    mut coalescing: lane::ingest::Coalescing,
) -> bus::JournalTransaction<(
    Vec<String>,
    Vec<TranscriptObservation>,
    lane::ingest::Coalescing,
)> {
    let mut refusals = Vec::new();
    let mut observations = Vec::new();
    let mut range: Option<(wire::Seq, wire::Seq)> = None;
    for row in resident_lanes(journal, &mut refusals) {
        if only.is_some_and(|lane| lane != row.lane) {
            continue;
        }
        match ingest_lane_transcript(journal, tapes, &row, at, bound, &mut coalescing) {
            Ok(Some((applied, observation))) => {
                if let Some(applied) = applied {
                    let first = range.map_or(applied.first(), |(first, _)| first);
                    range = Some((first, applied.last()));
                }
                observations.push(observation);
            }
            Ok(None) => {}
            Err(refusal) => refusals.push(refusal),
        }
    }
    let result = (refusals, observations, coalescing);
    match range {
        Some((first, last)) => bus::JournalTransaction::committed(result, first, last),
        None => bus::JournalTransaction::settled(result),
    }
}

/// Lanes this resident currently holds, empty when residency cannot be read.
fn resident_lanes(
    journal: &journal::Journal,
    refusals: &mut Vec<String>,
) -> Vec<wire::LaneResidency> {
    match journal.read(|read| read.residency_head()) {
        Ok(Some(head)) => head.into_body().lanes,
        Ok(None) => Vec::new(),
        Err(error) => {
            refusals.push(format!("resident lanes unreadable: {error}"));
            Vec::new()
        }
    }
}

/// Account one lane's transcript, naming any refusal instead of raising it.
///
/// A lane with no tape yet has nothing to account rather than something wrong:
/// a seat that has never launched an occupant is the ordinary state at startup,
/// so its absent directory is skipped silently instead of reported every sweep.
fn ingest_lane_transcript(
    journal: &mut journal::Journal,
    tapes: &Path,
    row: &wire::LaneResidency,
    at: Instant,
    bound: lane::ingest::Bound,
    coalescing: &mut lane::ingest::Coalescing,
) -> Result<Option<(Option<journal::Applied>, TranscriptObservation)>, String> {
    let Some(vendor) = ingest_vendor(&row.vendor) else {
        return Err(format!(
            "lane {} transcript not accounted: vendor {} has no decoder dialect",
            row.lane,
            row.vendor.as_str()
        ));
    };
    let root = tapes.join(row.lane.to_string()).join(row.vendor.as_str());
    if !root.exists() {
        return Ok(None);
    }
    let origin = format!("lane:{}", row.lane)
        .parse::<wire::Origin>()
        .map_err(|error| format!("lane {} origin is not addressable: {error}", row.lane))?;
    let ingestion = lane::ingest::Ingestion {
        lane: row.lane,
        vendor: row.vendor.clone(),
        stream: wire::body::TASK_CREATION_CONTEXT_STREAM.to_owned(),
        root,
        dialect: vendor.dialect(),
        actor: Actor::Spice,
        origin,
        at,
    };
    match lane::ingest::ingest_bounded(journal, ingestion.clone(), coalescing, bound) {
        Ok(lane::ingest::Ingested::Accounted {
            applied,
            batch,
            records,
        }) => Ok(Some((
            Some(applied),
            TranscriptObservation { batch, at, records },
        ))),
        // A packet cites its accounted hunk, so a pass that hands one over
        // accounts its hold at once rather than leaving the packet unpublished.
        Ok(lane::ingest::Ingested::Deferred { batch, records })
            if records.iter().any(super::steering::carries_packet) =>
        {
            let applied = lane::ingest::settle_held(journal, &ingestion, coalescing)
                .map_err(|error| format!("lane {} transcript not accounted: {error}", row.lane))?;
            Ok(Some((
                applied,
                TranscriptObservation { batch, at, records },
            )))
        }
        Ok(lane::ingest::Ingested::Deferred { batch, records }) => {
            Ok(Some((None, TranscriptObservation { batch, at, records })))
        }
        Ok(lane::ingest::Ingested::Unchanged) => Ok(None),
        Err(error) => Err(format!(
            "lane {} transcript not accounted: {error}",
            row.lane
        )),
    }
}

#[cfg(test)]
mod extension_tests {
    use super::*;

    const FIXTURE_DIALECT: vendor::Vendor = vendor::Vendor::named("fixture", &vendor::OMP);
    const FIXTURE_VENDOR: spice::extension::Vendor =
        spice::extension::Vendor::new(&FIXTURE_DIALECT, fixture_configuration, 2);
    const FIXTURE_DECLARATIONS: &[spice::extension::Declaration] =
        &[spice::extension::Declaration {
            namespace: spice::extension::Namespace::Vendor,
            name: "fixture",
            provider: "fixture",
            value: spice::extension::Value::Vendor(&FIXTURE_VENDOR),
        }];

    fn fixture_configuration(root: &spice::config::Root) -> vendor::LaunchValues<'_> {
        let configured = root.vendor().omp();
        vendor::LaunchValues {
            binary: configured.binary().as_str(),
            model: configured.model(),
            effort: configured.effort(),
            fast_mode: Some(configured.fast_mode()),
            auto_compact_window_tokens: None,
            lsp: None,
        }
    }

    #[test]
    fn a_third_declaration_reaches_live_ingest_without_another_mode_arm() {
        let registry = spice::extension::Registry::discover(FIXTURE_DECLARATIONS)
            .expect("third declaration is coherent");
        let name = "fixture".parse().expect("fixture vendor name");
        let selected = ingest_vendor_from(&registry, &name).expect("third declaration resolves");

        assert_eq!(selected.name(), "fixture");
        assert_eq!(selected.dialect(), listen::Dialect::Omp);
    }
}
