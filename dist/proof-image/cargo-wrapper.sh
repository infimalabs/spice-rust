#!/bin/sh
set -eu

# The image fixes TMPDIR under the target directory and the sealed container
# contract refuses to start unless that directory is exactly /cargo-target, so
# nothing here is variable. Only the directory itself has to be made, because
# the proof boundary mounts an empty tmpfs over it.
mkdir -p /cargo-target/tmp

exec /usr/local/cargo/bin/cargo --config /opt/spice/cargo-config.toml "$@"
