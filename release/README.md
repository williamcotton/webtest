# WebTest release artifacts

Tier 1: macOS Intel/Apple Silicon; Linux x86_64/arm64 (Ubuntu 22.04,
glibc 2.35 or newer); Windows x86_64. Chrome is installed separately with
`webtest browser install`. `webtest --version --json` reports the exact product,
build target/commit, Chrome pin and supported protocol formats.

Archives contain the executable, license inventory, CycloneDX SBOM, Cargo lock,
toolchain identity, Chrome metadata, and compatibility manifest. Raw binaries
are published for installers, which never extract archives. SHA256SUMS is a
convenience index; the authoritative signed manifest includes hashes and sizes.

Verify `manifest.json` against `manifest.sigstore.json` using Sigstore/cosign,
with OIDC issuer `https://token.actions.githubusercontent.com` and exact identity
`https://github.com/williamcotton/webtest/.github/workflows/release.yml@refs/tags/vVERSION`.
Then verify the selected artifact against the signed manifest. GitHub artifact
attestations separately bind artifacts and SBOMs to the workflow commit.

Shell/PowerShell installers require cosign and an exact version; they install
only the platform binary under the supplied prefix, without editing shell startup
files or installing Chrome. Install the extension as a universal VSIX and run
**WebTest: Install/Update Server** to explicitly download a verified server.
That flow keeps previous immutable installations for rollback and requires reload.

Build release candidates using locked dependencies and the recorded toolchain.
Release publishing requires a protected GitHub `release` environment and a trusted
version tag. Marketplace, npm and GHCR access must be configured by maintainers.
A locally passing test suite is not evidence that those external releases exist.

The pinned Linux container uses UID/GID 10001, `/workspace` for mounted sources,
`/artifacts` for host-selected outputs, and `/home/webtest/.cache` for managed Chrome.
It exposes no port and keeps Chrome sandboxing enabled. Hosts must permit its
sandbox user namespaces; the release smoke explicitly grants SYS_ADMIN to its CI
container. Mount writable volumes with UID 10001 ownership. Release container tags
are `ghcr.io/williamcotton/webtest:VERSION`; no reproducibility example uses latest.

`package_managers.py` projects the signed manifest into reviewable Homebrew and
WinGet definitions. Repository submission is separate from artifact generation.
Sigstore verification follows https://docs.sigstore.dev/language_clients/javascript/;
WinGet projections follow https://learn.microsoft.com/windows/package-manager/package/manifest.
