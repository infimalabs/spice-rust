# The frozen report study

The report reads filed task history at one fixed journal position and
publishes a sanitized offline field guide. The resident serves private evidence
one bounded page at a time; the xtask consumer reduces it and publishes. Neither
half is a board export, and private originals never enter the publication tree.

## Bounded historical read

`spice dev report-page REQUEST` reads one page of retained journal facts through
the authenticated resident door. `REQUEST` is a small JSON file:

```json
{"version": 2, "upper": 797204, "kinds": ["task::added", "task::phased"]}
```

- `upper` is the inclusive study barrier; one beyond the resident's cursor
  refuses with `report::boundary-unavailable`.
- `kinds` must come from `wire::intent::REPORT_FACT_KINDS`, else
  `report::kind-outside-scope`.
- Rows are limited to filed tasks, with claim and lease facts admitted by the
  task they concern; unfiled Drafts are excluded. Rows use the app stream's
  credential-free fact form, so no lane capability is exposed.
- The first answer carries a `pin` (journal schema, resident executable digest,
  residency epoch). Continue with `after` set to the answer's `continuation`
  and the same `pin`; a missing pin refuses with `report::pin-required`, and a
  changed decoder, schema, or residency refuses with
  `report::generation-changed`, so acquisition restarts rather than splicing
  generations.
- `command_at` optionally attaches the review command record at one position
  wholly within the barrier and a public task.

## Acquire, analyze, render

```sh
spice dev xtask -- report acquire UPPER PRIVATE BOARD-ORACLE
spice dev xtask -- report analyze PRIVATE
spice dev xtask -- report render PRIVATE PUBLIC FONTS LPPL
```

Each step verifies the digests the previous step recorded.

- `acquire` pages `dev report-page` to the end into a new `0700` directory
  `PRIVATE`. `BOARD-ORACLE` is a `spice board --output FILE` export taken at the
  same cursor, kept as an independent cross-check.
- `analyze` rebuilds every task from the facts and refuses unless its selected
  and complete identity sets match the oracle exactly. It writes the private
  study and, only after the privacy check passes, the allowlisted publication
  data.
  Both census consumers read the current board shape without a project-visibility
  field; the frozen cursor and exact selected/completed identity checks still
  apply.
- `render` writes `PUBLIC`, which must not overlap `PRIVATE`: an accessible
  HTML edition, complete tables, SVG figures, CSV, JSON, an A4 PDF, and a
  hashed manifest. `FONTS` is the Latin Modern OpenType directory and `LPPL` its
  license. It drives `xtask/src/report/print.mjs` through operator-supplied
  Playwright at the exact version the renderer admits, with the network
  disabled, and needs `pdfinfo`, `pdffonts`, and `pdftotext`. It refuses on any
  non-`file:` request, page error, broken anchor, unlabeled table, reflow
  overflow, off-A4 geometry, unembedded font, local path in the PDF, or sheet
  count mismatch.

Publication uses report-local aliases and rejects original task and lane
identities, Git object ids, UUIDs, second-precision timestamps, home paths, and
credential markers; dates are published at day or hour granularity in UTC.
Aliases reduce disclosure but do not prevent re-identification from outside
information.

Report categories use an explicit allowlist of complete project labels. A dot
does not confer a parent category: `app` can have an allowlisted category while
`app.private` falls into the generic redacted category. Raw project names are
never copied into the public task rows, relationship nodes, or domain totals.

Print admission is not a visual review: rasterize the PDF and inspect every
page, and exercise the figure and table links with the network disabled. Keep
screenshots and the private admission receipt outside the publication tree.
