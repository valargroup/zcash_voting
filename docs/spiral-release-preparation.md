# Spiral compatibility release preparation

Proposed release: `zcash_voting 5.1.1-rc.1`.

The exact Spiral requirement changes from `0.5.2` to `0.5.3-rc.1`, and
YPIR changes from `0.2.0` to `0.2.1-rc.1`. This matches the published
Enhance PIR dependency graph without downstream local manifest overrides.
The voting source, public API, wire format, and database schema are unchanged.

## Publication prerequisite

Publish `valar-ypir 0.2.1-rc.1` first. Until it exists on crates.io, Cargo
cannot resolve this preparation and `Cargo.lock` remains at the previous
release. Do not merge or publish this draft in that state.

After YPIR publication:

1. Run `cargo update -p valar-ypir --precise 0.2.1-rc.1` and commit the
   resulting lockfile, including the new voting package version.
2. Run `make check`, `make test`, and `make msrv`.
3. Verify the registry dependency readiness check and the full PR CI.
4. Follow `docs/release-branches.md` and the release skill to prepare the
   maintenance-line release commit and publish only with release approval.
5. Update the Vizor voting pin and remove its two local manifest overrides.

No crate publication or tag creation is part of this preparation PR.
