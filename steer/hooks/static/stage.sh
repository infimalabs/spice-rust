#!/bin/sh
# The inner shell stage, executed exactly once by the wrapper.
#
# This stage removes the packaged routing environment, restores the user's
# native shell, and replaces itself with that shell. The exec preserves the
# immediate child's process identity while every process it starts inherits the
# native shell rather than either Spice stage.
#
# Sourcing remains an operation inside that native command shell and therefore
# shares its functions and options without crossing another process boundary.

# These handoff reads are the complete environment contract this stage consumes.
native_shell=${SPICE_NATIVE_SHELL:?spice native shell} # env-policy: allow
unset SPICE_HOOKS SPICE_NATIVE_SHELL # env-policy: allow
# `native_shell` is a shell-local handoff after the packaged environment is
# removed; these scoped markers do not admit another inherited environment key.
SHELL=$native_shell # env-policy: allow
export SHELL
exec "$native_shell" "$@" # env-policy: allow
