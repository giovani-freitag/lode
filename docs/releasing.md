# Releasing

A release is cut by merging the release PR; everything after that is automated by
[release-please] and [cargo-dist].

## Flow

1. Conventional-commit PRs land on `main`.
2. **release-please** keeps a release PR open that bumps the version and updates the changelog.
   Merging it tags the commit and cuts a draft GitHub release.
3. That merge dispatches **cargo-dist** (`release.yml`), which builds every target, uploads the
   archives and installers, and flips the release public.
4. In the same run cargo-dist calls `publish-package-managers.yml`, which pushes the new version to
   winget and Scoop — each step skips cleanly until its token is configured.

The release stays a draft until its binaries are attached, so the `releases/latest/download/…`
installer URLs never resolve to an incomplete release. Versions belong to release-please
(`release-please-config.json`, `.release-please-manifest.json`) and are never hand-edited.

[release-please]: https://github.com/googleapis/release-please
[cargo-dist]: https://github.com/axodotdev/cargo-dist
