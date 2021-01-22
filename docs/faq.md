# FAQ

## How do I update my README or other files

Cargo-release 0.8 allows you to search and replace version string in
any project or source file.

For example, to update the version in predicates
[`README.md`](https://github.com/assert-rs/predicates-rs/blob/master/README.md):
```toml
[dependencies]
predicates = "1.0.2"
```

You use the following
[`release.toml`](https://github.com/assert-rs/predicates-rs/blob/master/release.toml):
```toml
pre-release-replacements = [
  {file="README.md", search="predicates = .*", replace="{{crate_name}} = \"{{version}}\""},
  {file="src/lib.rs", search="predicates = .*", replace="{{crate_name}} = \"{{version}}\""},
]
```

Note: we only substitute variables on `replace` and not `search` so you'll need
to change `predicates` to match your crate name.

See [`pre-release-replacements`](reference.md) for more.

## Maintaining Changelog

At the moment, `cargo release` won't try to generate a changelog from
git history or anything. Because I think changelog is an important
communication between developer and users, which requires careful maintenance.

However, you can still use [`pre-release-replacements`](reference.md) to smooth your
process of releasing a changelog, along with your crate. You need to
keep your changelog arranged during feature development, in an `Unreleased`
section (recommended by [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)):

```markdown
<!-- next-header -->

## [Unreleased] - ReleaseDate

### Added
- feature 3

### Changed
- bug 1

## [1.0.0] - 2017-06-20
### Added
- feature 1
- feature 2

<!-- next-url -->
[Unreleased]: https://github.com/assert-rs/predicates-rs/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/assert-rs/predicates-rs/compare/v0.9.0...v1.0.0
```

In `release.toml`, configure `cargo release` to do replacements while
bumping version:

```toml
pre-release-replacements = [
  {file="CHANGELOG.md", search="Unreleased", replace="{{version}}"},
  {file="CHANGELOG.md", search="\\.\\.\\.HEAD", replace="...{{tag_name}}", exactly=1},
  {file="CHANGELOG.md", search="ReleaseDate", replace="{{date}}"},
  {file="CHANGELOG.md", search="<!-- next-header -->", replace="<!-- next-header -->\n\n## [Unreleased] - ReleaseDate", exactly=1},
  {file="CHANGELOG.md", search="<!-- next-url -->", replace="<!-- next-url -->\n[Unreleased]: https://github.com/assert-rs/predicates-rs/compare/{{tag_name}}...HEAD", exactly=1},
]
```

`{{version}}` and `{{date}}` are pre-defined variables with value of
current release version and date.

`predicates` is a real world example
- [`release.toml`](https://github.com/assert-rs/predicates-rs/blob/master/release.toml)
- [`CHANGELOG.md`](https://github.com/assert-rs/predicates-rs/blob/master/CHANGELOG.md)

## How do I apply a setting only to one crate in my workspace?

Example problems:
- Release fails because `cargo-release` was trying to update a non-existent `CHANGELOG.md` ([#157](https://github.com/sunng87/cargo-release/issues/157))
- Only create one tag for the entire workspace ([#162](https://github.com/sunng87/cargo-release/issues/162))

Somethings only need to be done on for a release, like updating the
`CHANGELOG.md`, no matter how many crates are being released.  Usually these
operations are tied to a specific crate.  This is straightforward when you do
not have a crate in your workspace root.

When you have a root crate, it shares its `release.toml` with the workspace,
making it less obvious how to do root-crate-specific settings.   If you'd like
to customize settings differently for the root crate vs the other crate's, you
have two choices:
- Put the common setting in the workspace's `release.toml` and override it for the root crate in `Cargo.toml`.
- Modify each crate's `release.toml` with the setting relevant for that crate.

## How do I customize my tagging in a workspace?

Example problems:
- Customizing tags while needing the root workspace to follow a specific convention ([#172](https://github.com/sunng87/cargo-release/issues/172))

By default, your tag will look like:
- `v{{version}}` if the crate is in the repo root.
- `{{crate_name}}-v{{version}}` otherwise.

This is determined by `tag-name` with the default `"{{prefix}}v{{version}}"`.
- `"{{prefix}}"` comes from `tag-prefix` which has two defaults:
  - `""` if the crate is in the repo root.
  - `"{{crate_name}}-"` otherwise.
- `"{{version}}"` is the current crate's version.

Other variables are available for use.  See the [reference](reference.md) for more.

`tag-name`, `tag-prefix` come from the config file and `cargo-release` uses a layered config.  The relevant layers are:
1. Read from the crate's `Cargo.toml`
2. Read from the crate's `release.toml`
3. Read from the workspace's `release.toml`.

Something to keep in mind is if you have a crate in your workspace root, it
shares the `release.toml` with the workspace.  If you'd like to customize the
tag differently for the root crate vs the other crate's, you have two choices:
- Put the common setting in the workspace's `release.toml` and override it for the root crate in `Cargo.toml`.
- Modify each crate's `release.toml` with the setting relevant for that crate.
