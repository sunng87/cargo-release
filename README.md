# cargo release

[![](http://meritbadge.herokuapp.com/cargo-release)](https://crates.io/crates/cargo-release)
[![Build Status](https://travis-ci.org/sunng87/cargo-release.svg?branch=master)](https://travis-ci.org/sunng87/cargo-release)
[![Donate](https://img.shields.io/badge/donate-liberapay-yellow.svg)](https://liberapay.com/Sunng/donate)

Performs release best-practices, including:

* Ensure the git working directory is clean (except if the `--allow-dirty` flag is set).
* Bump the version in Cargo.toml
* Run `cargo publish` ([if not disabled](https://doc.rust-lang.org/cargo/reference/manifest.html#the-publish--field-optional))
* Create a git tag for this version
* Bump version for next development cycle
* `git push`

## Install

Current release: 0.12.4

`cargo install cargo-release`

## Usage

`cargo release [level]`

* See the [reference](docs/reference.md) for more on `level`, other CLI
  arguments, and configuration file format.
* See also the [FAQ](docs/faq.md) for help in figuring out how to adapt
  cargo-release to your workflow.

### Prerequisite

* Your project should be managed by git.

### Dry run

We recommend calling `cargo release --dry-run` with your custom options before
actually executing it. The dry-run mode will print all commands to
execute during the release process. And you will get an overview of
what's going on.

Here is an example.

```
 $ cargo release --dry-run
cd .
git commit -S -am (cargo-release) version 0.18.3
cd -
cargo publish
Building and exporting docs.
cargo doc --no-deps
cd target/doc/
git init
cd -
cd target/doc/
git add .
cd -
cd target/doc/
git commit -S -am (cargo-release) generate docs
cd -
cd target/doc/
git push -f git@github.com:sunng87/handlebars-rust.git master:gh-pages
cd -
git tag -a 0.18.3 -m (cargo-release)  version 0.18.3 -s
Starting next development iteration 0.18.4-pre
cd .
git commit -S -am (cargo-release) start next development iteration 0.18.4-pre
cd -
git push origin --follow-tags
```

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
  at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.

### Donation

I'm now accepting donation on [liberapay](https://liberapay.com/Sunng/donate),
if you find my work helpful and want to keep it going.
