# Release branches and backports

`main` is the development line for the next release. A branch named
`release/vMAJOR.MINOR.x` is the maintenance line for a shipped release series,
such as `release/v5.0.x` for the `v5.0` releases. The maintenance line exists so
a fix can ship against an already-released version without also shipping
whatever else has landed on `main` since. The historical `release/v3.x` branch
predates this naming pattern and keeps its original name.

The currently supported maintenance lines are:

- `release/v3.x`, selected by `A:backport/v3.x`;
- `release/v4.0.x`, selected by `A:backport/v4.0.x`; and
- `release/v5.0.x`, selected by `A:backport/v5.0.x`.

Cut a maintenance branch from the chosen release commit before the first release
candidate on that line. Every release tag for the line should be reachable from
its branch: `v5.0.0-rc.1` and `v5.0.0` both belong on `release/v5.0.x`. Nothing
enforces this in CI, so check it before tagging.

Tags in this repository take two forms. `zcash_voting` releases are tagged
`v<version>`, and the other published crates are tagged `<package>-v<version>`,
such as `vote-commitment-tree-v0.6.0`. Publishing itself is manual and is
described in `.agents/skills/release-librustvoting/SKILL.md`; this document
covers only which branch a change lands on.

## What belongs on a maintenance line

A maintenance line ships bug fixes and other semver-compatible changes to
consumers already pinned to that major version. Before requesting a backport,
confirm the change does not:

- remove, rename, or narrow a public API, or change the meaning of one;
- add a public API that a later release on `main` would define differently;
- add a feature, rather than correct a defect;
- raise the MSRV, or take a major-version bump of a public dependency; or
- change a serialized or on-disk representation that an existing release reads,
  including the SQLite round-state schema and any wire representation.

Changes to tests, CI, documentation, and developer tooling are normally safe.
When a fix is genuinely needed but cannot be made compatibly, it belongs in the
next release from `main`, not on the maintenance line.

## Backport flow

Changes merge to `main` first. Apply each `A:backport/*` label whose maintenance
line should receive the change. A source PR may target more than one supported
line. After the source PR merges, Mergify opens a separate PR against each
selected release branch and assigns them to the source author.

That backport PR is an ordinary PR. It runs the normal CI suite and needs human
review; it is never merged automatically. If the cherry-pick conflicts, Mergify
opens the PR anyway and applies `A:backport/conflict` — resolve the conflict on
the generated PR rather than pushing directly to the maintenance branch.

Release-only metadata, such as a version bump for the maintenance line, may
target that release branch directly. An emergency fix made directly on a
maintenance branch must be forwarded to `main` immediately afterward, or the
next release will silently regress it.

Applying a backport label never creates a tag, publishes a crate, or releases
anything. Releases remain explicit tags cut by a human.

## Opening a maintenance line

Prepare a maintenance line in this order so Mergify never targets a branch whose
policy is absent from `main`:

1. Open a PR against `main` that updates this document, contributor guidance,
   the pull request template, and `.github/mergify.yml` for the new line.
2. Merge that PR, update the local `main` from `origin/main`, and record the
   resulting commit.
3. Create `release/vMAJOR.MINOR.x` at that exact commit and push the branch.
4. Create its `A:backport/vMAJOR.MINOR.x` label with a description that names
   the target branch.
5. Verify that the branch and label exist on GitHub and that the Mergify rule on
   `main` names both exactly.

Do not apply the new backport label until the target branch exists. The branch
cut commit must contain the matching Mergify rule, so future maintenance work
and the source branch agree on the backport policy.

## Retiring a line

When a maintenance line reaches end of life, remove its rule from
`.github/mergify.yml` and delete its `A:backport/*` label after confirming that
no open PR still uses it. Keep only the currently supported release lines
active. Historical release branches remain available; retiring a line does not
delete its branch or tags.

## Requirements

The backport automation runs through the Mergify GitHub App, which must be
enabled for this repository by a `valargroup` organization administrator.
Until it is, `.github/mergify.yml` is inert: applying the label will have no
effect and backports must be cherry-picked by hand.
