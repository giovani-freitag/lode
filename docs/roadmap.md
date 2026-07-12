# Roadmap

Rough, unordered — priorities shift with use.

## Distribution (reach both audiences)
- **More install channels:** winget · Scoop · Homebrew · npm · apt/dnf repositories.
- **Code signing** for the Windows `.exe`/`.msi` (avoid the SmartScreen/Defender
  warning for end users) — e.g. SignPath (free for OSS) or Azure Trusted Signing.

## Resolution & manifest
- **Version ranges:** `^` / `~` constraints (today: `*` = latest, or an exact version).
- **CurseForge:** validate the live add/install path end-to-end (needs an API key).
- **Comment preservation** when `lode add` rewrites `lode.jsonc`.

## Packs
- **Multi-loader packs** (targeting more than one loader).
- Richer `list` / `update` UX (release channels, ignore a specific version, changelogs).
- Advanced overlays and local-jar sources.

## Quality
- Broaden test coverage (golden tests for CurseForge, resolver edge cases).
