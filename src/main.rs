#![allow(dead_code)]

extern crate ansi_term;
extern crate chrono;
extern crate clap;
#[macro_use]
extern crate maplit;
#[macro_use]
extern crate quick_error;
extern crate regex;
extern crate semver;
extern crate toml;

use std::io::{stdin, stdout, Write};
use std::process::exit;

use clap::{App, ArgMatches, SubCommand};
use semver::Identifier;
use ansi_term::Style;
use ansi_term::Colour::{Green, Red};
use toml::Value;

mod config;
mod error;
mod cmd;
mod git;
mod cargo;
mod version;
mod replace;

fn confirm(prompt: &str) -> bool {
    let mut input = String::new();

    print!(
        "{}",
        Style::new().bold().paint(format!("{} [y/N] ", prompt))
    );

    stdout().flush().unwrap();
    stdin().read_line(&mut input).expect("y/n required");

    input.trim().to_lowercase() == "y"
}

fn execute(args: &ArgMatches) -> Result<i32, error::FatalError> {
    let cargo_file = try!(config::parse_cargo_config());

    // step -1
    if let Some(invalid_keys) = config::verify_release_config(&cargo_file) {
        for i in invalid_keys {
            println!(
                "{}",
                Red.bold().paint(format!(
                    "Unknown config key \"{}\" found for [package.metadata.release]",
                    i
                ))
            );
        }
        return Ok(109);
    }

    let dry_run = args.occurrences_of("dry-run") > 0;
    let level = args.value_of("level");
    let sign = args.occurrences_of("sign") > 0
        || config::get_release_config(&cargo_file, config::SIGN_COMMIT)
            .and_then(|f| f.as_bool())
            .unwrap_or(false);
    let upload_doc = args.occurrences_of("upload-doc") > 0
        || config::get_release_config(&cargo_file, config::UPLOAD_DOC)
            .and_then(|f| f.as_bool())
            .unwrap_or(false);
    let git_remote = args.value_of("push-remote")
        .or_else(|| {
            config::get_release_config(&cargo_file, config::PUSH_REMOTE).and_then(|f| f.as_str())
        })
        .unwrap_or("origin");
    let doc_branch = args.value_of("doc-branch")
        .or_else(|| {
            config::get_release_config(&cargo_file, config::DOC_BRANCH).and_then(|f| f.as_str())
        })
        .unwrap_or("gh-pages");
    let skip_push = args.occurrences_of("skip-push") > 0
        || config::get_release_config(&cargo_file, config::DISABLE_PUSH)
            .and_then(|f| f.as_bool())
            .unwrap_or(false);
    let dev_version_ext = args.value_of("dev-version-ext")
        .or_else(|| {
            config::get_release_config(&cargo_file, config::DEV_VERSION_EXT)
                .and_then(|f| f.as_str())
        })
        .unwrap_or("alpha.0");
    let no_dev_version = args.occurrences_of("no-dev-version") > 0
        || config::get_release_config(&cargo_file, config::NO_DEV_VERSION)
            .and_then(|f| f.as_bool())
            .unwrap_or(false);
    let pre_release_commit_msg =
        config::get_release_config(&cargo_file, config::PRE_RELEASE_COMMIT_MESSAGE)
            .and_then(|f| f.as_str())
            .unwrap_or("(cargo-release) version {{version}}");
    let pro_release_commit_msg =
        config::get_release_config(&cargo_file, config::PRO_RELEASE_COMMIT_MESSAGE)
            .and_then(|f| f.as_str())
            .unwrap_or("(cargo-release) start next development iteration {{version}}");
    let pre_release_replacements =
        config::get_release_config(&cargo_file, config::PRE_RELEASE_REPLACEMENTS);
    let pre_release_hook = config::get_release_config(&cargo_file, config::PRE_RELEASE_HOOK)
        .and_then(|h| match h {
            &Value::String(ref s) => Some(vec![s.as_ref()]),
            &Value::Array(ref a) => Some(
                a.iter()
                    .map(|v| v.as_str())
                    .filter(|o| o.is_some())
                    .map(|s| s.unwrap())
                    .collect(),
            ),
            _ => None,
        });
    let pre_publish_hook = config::get_release_config(&cargo_file, config::PRE_PUBLISH_HOOK)
        .and_then(|h| match h {
            &Value::String(ref s) => Some(vec![s.as_ref()]),
            &Value::Array(ref a) => Some(
                a.iter()
                    .map(|v| v.as_str())
                    .filter(|o| o.is_some())
                    .map(|s| s.unwrap())
                    .collect(),
            ),
            _ => None,
        });
    let tag_msg = config::get_release_config(&cargo_file, config::TAG_MESSAGE)
        .and_then(|f| f.as_str())
        .unwrap_or("(cargo-release) {{prefix}} version {{version}}");
    let doc_commit_msg = config::get_release_config(&cargo_file, config::DOC_COMMIT_MESSAGE)
        .and_then(|f| f.as_str())
        .unwrap_or("(cargo-release) generate docs");
    let no_confirm = args.occurrences_of("no-confirm") > 0;
    let publish = cargo_file
        .get("package")
        .and_then(|f| f.as_table())
        .and_then(|f| f.get("publish"))
        .and_then(|f| f.as_bool())
        .unwrap_or(true);

    // STEP 0: Check if working directory is clean
    if !try!(git::status()) {
        println!(
            "{}",
            Red.bold()
                .paint("Uncommitted changes detected, please commit before release.",)
        );
        if !dry_run {
            return Ok(101);
        }
    }

    // STEP 1: Read version from Cargo.toml and remove
    let mut version = cargo_file
        .get("package")
        .and_then(|f| f.as_table())
        .and_then(|f| f.get("version"))
        .and_then(|f| f.as_str())
        .and_then(|f| config::parse_version(f).ok())
        .unwrap();
    let prev_version_string = version.to_string();

    // STEP 2: update current version, save and commit
    if try!(version::bump_version(&mut version, level)) {
        let new_version_string = version.to_string();
        // Release Confirmation
        if !dry_run {
            if !no_confirm {
                if !confirm(&format!("Release version {} ?", new_version_string)) {
                    return Ok(0);
                }
            }
        }

        // pre-release hook
        if let Some(pre_rel_hook) = pre_release_hook {
            println!(
                "{}",
                Green.paint(format!("Calling pre-release hook: {:?}", pre_rel_hook))
            );
            let envs = btreemap!{
                "PREV_VERSION" => prev_version_string.as_ref(),
                "NEW_VERSION" => new_version_string.as_ref(),
                "DRY_RUN" => if dry_run { "true" } else { "false" }
            };
            // we use dry_run environmental variable to run the script
            // so here we set dry_run=false and always execute the command.
            if !try!(cmd::call_with_env(pre_rel_hook, envs, false)) {
                println!(
                    "{}",
                    Red.paint("Release aborted by non-zero return of prerelease hook.")
                );
                return Ok(107);
            }
        }

        println!(
            "{}",
            Green.paint(format!(
                "Update to version {} and commit",
                new_version_string
            ))
        );
        if !dry_run {
            try!(config::rewrite_cargo_version(&new_version_string));
        }

        if let Some(pre_rel_rep) = pre_release_replacements {
            // try update version number in configured files
            try!(replace::do_replace_versions(
                pre_rel_rep,
                &new_version_string,
                dry_run,
            ));
        }

        let commit_msg =
            String::from(pre_release_commit_msg).replace("{{version}}", &new_version_string);
        if !try!(git::commit_all(".", &commit_msg, sign, dry_run)) {
            // commit failed, abort release
            return Ok(102);
        }
    }

    // STEP 3: cargo publish
    if publish {
        // pre-publish hook
        if let Some(pre_pub_hook) = pre_publish_hook {
            println!(
                "{}",
                Green.paint(format!("Calling pre-publish hook: {:?}", pre_pub_hook))
            );
            let envs = btreemap!{
                "DRY_RUN" => if dry_run { "true" } else { "false" }
            };
            // we use dry_run environmental variable to run the script
            // so here we set dry_run=false and always execute the command.
            if !try!(cmd::call_with_env(pre_pub_hook, envs, false)) {
                println!(
                    "{}",
                    Red.paint("Release aborted by non-zero return of pre-publish hook.")
                );
                return Ok(108);
            }
        }

        println!("{}", Green.paint("Running cargo publish"));
        if !try!(cargo::publish(dry_run)) {
            return Ok(103);
        }
    }

    // STEP 4: upload doc
    if upload_doc {
        println!("{}", Green.paint("Building and exporting docs."));
        try!(cargo::doc(dry_run));

        let doc_path = "target/doc/";

        println!("{}", Green.paint("Commit and push docs."));
        try!(git::init(doc_path, dry_run));
        try!(git::add_all(doc_path, dry_run));
        try!(git::commit_all(doc_path, doc_commit_msg, sign, dry_run));
        let default_remote = try!(git::origin_url());

        let mut refspec = String::from("master:");
        refspec.push_str(doc_branch);

        try!(git::force_push(
            doc_path,
            default_remote.trim(),
            &refspec,
            dry_run,
        ));
    }

    // STEP 5: Tag
    let root = try!(git::top_level());
    let rel_path = try!(cmd::relative_path_for(&root));
    let tag_prefix = args.value_of("tag-prefix")
        .map(|t| t.to_owned())
        .or_else(|| {
            config::get_release_config(&cargo_file, config::TAG_PREFIX)
                .and_then(|f| f.as_str())
                .map(|f| f.to_string())
        })
        .or_else(|| rel_path.as_ref().map(|t| format!("{}-", t)));

    let current_version = version.to_string();
    let tag_name = tag_prefix.as_ref().map_or_else(
        || current_version.clone(),
        |x| format!("{}{}", x, current_version),
    );

    let tag_message = String::from(tag_msg)
        .replace("{{prefix}}", tag_prefix.as_ref().unwrap_or(&"".to_owned()))
        .replace("{{version}}", &current_version);

    println!("{}", Green.paint(format!("Creating git tag {}", tag_name)));
    if !try!(git::tag(&tag_name, &tag_message, sign, dry_run)) {
        // tag failed, abort release
        return Ok(104);
    }

    // STEP 6: bump version
    if !version::is_pre_release(level) && !no_dev_version {
        version.increment_patch();
        version
            .pre
            .push(Identifier::AlphaNumeric(dev_version_ext.to_owned()));
        println!(
            "{}",
            Green.paint(format!("Starting next development iteration {}", version))
        );
        let updated_version_string = version.to_string();
        if !dry_run {
            try!(config::rewrite_cargo_version(&updated_version_string));
        }
        let commit_msg =
            String::from(pro_release_commit_msg).replace("{{version}}", &updated_version_string);

        if !try!(git::commit_all(".", &commit_msg, sign, dry_run)) {
            return Ok(105);
        }
    }

    // STEP 7: git push
    if !skip_push {
        println!("{}", Green.paint("Pushing to git remote"));
        if !try!(git::push(git_remote, dry_run)) {
            return Ok(106);
        }
        if !try!(git::push_tag(git_remote, &tag_name, dry_run)) {
            return Ok(106);
        }
    }

    println!("{}", Green.paint("Finished"));
    Ok(0)
}

static USAGE: &'static str = "-l, --level=[level] 'Release level: bumpping major|minor|patch version on release or removing prerelease extensions by default'
                             [sign]... --sign 'Sign git commit and tag'
                             [dry-run]... --dry-run 'Do not actually change anything'
                             [upload-doc]... --upload-doc 'Upload rust document to gh-pages branch'
                             --push-remote=[push-remote] 'Git remote to push'
                             [skip-push]... --skip-push 'Do not run git push in the last step'
                             --doc-branch=[doc-branch] 'Git branch to push documentation on'
                             --tag-prefix=[tag-prefix] 'Prefix of git tag, note that this will override default prefix based on sub-directory'
                             --dev-version-ext=[dev-version-ext] 'Pre-release identifier(s) to append to the next development version after release'
                             [no-dev-version]... --no-dev-version 'Do not create dev version after release'
                             [no-confirm]... --no-confirm 'Skip release confirmation'";

fn main() {
    let matches = App::new("cargo")
        .subcommand(
            SubCommand::with_name("release")
                .version(env!("CARGO_PKG_VERSION"))
                .author("Ning Sun <sunng@protonmail.ch>")
                .about("Cargo subcommand for you to smooth your release process.")
                .args_from_usage(USAGE),
        )
        .get_matches();

    if let Some(ref release_matches) = matches.subcommand_matches("release") {
        match execute(release_matches) {
            Ok(code) => exit(code),
            Err(e) => {
                println!("{}", Red.bold().paint(format!("Fatal: {}", e)));
                exit(128);
            }
        }
    }
}
