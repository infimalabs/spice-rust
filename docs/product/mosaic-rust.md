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

Older history pages only on reader demand: a member's placed frontier card
within reach of the viewport, or the plane's trailing edge. A card still
measuring is not demand, so arriving pages never chain into autonomous paging.
A page that a dropped socket never answered is requested again after reconnect,
so an unscrolled cold load retains the same history whatever the socket timing.

Compaction and lifecycle dividers stay full-width boundaries between
chronological epochs. A replayed epoch rests against its retained boundary,
carrying all older history with it; where incremental placement cannot keep
every boundary whole, the plane replays in full. A paged plane therefore matches
a fresh load of the same cards however its pages were batched, and a card that
retention evicts leaves no gap.
