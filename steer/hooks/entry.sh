#!/bin/sh
# The dynamic interception stage.
#
# The first shell an agent-bound worktree starts arrives here and reexecs itself
# through spice, so spice owns stderr steering before the requested command
# runs. That happens exactly once: the reexec exports the activation proof, and
# every descendant shell sees it and sources the static stage instead.
#
# Nothing here opens authority or discovers a repository. Geometry arrives in
# the environment the installer wrote, and direction arrives as a file that was
# rendered ahead of time.

if [ -n "${SPICE_PROOF-}" ]; then
	. "${SPICE_HOOKS:?spice hooks directory}/static/stage.sh"
else
	SPICE_PROOF="${SPICE_LANE:?spice lane}"
	export SPICE_PROOF
	exec spice run -- "$@"
fi
