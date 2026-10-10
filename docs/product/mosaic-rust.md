# Mosaic card lineage

[Spice Rust implementation profile](rust.md)

The Mosaic shows prose, failure, sign-out, compaction, and image-bearing tool
activity as cards. Turn starts, reasoning, and imageless tool calls remain
lifecycle evidence: they take no card slot and render no announcement. History
paging follows the source frontier rather than card eligibility, so a page
holding only non-card records still leads to older prose.

A seating that starts a fresh vendor session renders a "started fresh session"
divider shaped like a compaction boundary. An exact-session resume renders none,
and a seating that recorded no session disposition infers none.

Older history pages when a placed member frontier is near the viewport, the
reader reaches the plane's trailing edge, or a member lacks composer prose.
Missing prose can demand successive offscreen pages; a card still measuring
does not itself establish viewport demand. Each frontier is requested once
unless a fresh viewport gesture follows a completed page. An unanswered page
is requested again after reconnect, even if its frontier has moved offscreen.

Compaction and lifecycle dividers stay full-width boundaries between
chronological epochs. A replayed epoch rests against its retained boundary,
carrying all older history with it; where incremental placement cannot keep
every boundary whole, the plane replays in full. A paged plane therefore matches
a fresh load of the same cards however its pages were batched, and a card that
retention evicts leaves no gap.
