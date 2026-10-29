# Mosaic card lineage

[Spice implementation profile](implementation.md)

The Mosaic shows prose, failure, sign-out, compaction, fresh-session, and
image-bearing tool activity as cards. Turn starts, reasoning, and imageless tool
calls remain lifecycle evidence: they take no card slot and render no
announcement. History paging follows the source frontier rather than card
eligibility, so a page holding only non-card records still leads to older prose.

A fresh vendor session renders a "started fresh session" divider shaped like a
compaction boundary. The divider is a transcript card anchored at the lane
tape's session-state record that reports no messages yet, so live delivery,
resident restarts, browser reloads, and history pages all place it at that same
record exactly once. An exact-session resume reports its earlier messages and
renders none, and a state record that reports no message count infers none.

A successful compaction names the strategy reported by its completion:
`snapcompact` renders as snapcompacted, while another method precedes
“compacted context” by name. The method reads quieter than the label it
prefixes: regular weight in the console's muted base-flavor neutral, never an
agent, lane, or Design accent. A completion with no method keeps the plain
“compacted context” label. Boundary reconciliation carries a live
completion's method onto the timestamped preserved record, so paging and
reconnect render the same label.

Older history pages when a placed member frontier is near the viewport, the
reader reaches the plane's trailing edge, or a member lacks composer prose.
Missing prose can demand successive offscreen pages, and it pages a plane that
holds no card yet, such as a team whose lanes have not run since the resident
started, from each member's newest history; a card still measuring does not
itself establish viewport demand. Each frontier is requested once
unless a fresh viewport gesture follows a completed page. An unanswered page
is requested again after reconnect, even if its frontier has moved offscreen.

Compaction and lifecycle dividers stay full-width boundaries between
chronological epochs. A replayed epoch rests against its retained boundary,
carrying all older history with it; where incremental placement cannot keep
every boundary whole, the plane replays in full. A paged plane therefore matches
a fresh load of the same cards however its pages were batched, and a card that
retention evicts leaves no gap.
