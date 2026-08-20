#!/bin/sh
# The outer interception stage.
#
# The agent's shell tool reaches this executable once with the complete shell
# argument vector. It always reexecs through spice and into the separately
# packaged inner stage. No marker decides whether interception happens.
#
# Nothing here opens authority or discovers a repository. Geometry arrives in
# the environment the installer wrote, and direction arrives as a file that was
# rendered ahead of time.

exec "${SPICE_RS_WRAPPER_SPICE:?rust spice executable}" run -- \
  "${SPICE_RS_HOOKS:?rust spice hooks directory}/static/stage.sh" "$@"
