#[macro_use]
extern crate clap;
#[macro_use]
extern crate maplit;
#[macro_use]
extern crate quick_error;

use structopt;

#[cfg(test)]
extern crate assert_fs;

use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::Path;
use std::process::exit;

use boolinator::Boolinator;
use chrono::prelude::Local;
use semver::Identifier;
use structopt::StructOpt;

use crate::error::FatalError;
use crate::replace::{do_file_replacements, replace_in, Replacements};

mod cargo;
mod cmd;
mod config;
mod error;
mod git;
mod replace;
mod shell;
mod version;

fn find_dependents<'w>(
    ws_meta: &'w cargo_metadata::Metadata,
    pkg_meta: &'w cargo_metadata::Package,
) -> impl Iterator<Item = (&'w cargo_metadata::Package, &'w cargo_metadata::Dependency)> {
    ws_meta.packages.iter().filter_map(move |p| {
        if ws_meta.workspace_members.iter().any(|m| *m == p.id) {
            p.dependencies
                .iter()
                .find(|d| d.name == pkg_meta.name)
                .map(|d| (p, d))
        } else {
            None
        }
    })
}

struct Package<'m> {
    meta: &'m cargo_metadata::Package,
    manifest_path: &'m Path,
    package_path: &'m Path,
    config: config::Config,
}

impl<'m> Package<'m> {
    fn load(
        args: &ReleaseOpt,
        ws_meta: &'m cargo_metadata::Metadata,
        pkg_meta: &'m cargo_metadata::Package,
    ) -> Result<Self, error::FatalError> {
        let manifest_path = pkg_meta.manifest_path.as_path();
        let cwd = manifest_path.parent().unwrap_or_else(|| Path::new("."));

        let release_config = {
            let mut release_config = config::Config::default();

            if !args.isolated {
                let cfg = config::resolve_config(&ws_meta.workspace_root, &manifest_path)?;
                release_config.update(&cfg);
            }

            if let Some(custom_config_path) = args.custom_config.as_ref() {
                // when calling with -c option
                let cfg = config::resolve_custom_config(Path::new(custom_config_path))?
                    .unwrap_or_default();
                release_config.update(&cfg);
            }

            release_config.update(&args.config);

            // the publish flag in cargo file
            let cargo_file = cargo::parse_cargo_config(&manifest_path)?;
            if !cargo_file
                .get("package")
                .and_then(|f| f.as_table())
                .and_then(|f| f.get("publish"))
                .and_then(|f| f.as_bool())
                .unwrap_or(true)
            {
                release_config.disable_publish = Some(true);
            }

            release_config
        };

        let pkg = Package {
            meta: pkg_meta,
            manifest_path,
            package_path: cwd,
            config: release_config,
        };
        Ok(pkg)
    }
}

fn release_workspace(args: &ReleaseOpt) -> Result<i32, error::FatalError> {
    let ws_meta = args.manifest.metadata().exec().map_err(FatalError::from)?;

    let (selected_pkgs, _excluded_pkgs) = args.workspace.partition_packages(&ws_meta);
    if selected_pkgs.is_empty() {
        log::info!("No packages selected.");
        return Ok(0);
    }

    let pkgs: Result<HashMap<_, _>, _> = selected_pkgs
        .iter()
        .map(|p| Package::load(args, &ws_meta, p).map(|p| (&p.meta.id, p)))
        .collect();
    let pkgs = pkgs?;

    git::git_version()?;

    if git::is_dirty(&ws_meta.workspace_root)? {
        log::warn!("Uncommitted changes detected, please commit before release.");
        if !args.dry_run {
            return Ok(101);
        }
    }

    let dep_tree: HashMap<_, _> = ws_meta
        .resolve
        .as_ref()
        .expect("cargo-metadata resolved deps")
        .nodes
        .iter()
        .map(|n| (&n.id, &n.dependencies))
        .collect();

    let mut processed = HashSet::new();

    for node in ws_meta
        .resolve
        .as_ref()
        .expect("cargo-metadata resolved deps")
        .nodes
        .iter()
    {
        let code =
            release_workspace_inner(args, &ws_meta, &node.id, &pkgs, &dep_tree, &mut processed)?;
        if code != 0 {
            return Ok(code);
        }
    }

    Ok(0)
}

fn release_workspace_inner<'m>(
    args: &ReleaseOpt,
    ws_meta: &'m cargo_metadata::Metadata,
    pkg_id: &'m cargo_metadata::PackageId,
    pkgs: &std::collections::HashMap<&'m cargo_metadata::PackageId, Package<'m>>,
    dep_tree: &std::collections::HashMap<
        &'m cargo_metadata::PackageId,
        &'m std::vec::Vec<cargo_metadata::PackageId>,
    >,
    processed: &mut std::collections::HashSet<&'m cargo_metadata::PackageId>,
) -> Result<i32, error::FatalError> {
    if !processed.insert(pkg_id) {
        return Ok(0);
    }

    for dep_id in dep_tree[pkg_id].iter() {
        let code = release_workspace_inner(args, ws_meta, dep_id, pkgs, dep_tree, processed)?;
        if code != 0 {
            return Ok(code);
        }
    }

    if let Some(pkg) = pkgs.get(pkg_id) {
        let code = release_package(args, ws_meta, pkg)?;
        if code != 0 {
            return Ok(code);
        }
    }

    Ok(0)
}

fn release_package(
    args: &ReleaseOpt,
    ws_meta: &cargo_metadata::Metadata,
    pkg: &Package<'_>,
) -> Result<i32, error::FatalError> {
    let dry_run = args.dry_run;
    let sign = pkg.config.sign_commit();
    let cwd = pkg.package_path;

    // STEP 1: Query a bunch of information for later use.
    let mut version = pkg.meta.version.clone();
    let prev_version_string = version.to_string();

    let crate_name = pkg.meta.name.as_str();

    let mut replacements = Replacements::new();
    replacements.insert("{{prev_version}}", prev_version_string.clone());
    replacements.insert("{{version}}", version.to_string());
    replacements.insert("{{crate_name}}", crate_name.to_owned());
    replacements.insert("{{date}}", Local::now().format("%Y-%m-%d").to_string());

    let root = git::top_level(cwd)?;
    let is_root = root == cwd;
    let tag_prefix = pkg.config.tag_prefix().unwrap_or_else(|| {
        // crate_name as default tag prefix for multi-crate project
        if !is_root {
            "{{crate_name}}-"
        } else {
            ""
        }
    });
    let tag_prefix = replace_in(&tag_prefix, &replacements);
    replacements.insert("{{prefix}}", tag_prefix.clone());

    // STEP 2: update current version, save and commit
    if args
        .level
        .bump_version(&mut version, args.metadata.as_ref())?
    {
        // Must run before `{{version}}` gets updated with the next version
        let prev_tag_name = replace_in(pkg.config.tag_name(), &replacements);
        if let Some(changed) = git::changed_from(cwd, &prev_tag_name)? {
            if !changed {
                log::warn!(
                    "Releasing {} despite no changes made since tag {}",
                    crate_name,
                    prev_tag_name
                );
            }
        } else {
            log::info!(
                "Cannot detect changes for {} because tag {} is missing",
                crate_name,
                prev_tag_name
            );
        }

        let new_version_string = version.to_string();
        replacements.insert("{{version}}", new_version_string.clone());
        // Release Confirmation
        if !dry_run && !args.no_confirm {
            let confirmed = shell::confirm(&format!(
                "Release version {} {}?",
                crate_name, new_version_string
            ));
            if !confirmed {
                return Ok(0);
            }
        }

        log::info!("Update {} to version {}", crate_name, new_version_string);
        if !dry_run {
            cargo::set_package_version(&pkg.manifest_path, &new_version_string)?;
        }
        let mut dependents_failed = false;
        for (dep_pkg, dep) in find_dependents(&ws_meta, &pkg.meta) {
            match pkg.config.dependent_version() {
                config::DependentVersion::Ignore => (),
                config::DependentVersion::Warn => {
                    if !dep.req.matches(&version) {
                        log::warn!(
                            "{}'s dependency on {} `{}` is incompatible with {}",
                            dep_pkg.name,
                            pkg.meta.name,
                            dep.req,
                            new_version_string
                        );
                    }
                }
                config::DependentVersion::Error => {
                    if !dep.req.matches(&version) {
                        log::warn!(
                            "{}'s dependency on {} `{}` is incompatible with {}",
                            dep_pkg.name,
                            pkg.meta.name,
                            dep.req,
                            new_version_string
                        );
                        dependents_failed = true;
                    }
                }
                config::DependentVersion::Fix => {
                    if !dep.req.matches(&version) {
                        let new_req = version::set_requirement(&dep.req, &version)?;
                        if let Some(new_req) = new_req {
                            if dry_run {
                                log::info!(
                                    "Fixing {}'s dependency on {} to `{}` (from `{}`)",
                                    dep_pkg.name,
                                    pkg.meta.name,
                                    new_req,
                                    dep.req
                                );
                            } else {
                                cargo::set_dependency_version(
                                    &dep_pkg.manifest_path,
                                    &pkg.meta.name,
                                    &new_req,
                                )?;
                            }
                        }
                    }
                }
                config::DependentVersion::Upgrade => {
                    let new_req = version::set_requirement(&dep.req, &version)?;
                    if let Some(new_req) = new_req {
                        if dry_run {
                            log::info!(
                                "Upgrading {}'s dependency on {} to `{}` (from `{}`)",
                                dep_pkg.name,
                                pkg.meta.name,
                                new_req,
                                dep.req
                            );
                        } else {
                            cargo::set_dependency_version(
                                &dep_pkg.manifest_path,
                                &pkg.meta.name,
                                &new_req,
                            )?;
                        }
                    }
                }
            }
        }
        if dependents_failed {
            return Ok(110);
        }
        if dry_run {
            log::debug!("Updating lock file");
        } else {
            cargo::update_lock(&pkg.manifest_path)?;
        }

        if !pkg.config.pre_release_replacements().is_empty() {
            // try replacing text in configured files
            do_file_replacements(
                pkg.config.pre_release_replacements(),
                &replacements,
                cwd,
                dry_run,
            )?;
        }

        // pre-release hook
        if let Some(pre_rel_hook) = pkg.config.pre_release_hook() {
            let pre_rel_hook = pre_rel_hook.args();
            log::debug!("Calling pre-release hook: {:?}", pre_rel_hook);
            let envs = btreemap! {
                OsStr::new("PREV_VERSION") => prev_version_string.as_ref(),
                OsStr::new("NEW_VERSION") => new_version_string.as_ref(),
                OsStr::new("DRY_RUN") => OsStr::new(if dry_run { "true" } else { "false" }),
                OsStr::new("CRATE_NAME") => OsStr::new(crate_name),
                OsStr::new("WORKSPACE_ROOT") => ws_meta.workspace_root.as_os_str(),
                OsStr::new("CRATE_ROOT") => pkg.manifest_path.parent().unwrap_or_else(|| Path::new(".")).as_os_str(),
            };
            // we use dry_run environmental variable to run the script
            // so here we set dry_run=false and always execute the command.
            if !cmd::call_with_env(pre_rel_hook, envs, cwd, false)? {
                log::warn!(
                    "Release of {} aborted by non-zero return of prerelease hook.",
                    crate_name
                );
                return Ok(107);
            }
        }

        let commit_msg = replace_in(pkg.config.pre_release_commit_message(), &replacements);
        if !git::commit_all(cwd, &commit_msg, sign, dry_run)? {
            // commit failed, abort release
            return Ok(102);
        }
    }

    // STEP 3: cargo publish
    if !pkg.config.disable_publish() {
        log::info!("Running cargo publish on {}", crate_name);
        // feature list to release
        let feature_list = if !pkg.config.enable_features().is_empty() {
            Some(pkg.config.enable_features().to_owned())
        } else {
            None
        };
        // flag to release all features
        let all_features = pkg.config.enable_all_features();

        let features = if all_features {
            Features::All
        } else {
            match feature_list {
                Some(vec) => Features::Selective(vec),
                None => Features::None,
            }
        };
        if !cargo::publish(dry_run, &pkg.manifest_path, features)? {
            return Ok(103);
        }
    }

    // STEP 4: upload doc
    if pkg.config.upload_doc() {
        log::info!("Building and exporting docs for {}", crate_name);
        cargo::doc(dry_run, &pkg.manifest_path)?;

        let doc_path = ws_meta.target_directory.join("doc");

        log::info!("Commit and push docs for {}", crate_name);
        git::init(&doc_path, dry_run)?;
        git::add_all(&doc_path, dry_run)?;
        git::commit_all(&doc_path, pkg.config.doc_commit_message(), sign, dry_run)?;
        let default_remote = git::origin_url(cwd)?;

        let refspec = format!("master:{}", pkg.config.doc_branch());
        git::force_push(&doc_path, default_remote.trim(), &refspec, dry_run)?;
    }

    // STEP 5: Tag
    let tag_name = replace_in(pkg.config.tag_name(), &replacements);
    replacements.insert("{{tag_name}}", tag_name.clone());

    if !pkg.config.disable_tag() {
        let tag_message = replace_in(pkg.config.tag_message(), &replacements);

        log::debug!("Creating git tag {}", tag_name);
        if !git::tag(cwd, &tag_name, &tag_message, sign, dry_run)? {
            // tag failed, abort release
            return Ok(104);
        }
    }

    // STEP 6: bump version
    if !args.level.is_pre_release() && !pkg.config.no_dev_version() {
        version.increment_patch();
        version.pre.push(Identifier::AlphaNumeric(
            pkg.config.dev_version_ext().to_owned(),
        ));
        log::info!(
            "Starting {}'s next development iteration {}",
            crate_name,
            version
        );
        let updated_version_string = version.to_string();
        replacements.insert("{{next_version}}", updated_version_string.clone());
        if !dry_run {
            cargo::set_package_version(&pkg.manifest_path, &updated_version_string)?;
            cargo::update_lock(&pkg.manifest_path)?;
        }
        let commit_msg = replace_in(pkg.config.post_release_commit_message(), &replacements);

        if !git::commit_all(cwd, &commit_msg, sign, dry_run)? {
            return Ok(105);
        }
    }

    // STEP 7: git push
    if !pkg.config.disable_push() {
        log::info!("Pushing to git remote");
        let git_remote = pkg.config.push_remote();
        if git::check_remote(cwd, git_remote) {
            if !git::push(cwd, git_remote, dry_run)? {
                return Ok(106);
            }
            if !pkg.config.disable_tag() && !git::push_tag(cwd, git_remote, &tag_name, dry_run)? {
                return Ok(106);
            }
        } else {
            log::info!(
                "push_remote: {} is not configured for git, skip git push",
                git_remote
            );
        }
    }

    log::info!("Finished {}", crate_name);
    Ok(0)
}

/// Expresses what features flags should be used
pub enum Features {
    /// None - don't use special features
    None,
    /// Only use selected features
    Selective(Vec<String>),
    /// Use all features via `all-features`
    All,
}

#[derive(Debug, StructOpt)]
struct ReleaseOpt {
    #[structopt(flatten)]
    manifest: clap_cargo::Manifest,

    #[structopt(flatten)]
    workspace: clap_cargo::Workspace,

    /// Release level: bumping specified version field or remove prerelease extensions by default
    #[structopt(
        possible_values(&version::BumpLevel::variants()),
        case_insensitive(true),
        default_value = "release"
    )]
    level: version::BumpLevel,

    #[structopt(short = "m")]
    /// Semver metadata
    metadata: Option<String>,

    #[structopt(short = "c", long = "config")]
    /// Custom config file
    custom_config: Option<String>,

    #[structopt(long)]
    /// Ignore implicit configuration files.
    isolated: bool,

    #[structopt(flatten)]
    config: ConfigArgs,

    #[structopt(short = "n", long)]
    /// Do not actually change anything, just log what are going to do
    dry_run: bool,

    #[structopt(long)]
    /// Skip release confirmation and version preview
    no_confirm: bool,

    #[structopt(flatten)]
    logging: Verbosity,
}

#[derive(StructOpt, Debug, Clone)]
pub struct Verbosity {
    /// Pass many times for more log output
    ///
    /// By default, it'll only report errors. Passing `-v` one time also prints
    /// warnings, `-vv` enables info logging, `-vvv` debug, and `-vvvv` trace.
    #[structopt(long = "verbosity", short = "v", parse(from_occurrences))]
    verbosity: u8,
}

impl Verbosity {
    /// Get the log level.
    pub fn log_level(&self) -> log::Level {
        match self.verbosity {
            0 => log::Level::Error,
            1 => log::Level::Warn,
            2 => log::Level::Info,
            3 => log::Level::Debug,
            _ => log::Level::Trace,
        }
    }
}

#[derive(Debug, StructOpt)]
struct ConfigArgs {
    #[structopt(long)]
    /// Sign git commit and tag
    sign: bool,

    #[structopt(long)]
    /// Upload rust document to gh-pages branch
    upload_doc: bool,

    #[structopt(long)]
    /// Git remote to push
    push_remote: Option<String>,

    #[structopt(long)]
    /// Do not run cargo publish on release
    skip_publish: bool,

    #[structopt(long)]
    /// Do not run git push in the last step
    skip_push: bool,

    #[structopt(long)]
    /// Do not create git tag
    skip_tag: bool,

    #[structopt(
        long,
        possible_values(&config::DependentVersion::variants()),
        case_insensitive(true),
    )]
    /// Specify how workspace dependencies on this crate should be handed.
    dependent_version: Option<config::DependentVersion>,

    #[structopt(long)]
    /// Git branch to push documentation on
    doc_branch: Option<String>,

    #[structopt(long)]
    /// Prefix of git tag, note that this will override default prefix based on sub-directory
    tag_prefix: Option<String>,

    #[structopt(long)]
    /// The name of the git tag.
    tag_name: Option<String>,

    #[structopt(long)]
    /// Pre-release identifier(s) to append to the next development version after release
    dev_version_ext: Option<String>,

    #[structopt(long)]
    /// Do not create dev version after release
    no_dev_version: bool,

    #[structopt(long)]
    /// Provide a set of features that need to be enabled
    features: Vec<String>,

    #[structopt(long)]
    /// Enable all features via `all-features`. Overrides `features`
    all_features: bool,
}

impl config::ConfigSource for ConfigArgs {
    fn sign_commit(&self) -> Option<bool> {
        self.sign.as_some(true)
    }

    fn upload_doc(&self) -> Option<bool> {
        self.upload_doc.as_some(true)
    }

    fn push_remote(&self) -> Option<&str> {
        self.push_remote.as_ref().map(|s| s.as_str())
    }

    fn doc_branch(&self) -> Option<&str> {
        self.doc_branch.as_ref().map(|s| s.as_str())
    }

    fn disable_publish(&self) -> Option<bool> {
        self.skip_publish.as_some(true)
    }

    fn disable_push(&self) -> Option<bool> {
        self.skip_push.as_some(true)
    }

    fn dev_version_ext(&self) -> Option<&str> {
        self.dev_version_ext.as_ref().map(|s| s.as_str())
    }

    fn no_dev_version(&self) -> Option<bool> {
        self.no_dev_version.as_some(true)
    }

    fn tag_prefix(&self) -> Option<&str> {
        self.tag_prefix.as_ref().map(|s| s.as_str())
    }

    fn tag_name(&self) -> Option<&str> {
        self.tag_name.as_ref().map(|s| s.as_str())
    }

    fn disable_tag(&self) -> Option<bool> {
        self.skip_tag.as_some(true)
    }

    fn enable_features(&self) -> Option<&[String]> {
        if !self.features.is_empty() {
            Some(self.features.as_slice())
        } else {
            None
        }
    }

    fn enable_all_features(&self) -> Option<bool> {
        self.all_features.as_some(true)
    }

    fn dependent_version(&self) -> Option<config::DependentVersion> {
        self.dependent_version
    }
}

#[derive(Debug, StructOpt)]
#[structopt(name = "cargo")]
enum Command {
    #[structopt(name = "release")]
    Release(ReleaseOpt),
}

pub fn get_logging(level: log::Level) -> env_logger::Builder {
    let mut builder = env_logger::Builder::new();

    builder.filter(None, level.to_level_filter());

    builder
        .default_format_timestamp(false)
        .default_format_module_path(false);

    builder
}

fn main() {
    let Command::Release(ref release_matches) = Command::from_args();

    let mut builder = get_logging(release_matches.logging.log_level());
    builder.init();

    match release_workspace(release_matches) {
        Ok(code) => exit(code),
        Err(e) => {
            log::warn!("Fatal: {}", e);
            exit(128);
        }
    }
}
