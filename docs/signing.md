# Signing & verification

A signed pack proves it came from your repository — not just that it is internally consistent.
Signing is **keyless**: it uses a CI (or personal) OIDC identity through cosign and Sigstore, so
there is no key to generate, store, or rotate.

## Signing

`lode publish --sign` bundles the pack, signs it, and uploads the archive, its `.sha256`, and a
`.sigstore` bundle to the release together. It shells out to cosign, which must be on `PATH`.

Signing is meant to run in CI, where the signature binds to the repository's workflow identity. The
only requirement beyond an ordinary publish job is the `id-token: write` permission that lets the
job mint a keyless certificate:

```yaml
permissions:
  contents: write   # create the release, upload assets
  id-token: write   # keyless signing identity
```

Run locally, `lode publish --sign --tag v1.0.0` works too — cosign opens a browser for a one-time
OIDC login and the signature binds to your personal identity instead of the repo's.

## Verifying

```sh
lode get <host/owner/repo>@<tag> --verify
```

`--verify` refuses to unpack or install unless the release carries a valid `.sigstore` signature
from the ref's own repo. Verification is **native** — the consumer needs no cosign — because lode
vendors Sigstore's trusted-root material in `src/assets/`, kept current by `refresh-trusted-root.yml`.
