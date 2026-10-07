# Task overview

## Graph-first task overview

Task activation opens one contextual overview for the complete selected task
set. The selection is an ordered, stable-deduplicated list: every selected
root remains visible even when the set is larger than the relation command's
former root or node limits. Scope and project entry points derive that list
from the locally admitted board in its offered order.

The overview is one continuous modal surface at the task sheet's reading width:
46rem, 42rem at or below 1180px, and the full window at or below 560px. Its
admitted-board graph renders first, including every selected root, roots with
no edges, and explicit outside-board or outside-selection neighbors. The same
graph offers focused Dependency, Lineage, Reviews, and Lifecycle lenses.
Dependency direction and typed depth/node controls bound optional resident
enrichment without bounding the local selection; loading, refreshing,
unavailable, retry, and explicit truncation/frontier states never replace or
silently omit selected roots.
Lineage preserves exact task, ACK, and origin identities. Review chains retain
phase author → reviewer → verdict order. Lifecycle facts retain their durable
source order. Zoom, pan, reset, and task-node activation remain available
without changing the selection. At 100% zoom a diagram draws at no more than
its natural size, shrinking only to fit, and the graph frame shrinks to the
diagram's natural height within its 18–28rem viewport-relative ceiling; zoom
alone enlarges it. Each node's state badge straddles its top border so it never
covers the node's label.

Every selected task then owns a full detail section below the graph. Current
ownership and claim, task state and contract, project/team location, timestamps
and lifecycle, origin and dependencies answer who, what, where, when, and why
from live operational state. Evidence, blockers, relationships, provenance,
activity, notes, loading, unavailable, and retry states appear simultaneously
rather than behind tabs, pagination, or next/previous controls. Activating a
selected graph node scrolls and moves focus to that task's detail section;
activating a related external task follows that lead in the same overview.
Escape, backdrop, and Close dismiss the overview and restore the prior focus
when it still exists. Reduced-motion mode removes the sheet and graph
transitions.
