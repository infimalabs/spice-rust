# Invariant register — Distribution and conduct

[Invariant register](../invariants.md) · [Spice product model](../README.md)

## Distribution

The contracts live at [publication authorization](../planes/distribution.md#external-publication-is-a-plan-not-a-verb),
[versioned source](../planes/distribution.md#a-versioned-commit-is-sealed-before-publication),
[target proof](../planes/distribution.md#proof-is-sealed-per-target-and-composed-last),
[release notes](../planes/distribution.md#notes-are-read-out-of-the-ledger),
[publication convergence](../planes/distribution.md#the-publication-sequence),
[installation](../planes/distribution.md#installing-into-a-repository-the-product-does-not-own),
[identity-aware reversal](../planes/distribution.md#reversal-is-a-claim-about-identity),
and [receipt authority](../planes/distribution.md#the-canonical-receipt).

| Label | Invariant |
| --- | --- |
| <a id="distribution-the-install-digest-binds-private-observations-that-the-preview-does-not-disclose"></a>**Install approval does not disclose private observations** | The digest binds exact prior scoped and effective configuration values. Public preview exposes byte-free activation metadata, not those values; independent Git-scope inspection is the disclosure path. |
| <a id="distribution-an-approval-of-executable-repository-configuration-binds-an-exact-snapshot-of-it"></a>**Executable configuration approval is snapshot-bound** | Approval names the exact executable tracked and local configuration bytes. A changed configuration surface requires its own approval; approval does not authorize a moving path. |

## Conduct

See [cross-cutting principles](../planes/principles.md#cross-cutting-principles),
[repair-first refusals](../model/decisions.md#refusal-grammar-repair-first),
[failure and degradation](../model/failures.md#failure-and-degradation),
and [independently usable capability tiers](../model/boundaries.md#capability-tiers).
