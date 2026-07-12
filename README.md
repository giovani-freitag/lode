<p align="center">
  <img src="assets/icon.png" alt="lode icon" width="112">
</p>

<h1 align="center">lode</h1>

<p align="center">
  <strong>A package manager for Minecraft modpacks.</strong><br>
  Declare your mods in one file — <code>lode</code> resolves the dependencies, locks them by hash, and installs the jars.
</p>

<p align="center">
  <a href="https://github.com/giovani-freitag/lode/actions/workflows/ci.yml"><img src="https://github.com/giovani-freitag/lode/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/license-MIT-blue" alt="License: MIT">
  &nbsp;·&nbsp;
  <img src="https://img.shields.io/badge/Forge-1b1f3b" alt="Forge">
  <img src="https://img.shields.io/badge/NeoForge-ee6b2d" alt="NeoForge">
  <img src="https://img.shields.io/badge/Fabric-c7a17a" alt="Fabric">
  <img src="https://img.shields.io/badge/Quilt-8b2fc9" alt="Quilt">
</p>

---

Think **npm, but for Minecraft mods**. You keep a small, human-readable manifest (`lode.jsonc`); `lode` figures
out exact versions and dependencies, records them in a lockfile (`lode.lock`), and downloads the jars straight
into your instance — from **Modrinth** and **CurseForge**. No clone-and-build, no separate installer.

```sh
lode init                        # scaffold a pack — pick the loader + Minecraft version
lode add create                  # add a mod: resolve its deps, download the jars
lode install --server            # set up an instance from the lockfile (and provision the server)
lode get github.com/you/pack     # or fetch someone's published pack — checksum verified (add --verify for signature)
```

## ✨ Features

- 📄 **One manifest, one lockfile** — you edit `lode.jsonc`; `lode` generates `lode.lock`. Same result on every machine, exactly like `package.json` + `package-lock.json`.
- 🔗 **Real dependency resolution** — add a mod and its required deps come with it, deduplicated across the pack. `lode why <mod>` traces anything back to whatever pulled it in.
- 🔒 **Locked by hash** — every jar is pinned to its checksum; `lode verify` re-hashes an install and tells you if anything drifted or was tampered with.
- 🌐 **Modrinth and CurseForge in one pack** — pull from either platform freely; you're never tied to a single source.
- 📦 **Installs the loader, not just the mods** — `lode install --server` provisions the server and installs the server-side mods, no separate installer.
- ✍️ **Signed, verifiable releases** — `lode publish --sign` ships a signed GitHub release; `lode get … --verify` proves it came from your repo. Verification is native; signing uses cosign.
- 🔁 **packwiz bridge** — `lode import packwiz .` converts an existing pack in one step, and `lode export packwiz` bridges back out.

## 📦 Requirements

- A **JDK** to *run* the server, matched to the Minecraft version — **Java 17** for 1.20.x, **21** for 1.21+
  (`lode` provisions the loader, not Java). Pass `--java <path>` to choose one, or install
  [Adoptium Temurin](https://adoptium.net).
- A **CurseForge API key** only if you pull CurseForge mods — Modrinth works out of the box.

## 🚀 Install

```sh
# Linux / macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/giovani-freitag/lode/releases/latest/download/lode-installer.sh | sh

# Windows (PowerShell)
powershell -c "irm https://github.com/giovani-freitag/lode/releases/latest/download/lode-installer.ps1 | iex"

# Rust users — prebuilt binary, no compile (lode isn't on crates.io, so scope it to the repo)
cargo binstall --git https://github.com/giovani-freitag/lode lode
```

On Windows you can also use a package manager:

```powershell
winget install GiovaniFreitag.Lode
scoop bucket add lode https://github.com/giovani-freitag/scoop-lode; scoop install lode
```

Prefer clicking? Grab the **`.msi`** (Windows) or the tarball for your platform from the
[Releases page](https://github.com/giovani-freitag/lode/releases). From source:
`cargo install --git https://github.com/giovani-freitag/lode`.

Maintainer? How releases are cut lives in [docs/releasing.md](docs/releasing.md).

## 📖 Quick tour

Start a pack, add a couple of mods, and inspect what you've got:

```sh
lode init -y --loader forge --minecraft 1.20.1 --name "My Pack"
lode add create                  # from Modrinth — the default provider
lode add rei                     # Roughly Enough Items; its deps (Architectury, Cloth Config) come along
lode list                        # the resolved pack: direct mods vs. deps, side, provider
lode why cloth-config            # → required by: rei
lode install --server            # provisions Forge/NeoForge, then installs the server-side mods
```

Ship it, and let someone else install the *exact* same pack — verified end to end:

```sh
lode publish --sign --tag v1.0.0   # you
lode get github.com/you/pack --verify   # them
```

The published bundle is **thin** — manifest, lockfile, and your config/script overlays, but no jars. Those are
pulled from the provider on install and checked against the lockfile, so distribution stays within each mod's
redistribution rules and every install is byte-verified. Details in [docs/signing.md](docs/signing.md).

## 🧭 Commands

| Command | What it does |
|---------|--------------|
| `lode init` | Scaffold a pack (loader + Minecraft version). |
| `lode add <mod>` · `del` | Add / remove a mod — resolves deps, downloads the jars. |
| `lode install` | Set up an instance from the lockfile (`--server` provisions the loader too). |
| `lode get <host/owner/repo>` | Fetch + verify a published pack and set it up. |
| `lode publish` · `bundle` | Publish the pack as a signed release · build the distributable locally. |
| `lode update` · `list` · `why` | Bump versions · inspect the resolved pack and its provenance. |

Full reference, every flag → **[docs/commands.md](docs/commands.md)**.

## 📚 Docs

- **[Command reference](docs/commands.md)** — every command and flag.
- **[Signing & verification](docs/signing.md)** — sign a release; `lode get --verify`.
- **[Roadmap](docs/roadmap.md)** — what's done and what's next.

MIT licensed.
