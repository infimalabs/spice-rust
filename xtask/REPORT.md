# Making work land: the frozen report study

The report reads public board history at one fixed journal position and turns
it into an offline field guide. There are two halves. The resident serves
private evidence one bounded page at a time. The xtask consumer then reduces
that evidence and publishes a sanitized edition. Neither half is a board
export. Private originals never enter the publication tree.

## Bounded historical report read

`spice dev report-page REQUEST` reads one page of retained journal facts for
private report evidence through the same authenticated resident door as every
other command. `REQUEST` is a JSON file of at most 16 KiB:

```json
{"version": 2, "upper": 797204, "kinds": ["task::added", "task::phased"]}
```

- `upper` is the inclusive study barrier. A bound beyond the resident's cursor
  refuses with `report::boundary-unavailable`.
- `kinds` must come from the report vocabulary (`wire::intent::REPORT_FACT_KINDS`);
  anything else refuses with `report::kind-outside-scope`.
- Rows are limited to tasks in public projects. Claim and lease facts are
  admitted by the task they concern, so a hidden task's leases stay hidden.
- Rows use the Canvas stream's credential-free fact form: `lane::seated`
  carries the seat's public fields but never the lane's socket capability.
  Version 1 answered with full journal envelopes and is refused.
- The first answer carries a `pin`: journal schema, the resident executable's
  digest, and the residency epoch. While an answer carries `continuation`,
  request the next page with `after` set to it and the same `pin`. A
  continuation without a pin refuses with `report::pin-required`. A changed
  decoder, schema, or residency refuses with `report::generation-changed`, so
  the acquisition restarts instead of splicing two generations together.
- `command_at` optionally attaches the retained review command record at one
  position, for a case study of a single correction flow; it refuses unless
  the record lies wholly within the barrier and a public task.

Client and resident must both carry this command. A client that has it,
talking to a resident that predates it, faults with
`command::answer-undecodable`. Bringing the resident up to date is an
operator install (`spice dev xtask -- install` from a worktree containing the
change); a lane cannot run it.

## Consumer: acquire, analyze, render

Run the three steps in order. Each step checks the digests the earlier step
recorded and refuses on any mismatch.

```sh
spice dev xtask -- report acquire UPPER PRIVATE BOARD-ORACLE
spice dev xtask -- report analyze PRIVATE
spice dev xtask -- report render PRIVATE PUBLIC FONTS LPPL
```

- `acquire` runs the installed appliance's `dev report-page` until no
  continuation remains. It writes `facts.ndjson`, the retained correction
  command record and `snapshot-manifest.json` into a new `0700` directory
  `PRIVATE`. `BOARD-ORACLE` is a `spice board --json` capture taken at the
  same cursor `UPPER`. It is copied in as an independent cross-check.
  Evidence acquired before version 2 retained lane capabilities; `analyze`
  refuses it, and it should be deleted rather than reused.
- `analyze` rebuilds every task from the facts. It selects the last 1500
  completed births and refuses unless both the selected and complete
  identity sets match the oracle exactly (`cohort-reconciliation.json`). It
  then writes the private study, phase outcomes, Git and provenance evidence,
  and the identity map. The allowlisted `publication-data.json` is written
  last, after the privacy check passes.
- `render` writes `PUBLIC`, which must not overlap `PRIVATE`. It produces
  `index.html` (the normative accessible edition), `tables.html`, SVG
  figures, CSV tables, `data.json`, a portrait A4 PDF and a hashed
  `manifest.json`. `FONTS` is the Latin Modern OpenType directory and `LPPL`
  is its license text, for example from a TeX Live installation.

`render` drives `xtask/src/report/print.mjs` with Playwright 1.62.1 and its
Chromium 151.0.7922.34, with network access disabled. Playwright resolves from
a local `node_modules` at the worktree root; this is operator-supplied tooling,
not a repository npm contract, and any other version refuses. It refuses on any
non-`file:` request, page error, duplicate id or broken anchor, unlabeled
table, missing first-focus skip link, reflow overflow at 380 px or at 200%
zoom, sheet geometry off A4, footer clearance under 2 mm, an unembedded font,
local machine paths in the PDF, or a PDF page count different from the
admitted sheets. It also needs `pdfinfo`, `pdffonts` and `pdftotext` on
`PATH`. The private receipt lands in `print-admission-private.json`.

Publication uses report-local aliases (`T0001`, `L001`, `E01`). Before
writing, it rejects original task and lane identities, Git object ids, UUIDs,
ISO timestamps carrying seconds, home paths and credential markers. Dates are
published at day or hour granularity, in UTC. The aliases reduce disclosure.
They do not guarantee protection against re-identification from outside
information.

## Visual edition

The field guide is diagram-led: a 20-figure linked contact sheet, a UTC
delivery calendar, integration-aligned exposure, conditional quantile plots,
first-verdict shares, ordered correction paths, a proof mechanism schematic,
observed provider-session spans, an origin tree, typed adjacency maps and
exact merge-parent joins. The print edition has 23 portrait sheets. The
publication data and measurement definitions are unchanged by this renderer
revision; complete alternatives remain in `tables.html`.

Figure numbers follow reading order. Colour belongs to the diagram family:
magnitude is orange, flow blue, structure violet, time magenta and topology
green. Counts, units and populations travel with each figure. Calendar width,
count width, within-group shares, canonical sequence and schematic connections
are explicitly different encodings. Observed session spans never imply
continuous activity, and integration bands never imply runtime activation.

Print admission is necessary but not a visual review. Rasterize the admitted
PDF and inspect every page for readable labels, connectors crossing text,
legends, scale, clipping, crowding and footer spacing. Exercise the contact
sheet's figure links and the figures' complete-table links with the network
disabled. Keep screenshots and the private admission receipt outside the
publication tree; the public manifest carries only the sanitized print receipt.
