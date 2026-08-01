#!/bin/sh
# The static interception stage, sourced rather than executed.
#
# Descendant shells reach this stage with activation already proven, so it never
# reexecs and never bootstraps the resident boundary a second time. It writes
# the lane's rendered direction to stderr and returns, which is the whole of its
# work.
#
# A rendering that is absent, empty, or stale is ordinary: disposition lives in
# the journal and this file is only its rendering. Entering without proof is
# not ordinary, and says so.

if [ -z "${SPICE_PROOF-}" ]; then
	printf 'spice: static stage entered without activation proof\n' >&2
	return 1
fi

if [ -r "${SPICE_INBOX-}" ]; then
	cat -- "${SPICE_INBOX}" >&2 # env-policy: allow
fi
