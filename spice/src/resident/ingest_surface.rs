/// The one logical stream a lane's selected capture writes.
const INGEST_STREAM: &str = "session";

/// Decoder dialect owned by one vendor, absent when the vendor has none.
///
/// The map is closed and shared: a vendor that can be launched is a vendor
/// whose transcript can be read, so the launch door and the ingestion door must
/// never disagree about which spellings those are.
fn ingest_dialect(vendor: &wire::Name) -> Option<listen::Dialect> {
    match vendor.as_str() {
        "claude" => Some(listen::Dialect::Claude),
        "codex" => Some(listen::Dialect::Codex),
        _ => None,
    }
}

/// Account every complete record the named lanes' tapes have gained.
///
/// Live capture passes the one lane whose record just landed; startup catch-up
/// passes nothing and sweeps them all. Both arrive here, so there is exactly
/// one resume rule over the transcript checkpoint. A resident that read live
/// records one way and recovered missed ones another would hold two opinions
/// about where a head stands, and the bytes between them would be accounted
/// twice or not at all.
///
/// The residency read and every accounting share one writer transaction, so a
/// lane cannot be seated into or out of the sweep midway through it.
fn ingest_transcripts(
    bus: &Bus,
    paths: &RuntimePaths,
    only: Option<CanonicalUuid>,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let at = Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("resident::clock-invalid", &error.to_string()))?;
    let tapes = paths.tape.clone();
    let refusals = bus
        .transact(move |journal| sweep(journal, &tapes, only, at))
        .map_err(|error| store_fault("resident::ingest-bus-closed", &error.to_string()))?;
    for refusal in refusals {
        let _ = writeln!(diagnostic, "{refusal}");
    }
    Ok(())
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
) -> bus::JournalTransaction<Vec<String>> {
    let mut refusals = Vec::new();
    let mut range: Option<(wire::Seq, wire::Seq)> = None;
    for row in resident_lanes(journal, &mut refusals) {
        if only.is_some_and(|lane| lane != row.lane) {
            continue;
        }
        match ingest_lane_transcript(journal, tapes, &row, at) {
            Ok(Some(applied)) => {
                let first = range.map_or(applied.first(), |(first, _)| first);
                range = Some((first, applied.last()));
            }
            Ok(None) => {}
            Err(refusal) => refusals.push(refusal),
        }
    }
    match range {
        Some((first, last)) => bus::JournalTransaction::committed(refusals, first, last),
        None => bus::JournalTransaction::settled(refusals),
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
) -> Result<Option<journal::Applied>, String> {
    let Some(dialect) = ingest_dialect(&row.vendor) else {
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
        stream: INGEST_STREAM.to_owned(),
        root,
        dialect,
        actor: Actor::Spice,
        origin,
        at,
    };
    match lane::ingest::ingest(journal, ingestion) {
        Ok(lane::ingest::Ingested::Accounted { applied, .. }) => Ok(Some(applied)),
        Ok(lane::ingest::Ingested::Unchanged) => Ok(None),
        Err(error) => Err(format!(
            "lane {} transcript not accounted: {error}",
            row.lane
        )),
    }
}
