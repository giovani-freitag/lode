# lode — command reference

The complete list of commands and flags. For a quick start see the
[README](../README.md).

Verbs follow npm: `add`/`del` manage what's **declared** in the manifest; `install`
(`i`) materializes an instance from the **lockfile**. Everything is driven by two files
in the pack root — `lode.jsonc` (the manifest you edit) and `lode.lock` (generated).

| Command | Aliases | What it does |
|---------|---------|--------------|
| [`init`](#lode-init) | | Scaffold a new pack in the current directory |
| [`import`](#lode-import) | | Convert an existing packwiz pack into a lode project |
| [`add`](#lode-add) | | Declare a mod, resolve deps, download the jars |
| [`del`](#lode-del) | `remove`, `rm` | Remove a mod, prune orphaned deps, delete the jars |
| [`install`](#lode-install) | `i` | Download everything in the lockfile into an instance |
| [`get`](#lode-get) | | Fetch a published pack from a ref and set it up |
| [`update`](#lode-update) | | Re-resolve to the latest allowed versions |
| [`pin`](#lode-pin--lode-unpin) | | Freeze a mod at its locked version |
| [`unpin`](#lode-pin--lode-unpin) | | Let `update` bump a mod again |
| [`list`](#lode-list) | | Show the resolved pack |
| [`why`](#lode-why) | | Explain why a mod is in the pack |
| [`refresh`](#lode-refresh) | | Re-resolve the lockfile from the manifest |
| [`export`](#lode-export) | | Export the pack to packwiz format (interop bridge) |
| [`bundle`](#lode-bundle) | | Pack the definition into a distributable `.tar.gz` + checksum |
| [`publish`](#lode-publish) | | Bundle and publish the pack as a GitHub release |
| [`verify`](#lode-verify) | | Check installed jars against the lockfile (integrity) |
| [`config`](#lode-config) | | Get/set stored settings (e.g. the CurseForge key) |

---

## `lode init`

Scaffold a new pack (`lode.jsonc`) in the current directory. Interactive by default —
pick the loader, then choose from the **live** list of Minecraft and loader versions.
Fully scriptable with flags (any omitted version resolves to the latest).

```sh
lode init
lode init -y --loader fabric --minecraft 1.20.1 --name "My Pack" --author me
```

| Flag | Description |
|------|-------------|
| `--name <name>` | Pack name. |
| `--author <author>` | Pack author. |
| `--version <v>` | Pack version (default `0.1.0`). |
| `--minecraft <v>` | Minecraft version (default: latest). |
| `--loader <loader>` | `forge` \| `neoforge` \| `fabric` \| `quilt`. |
| `--loader-version <v>` | Loader version (default: latest for the chosen loader). |
| `-y`, `--yes` | Accept defaults without prompting. |

## `lode import`

```sh
lode import <source> [<dir>] [--out <dir>] [--force]
```

Convert an existing pack from another tool into this lode project — writing `lode.jsonc` +
`lode.lock`, and **leaving the source untouched**. This is the on-ramp for packs already
maintained with packwiz.

The source keyword is mandatory (it names what's being consumed, so the direction is always
*into* lode — the opposite of `export`). Today the only source is `packwiz`; `<dir>` is the
packwiz pack directory (or its `pack.toml`) and defaults to the current directory.

```sh
lode import packwiz .                       # adopt the packwiz pack here → lode.jsonc + lode.lock
lode import packwiz ./oldpack --out ./newpack
```

| Flag | Description |
|------|-------------|
| `--out <dir>` | Write the lode project into this directory instead of the current one. |
| `--force` | Overwrite an existing `lode.jsonc` (otherwise import refuses). |

Notes on the packwiz conversion:

- The `.pw.toml` files already carry the resolved file (URL + hash + ids), so jars are **not**
  re-picked — the pins are preserved exactly.
- Modrinth mods' human version number and dependency edges aren't in the metafile, so they're
  recovered from the Modrinth API (keyless). CurseForge files keep their filename as the
  version and need no key at import time (installing them later still needs `CF_API_KEY`).
- Every imported mod is recorded as directly declared (packwiz keeps no provenance). Overlay
  files (configs/scripts) are left in place — declare them under `overlays` if you want lode
  to ship them in `export`.

## `lode add`

```sh
lode add <slug | id | url | search>
```

Declare a mod, resolve its transitive dependencies, and **download the jars** — like
`npm install <pkg>`. The query accepts a Modrinth/CurseForge slug, a project id, a
project URL, or free-text (prompts you to pick from the search hits).

```sh
lode add create
lode add "just enough items"          # free-text search, pick from a list
lode add jei --cf --version 15.2.0.27 # exact CurseForge version
lode add sodium -y --lock-only        # no prompt, don't download yet
```

| Flag | Description |
|------|-------------|
| `--version <v>` | Pin an exact version instead of the latest. |
| `--side <side>` | Override the environment: `client` \| `server` \| `both` \| `none`. |
| `--curseforge`, `--cf` | Resolve from CurseForge instead of Modrinth (needs an API key). |
| `--lock-only` | Update the manifest + lockfile without downloading the jars. |
| `-y`, `--yes` | Take the first search hit without prompting. |

## `lode del`

```sh
lode del <slug>          # aliases: remove, rm
```

Remove a mod from the manifest, prune any dependency it alone pulled in, and **delete
the jars** from the instance — like `npm uninstall`.

## `lode install`

```sh
lode install             # alias: i
```

Download everything in the lockfile into an instance — like running `npm install` after
`git clone`. Each file is verified against its locked hash, and no separate installer or
launcher is required.

| Flag | Description |
|------|-------------|
| `--server` | Provision the loader server first, then install the server-side mods. |
| `--into <dir>` | Target a specific instance directory (default: the pack root). |
| `--skip-loader` | Assume the loader is already installed; skip provisioning. |
| `--java <path>` | Java executable to run the loader installer with (default: `java`). |

## `lode get`

```sh
lode get <host/owner/repo[@tag]>
lode get --from-url <url>
lode get --from-file <pack.tar.gz>
```

Fetch a **published** pack and set it up here — the consumer side of `publish`. Give exactly one
source: a release reference **with an explicit host** (`github.com/owner/repo`, optionally `@tag` —
defaults to the latest release; a full `https://…` URL works too), a direct `.tar.gz` URL, or a
local `.tar.gz`. The host is required — there's no default platform and no guessing between hosts.
(Only `github.com` is supported today; GitLab / self-hosted are planned.)

For a repo ref, `get` downloads the release's `.tar.gz` and its `.sha256` sibling and **verifies
the archive against that published checksum** (the trust anchor). It then unpacks the thin archive
(`lode.jsonc` + `lode.lock` + overlays) and runs the install — fetching the jars from their
providers, each verified against its lock hash. A **private** repo needs `GITHUB_TOKEN` set (it
authenticates both the release lookup and the asset download).

`--from-url`/`--from-file` have no trusted checksum channel, so integrity is reported as
unconfirmed (the mods are still verified against the lock on install).

**`--verify`** additionally requires a valid signature (a `.sigstore` bundle on the release) from
the ref's repo before it will unpack or install — so you know the pack came from that repo, not just
that it's internally consistent. See [signing.md](signing.md) to sign your releases.

| Flag | Description |
|------|-------------|
| `--from-url <url>` | Fetch a `.tar.gz` from a direct URL instead of a ref. |
| `--from-file <path>` | Use a local `.tar.gz` instead of fetching. |
| `--into <dir>` | Directory to unpack into (default: a folder named after the repo/archive). |
| `--verify` | Require a valid Sigstore attestation from the ref's repo (verification is native). |
| `--no-install` | Unpack and verify only; don't download the jars. |

## `lode update`

```sh
lode update [slug]
```

Re-resolve to the latest allowed versions and report what changed. Pinned mods are never
bumped (see `pin`).

| Flag | Description |
|------|-------------|
| `--all` | Update the whole pack (equivalent to omitting the slug). |

## `lode pin` · `lode unpin`

```sh
lode pin <slug>
lode unpin <slug>
```

`pin` freezes a mod at its currently-locked version so `update` won't bump it; `unpin`
removes that freeze. Useful when a newer version is known-broken, or to hold a mod steady
while updating the rest of the pack.

## `lode list`

```sh
lode list
```

Show the resolved pack — direct mods vs. dependencies, side, and provider.

| Flag | Description |
|------|-------------|
| `--json` | Emit the resolved lockfile as JSON for machine consumption. |

## `lode why`

```sh
lode why <slug>
```

Explain why a mod is in the pack: declared directly, or pulled in by which other mods
(from the lockfile's provenance).

## `lode refresh`

```sh
lode refresh
```

Re-resolve the lockfile from the manifest, creating it if missing (self-healing). Unlike
`update`, it respects the locked versions — it only fills gaps and repairs a stale lock.

## `lode export`

```sh
lode export packwiz
```

Export the pack to another tool's format — the mirror of `import`. Today the only target is
`packwiz`: it writes a distribution tree into `pack/` for launchers that consume packwiz (MultiMC/
Prism via packwiz-installer); serve that directory over HTTP. This is an **interop bridge** — lode's
own `install`/`get` need none of it.

## `lode bundle`

```sh
lode bundle [--out <dir>]
```

Pack the **definition** of the pack into a distributable, deterministic `<name>-<version>.tar.gz`
plus a `.sha256` checksum — ready to attach to a GitHub release. The archive is **thin**: it
contains `lode.jsonc`, `lode.lock`, and the overlay sources (config, scripts) — and deliberately
**no mod jars** (redistribution is forbidden; jars are fetched from the provider and verified
against the lock on install) and no runtime files.

The archive is reproducible — same inputs produce byte-identical output (sorted entries, zeroed
timestamps) — so the published checksum is meaningful.

| Flag | Description |
|------|-------------|
| `--out <dir>` | Directory to write the artifact into (default: the current directory). |

## `lode publish`

```sh
lode publish --tag <v> [--sign] [--repo <owner/repo>] [--prerelease]
```

Bundle the pack (like `bundle`) and publish it as a **GitHub release** — the producer side of
`get`. Creates the release for `--tag` (or attaches to it if it exists) and uploads the `.tar.gz`
and its `.sha256`. The repo is read from the `origin` remote unless `--repo` is given.

With `--sign`, it also **signs** the archive (keyless, via cosign) and uploads the `.sigstore`
bundle, so consumers can `lode get … --verify`. See [signing.md](signing.md).

Requires `GITHUB_TOKEN` set to a token with `contents: write` on the repo (and `cosign` on `PATH`
for `--sign`).

| Flag | Description |
|------|-------------|
| `--tag <v>` | The release tag to create or attach to (e.g. `v1.0.0`). **Required.** |
| `--sign` | Also sign the archive (keyless, via cosign) and upload the `.sigstore` bundle. |
| `--repo <owner/repo>` | Target repo (default: inferred from the `origin` remote). |
| `--prerelease` | Mark the release as a prerelease. |

## `lode verify`

```sh
lode verify [--into <dir>]
```

Re-hash the installed jars against `lode.lock` and report any that are missing, mismatched, or
unverifiable — a read-only integrity check you can run any time (e.g. before launching). A hash
mismatch means the pack was altered or corrupted, and exits non-zero. Warns if the lockfile is
stale relative to the manifest.

| Flag | Description |
|------|-------------|
| `--into <dir>` | Instance directory whose `mods/` to check (default: the pack root). |

## `lode config`

```sh
lode config set <key> <value>
lode config get <key>
```

Store settings outside the repo. The main key is `curseforge.key` — the CurseForge API
key (also read from the `CF_API_KEY` environment variable). Modrinth needs no key.
