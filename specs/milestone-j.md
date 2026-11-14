# Milestone J — Release Acceptance and Publication

## Status and dependencies

Proposed. This milestone owns the remaining external release work deferred from
[Milestone G](./milestone-g.md) on 2026-11-13. It consumes the implemented release
pipeline, installers, compatibility manifests, portable editor package, VSIX,
and container definitions. Milestones H and I do not depend on publication in J;
J must test their final protocol/plan/schema versions rather than G-era constants.

## Scope

- Execute the release workflow on all five tier-1 targets and install each signed
  candidate in clean environments. The release workflow and platform runners have
  not been exercised from this local change.
- Verify the real signed-manifest install/upgrade path and end-to-end rollback on
  every supported extension host. Local verification currently covers rejection,
  integrity, package activation, LSP and DAP; it does not substitute for a signed
  release rehearsal. Previous binaries can be selected with `webtest.serverPath`.
- Build/run/publish the official Linux container, including host sandbox policy and
  mounted artifact ownership. Its release smoke retains Chrome sandboxing and
  explicitly grants SYS_ADMIN in CI; no default `--no-sandbox` is introduced.
- Configure protected release/tag policy and npm/marketplace credentials/trusted
  publishing; publish npm/VSIX artifacts and submit Homebrew/WinGet projections.
  `release/publish-packages.yml.example` documents the optional protected steps.
- Complete signature/provenance/SBOM and installer/package-manager validation on
  clean target systems. Shell/PowerShell installers require cosign. Package-manager
  projections are generated for review and are not claims of registry acceptance.

## Acceptance

1. All five tier-1 native artifacts build, install and execute in clean target environments.
2. Signed manifests, checksums, source provenance and SBOMs are verified, including rejection of tampered/unsupported artifacts.
3. Compatible signed server installation, upgrade and rollback work in clean supported extension hosts.
4. The pinned non-root container runs real Chrome tests with documented sandbox and artifact-volume ownership.
5. Protected release/tag policy, npm/marketplace trusted publishing and credentials are configured; native, npm, VSIX and container releases are published.
6. Homebrew/WinGet projections are submitted and validated in their supported package-manager environments.
7. Release evidence records exact product/commit/platform/Chrome/protocol versions and all required quality-gate results.

Completion requires executed evidence and published artifacts, not only workflow YAML.
