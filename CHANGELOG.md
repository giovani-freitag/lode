# Changelog

## [0.2.1](https://github.com/giovani-freitag/lode/compare/v0.2.0...v0.2.1) (2026-07-14)


### Features

* **init:** default the author from git config user.name ([90e1d6d](https://github.com/giovani-freitag/lode/commit/90e1d6d06b5cda776e9d271d216df0f68060b291))

## [0.2.0](https://github.com/giovani-freitag/lode/compare/v0.1.0...v0.2.0) (2026-07-14)


### ⚠ BREAKING CHANGES

* the manifest is now strict JSON named `lode.json` (was `lode.jsonc`, JSONC-tolerant) -- rename existing manifests and remove any comments. `lode add` no longer guesses a mod from a fuzzy match (use `--search`), and the `-y` flag on `add` was removed.

### Features

* exact-first `add`, strict `lode.json` manifest, and an interactive `init` overhaul ([f5bee1e](https://github.com/giovani-freitag/lode/commit/f5bee1e6dc4917acda946c9b76d778cda5358ccf))
* **init:** show next steps after scaffolding a pack ([c5f0ef6](https://github.com/giovani-freitag/lode/commit/c5f0ef6d6cba9c2c697f0317c2ad701c23e0b33f))
* **ui:** clack-style interactive prompts and progress spinners via cliclack ([9fd9aa7](https://github.com/giovani-freitag/lode/commit/9fd9aa7cef159988d60ea7e3e73025f5f7bbe287))


### Bug Fixes

* **init:** drop the version-fetch spinners that collided with the selects ([a2285ef](https://github.com/giovani-freitag/lode/commit/a2285ef96624031fed178294a2aac6a6ed9489d2))
* **init:** filter + cap the version pickers so long lists scroll instead of flooding ([1a642b1](https://github.com/giovani-freitag/lode/commit/1a642b1b4a1416163e5400b7561f42b04ce71d6a))

## [0.1.0](https://github.com/giovani-freitag/lode/compare/v0.0.1...v0.1.0) (2026-07-12)


### Features

* lode — a Minecraft modpack manager ([b49b02a](https://github.com/giovani-freitag/lode/commit/b49b02a155154eb7da6b1b458bc0cb49dce27689))
