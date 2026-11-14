# Security and release trust

Report vulnerabilities privately through the repository's GitHub security advisory
page. Do not include credentials or private source in public issues. No telemetry
or source upload is enabled by the editor or extension.

Release trust is rooted in the exact `williamcotton/webtest` GitHub release
workflow identity at an immutable version tag and the GitHub OIDC issuer. Require
Sigstore transparency verification, then verify every binary's SHA-256 and byte
length against the signed manifest. Unsigned checksum files alone do not establish
a trusted publisher. Build provenance and SBOMs are included with releases.

The release workflow only publishes from tags after its quality, platform,
archive, editor and container gates; maintainers must protect version tags and the
`release` environment. npm/marketplace credentials are configured only in protected
publication jobs. Never provide these credentials to pull request workflows.

Managed installation downloads raw binaries, so downloaded archives have no
extraction authority. Installation never silently installs Chrome or modifies
shell startup files. Unsupported platforms must build from source. Package-manager
manifests are generated for review; publishing to Homebrew/WinGet is a separate
maintainer action. Container examples use immutable product tags and an unprivileged
UID. Sandboxed Chrome requires host support for user namespaces; the release smoke
uses SYS_ADMIN in the CI container while retaining Chrome's sandbox. Evaluate the
host's sandbox policy when configuring a CI runner.

Browser WASM has only explicit source/configuration/schema inputs and the supplied
WASM asset URL. It cannot execute test plans, launch native providers, or access a
live browser through the portable API. Worker fatal errors require recreation from
host-owned inputs. Diagnostics, plans and traces retain their shared Rust schemas;
adapters reject unsupported major formats rather than guessing compatibility.
