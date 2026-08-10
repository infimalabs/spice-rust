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

native_shell=${SPICE_NATIVE_SHELL:?spice native shell} # env-policy: allow
unset SPICE_HOOKS SPICE_NATIVE_SHELL # env-policy: allow
SHELL=$native_shell # env-policy: allow
export SHELL
exec "$native_shell" "$@" # env-policy: allow
