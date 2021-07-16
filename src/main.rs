use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::process::exit;

use boolinator::Boolinator;
use chrono::prelude::Local;
use semver::Identifier;
use structopt::StructOpt;

use crate::error::FatalError;
use crate::replace::{do_file_replacements, Template};

mod cargo;
mod cmd;
mod config;
mod error;
mod git;
mod replace;
mod shell;
mod version;

static NOW: once_cell::sync::Lazy<String> =
    once_cell::sync::Lazy::new(|| Local::now().format("%Y-%m-%d").to_string());

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

fn exclude_paths<'m>(
    ws_pkgs: &[&'m cargo_metadata::Package],
    pkg_meta: &'m cargo_metadata::Package,
) -> Vec<&'m Path> {
    let base_path = pkg_meta
        .manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("/"));
    ws_pkgs
        .iter()
        .filter_map(|p| {
            let cur_path = p.manifest_path.parent().unwrap_or_else(|| Path::new("/"));
            if cur_path != base_path && cur_path.starts_with(base_path) {
                Some(cur_path)
            } else {
                None
            }
        })
        .collect()
}

struct PackageRelease<'m> {
    meta: &'m cargo_metadata::Package,
    manifest_path: &'m Path,
    package_path: &'m Path,
    config: config::Config,

    crate_excludes: Vec<&'m Path>,
    custom_ignore: ignore::gitignore::Gitignore,

    prev_version: Version,
    prev_tag: String,
    version: Option<Version>,
    tag: Option<String>,
    post_version: Option<Version>,

    dependents: Vec<Dependency<'m>>,

    //dependent_version: config::DependentVersion,
    //dependents: Vec<&'m Path>,
    //failed_dependents: Vec<&'m Path>,
    features: Features,
}

#[derive(Debug)]
struct Version {
    version: semver::Version,
    version_string: String,
}

impl Version {
    fn is_prerelease(&self) -> bool {
        self.version.is_prerelease()
    }
}

struct Dependency<'m> {
    pkg: &'m cargo_metadata::Package,
    req: &'m semver::VersionReq,
}

impl<'m> PackageRelease<'m> {
    fn load(
        args: &ReleaseOpt,
        git_root: &Path,
        ws_meta: &'m cargo_metadata::Metadata,
        ws_pkgs: &[&'m cargo_metadata::Package],
        pkg_meta: &'m cargo_metadata::Package,
    ) -> Result<Option<Self>, error::FatalError> {
        let manifest_path = pkg_meta.manifest_path.as_path();
        let cwd = manifest_path.parent().unwrap_or_else(|| Path::new("."));

        let config = {
            let mut release_config = config::Config::default();

            if !args.isolated {
                let cfg = config::resolve_config(&ws_meta.workspace_root, manifest_path)?;
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
            let cargo_file = cargo::parse_cargo_config(manifest_path)?;
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
        if config.disable_release() {
            log::debug!("Disabled in config, skipping {}", manifest_path.display());
            return Ok(None);
        }

        let is_root = git_root == cwd;

        let prev_version = Version {
            version: pkg_meta.version.clone(),
            version_string: pkg_meta.version.to_string(),
        };

        let crate_excludes = exclude_paths(ws_pkgs, pkg_meta);
        let mut custom_ignore = ignore::gitignore::GitignoreBuilder::new(cwd);
        if let Some(globs) = config.exclude_paths() {
            for glob in globs {
                custom_ignore.add_line(None, glob)?;
            }
        }
        let custom_ignore = custom_ignore.build()?;

        let prev_tag = if let Some(prev_tag) = args.prev_tag_name.as_ref() {
            // Trust the user that the tag passed in is the latest tag for the workspace and that
            // they don't care about any changes from before this tag.
            prev_tag.to_owned()
        } else {
            let mut template = Template {
                prev_version: Some(&prev_version.version_string),
                version: Some(&prev_version.version_string),
                crate_name: Some(pkg_meta.name.as_str()),
                ..Default::default()
            };

            let tag_prefix = config.tag_prefix(is_root);
            let tag_prefix = template.render(tag_prefix);
            template.prefix = Some(&tag_prefix);
            template.render(config.tag_name())
        };

        let version = args
            .level_or_version
            .bump(&prev_version.version, args.metadata.as_deref())?;
        let is_pre_release = version
            .as_ref()
            .map(Version::is_prerelease)
            .unwrap_or(false);
        let dependents = if version.is_some() {
            find_dependents(ws_meta, pkg_meta)
                .map(|(pkg, dep)| Dependency { pkg, req: &dep.req })
                .collect()
        } else {
            Vec::new()
        };

        let base = version.as_ref().unwrap_or(&prev_version);

        let tag = if config.disable_tag() {
            None
        } else {
            let mut template = Template {
                prev_version: Some(&prev_version.version_string),
                version: Some(&base.version_string),
                crate_name: Some(pkg_meta.name.as_str()),
                ..Default::default()
            };

            let tag_prefix = config.tag_prefix(is_root);
            let tag_prefix = template.render(tag_prefix);
            template.prefix = Some(&tag_prefix);
            Some(template.render(config.tag_name()))
        };

        let post_version = if !is_pre_release && !config.no_dev_version() {
            let mut post = base.version.clone();
            post.increment_patch();
            post.pre.push(Identifier::AlphaNumeric(
                config.dev_version_ext().to_owned(),
            ));
            let post_string = post.to_string();

            Some(Version {
                version: post,
                version_string: post_string,
            })
        } else {
            None
        };

        let features = if config.enable_all_features() {
            Features::All
        } else {
            let features = config.enable_features();
            if features.is_empty() {
                Features::None
            } else {
                Features::Selective(features.to_owned())
            }
        };

        let pkg = PackageRelease {
            meta: pkg_meta,
            manifest_path,
            package_path: cwd,
            config,

            crate_excludes,
            custom_ignore,

            prev_version,
            prev_tag,
            version,
            tag,
            post_version,
            dependents,

            features,
        };
        Ok(Some(pkg))
    }
}

fn update_dependent_versions(
    pkg: &PackageRelease,
    version: &Version,
    dry_run: bool,
) -> Result<(), error::FatalError> {
    let new_version_string = version.version_string.as_str();
    let mut dependents_failed = false;
    for dep in pkg.dependents.iter() {
        match pkg.config.dependent_version() {
            config::DependentVersion::Ignore => (),
            config::DependentVersion::Warn => {
                if !dep.req.matches(&version.version) {
                    log::warn!(
                        "{}'s dependency on {} `{}` is incompatible with {}",
                        dep.pkg.name,
                        pkg.meta.name,
                        dep.req,
                        new_version_string
                    );
                }
            }
            config::DependentVersion::Error => {
                if !dep.req.matches(&version.version) {
                    log::warn!(
                        "{}'s dependency on {} `{}` is incompatible with {}",
                        dep.pkg.name,
                        pkg.meta.name,
                        dep.req,
                        new_version_string
                    );
                    dependents_failed = true;
                }
            }
            config::DependentVersion::Fix => {
                if !dep.req.matches(&version.version) {
                    let new_req = version::set_requirement(dep.req, &version.version)?;
                    if let Some(new_req) = new_req {
                        log::info!(
                            "Fixing {}'s dependency on {} to `{}` (from `{}`)",
                            dep.pkg.name,
                            pkg.meta.name,
                            new_req,
                            dep.req
                        );
                        if !dry_run {
                            cargo::set_dependency_version(
                                &dep.pkg.manifest_path,
                                &pkg.meta.name,
                                &new_req,
                            )?;
                        }
                    }
                }
            }
            config::DependentVersion::Upgrade => {
                let new_req = version::set_requirement(dep.req, &version.version)?;
                if let Some(new_req) = new_req {
                    log::info!(
                        "Upgrading {}'s dependency on {} to `{}` (from `{}`)",
                        dep.pkg.name,
                        pkg.meta.name,
                        new_req,
                        dep.req
                    );
                    if !dry_run {
                        cargo::set_dependency_version(
                            &dep.pkg.manifest_path,
                            &pkg.meta.name,
                            &new_req,
                        )?;
                    }
                }
            }
        }
    }
    if dependents_failed {
        Err(FatalError::DependencyVersionConflict)
    } else {
        Ok(())
    }
}

fn release_workspace(args: &ReleaseOpt) -> Result<i32, error::FatalError> {
    let ws_meta = args.manifest.metadata().exec().map_err(FatalError::from)?;
    let ws_config = {
        let mut release_config = config::Config::default();

        if !args.isolated {
            let cfg = config::resolve_workspace_config(&ws_meta.workspace_root)?;
            release_config.update(&cfg);
        }

        if let Some(custom_config_path) = args.custom_config.as_ref() {
            // when calling with -c option
            let cfg =
                config::resolve_custom_config(Path::new(custom_config_path))?.unwrap_or_default();
            release_config.update(&cfg);
        }

        release_config.update(&args.config);
        release_config
    };

    let pkg_ids = sort_workspace(&ws_meta);

    let (selected_pkgs, excluded_pkgs) = args.workspace.partition_packages(&ws_meta);
    if selected_pkgs.is_empty() {
        log::info!("No packages selected.");
        return Ok(0);
    }
    let mut all_pkgs = selected_pkgs.clone();
    all_pkgs.extend(excluded_pkgs);
    let all_pkgs = all_pkgs;

    let root = git::top_level(&ws_meta.workspace_root)?;
    let pkg_releases: Result<HashMap<_, _>, _> = selected_pkgs
        .iter()
        .filter_map(|p| PackageRelease::load(args, &root, &ws_meta, &all_pkgs, p).transpose())
        .map(|p| p.map(|p| (&p.meta.id, p)))
        .collect();
    let pkg_releases = pkg_releases?;
    let pkg_releases: Vec<_> = pkg_ids
        .into_iter()
        .filter_map(|id| pkg_releases.get(id))
        .collect();

    release_packages(args, &ws_meta, &ws_config, pkg_releases.as_slice())
}

fn sort_workspace(ws_meta: &cargo_metadata::Metadata) -> Vec<&cargo_metadata::PackageId> {
    let members: HashSet<_> = ws_meta.workspace_members.iter().collect();
    let dep_tree: HashMap<_, _> = ws_meta
        .resolve
        .as_ref()
        .expect("cargo-metadata resolved deps")
        .nodes
        .iter()
        .filter_map(|n| {
            if members.contains(&n.id) {
                Some((&n.id, &n.dependencies))
            } else {
                None
            }
        })
        .collect();

    let mut sorted = Vec::new();
    let mut processed = HashSet::new();
    for pkg_id in ws_meta.workspace_members.iter() {
        sort_workspace_inner(ws_meta, pkg_id, &dep_tree, &mut processed, &mut sorted);
    }

    sorted
}

fn sort_workspace_inner<'m>(
    ws_meta: &'m cargo_metadata::Metadata,
    pkg_id: &'m cargo_metadata::PackageId,
    dep_tree: &HashMap<&'m cargo_metadata::PackageId, &'m std::vec::Vec<cargo_metadata::PackageId>>,
    processed: &mut HashSet<&'m cargo_metadata::PackageId>,
    sorted: &mut Vec<&'m cargo_metadata::PackageId>,
) {
    if !processed.insert(pkg_id) {
        return;
    }

    for dep_id in dep_tree[pkg_id]
        .iter()
        .filter(|dep_id| dep_tree.contains_key(dep_id))
    {
        sort_workspace_inner(ws_meta, dep_id, dep_tree, processed, sorted);
    }

    sorted.push(pkg_id);
}

fn release_packages<'m>(
    args: &ReleaseOpt,
    ws_meta: &cargo_metadata::Metadata,
    ws_config: &config::Config,
    pkgs: &'m [&'m PackageRelease<'m>],
) -> Result<i32, error::FatalError> {
    let dry_run = args.dry_run;

    // STEP 0: Help the user make the right decisions.
    git::git_version()?;
    let mut dirty = false;
    if ws_config.consolidate_commits() {
        if git::is_dirty(&ws_meta.workspace_root)? {
            log::warn!("Uncommitted changes detected, please commit before release.");
            dirty = true;
        }
    } else {
        for pkg in pkgs {
            let cwd = pkg.package_path;
            if git::is_dirty(cwd)? {
                let crate_name = pkg.meta.name.as_str();
                log::warn!(
                    "Uncommitted changes detected for {}, please commit before release.",
                    crate_name
                );
                dirty = true;
            }
        }
    }
    if dirty {
        if !args.dry_run {
            return Ok(101);
        }
    }

    let lock_path = ws_meta.workspace_root.join("Cargo.lock");
    for pkg in pkgs {
        if let Some(version) = pkg.version.as_ref() {
            let cwd = pkg.package_path;
            let crate_name = pkg.meta.name.as_str();
            let prev_tag_name = &pkg.prev_tag;
            if let Some(changed) = git::changed_files(cwd, prev_tag_name)? {
                let mut changed: Vec<_> = changed
                    .into_iter()
                    .filter(|p| {
                        let file_in_subcrate =
                            pkg.crate_excludes.iter().any(|base| p.starts_with(base));
                        if file_in_subcrate {
                            return false;
                        }
                        let glob_status = pkg.custom_ignore.matched_path_or_any_parents(p, false);
                        if glob_status.is_ignore() {
                            log::trace!(
                                "{}: ignoring {} due to {:?}",
                                crate_name,
                                p.display(),
                                glob_status
                            );
                            return false;
                        }
                        true
                    })
                    .collect();
                if let Some(lock_index) = changed.iter().enumerate().find_map(|(idx, path)| {
                    if path == &lock_path {
                        Some(idx)
                    } else {
                        None
                    }
                }) {
                    log::debug!("Lock file changed since {} but ignored since it could be as simple as a pre-release version bump.", prev_tag_name);
                    let _ = changed.swap_remove(lock_index);
                }
                if changed.is_empty() {
                    log::warn!(
                        "Updating {} to {} despite no changes made since tag {}",
                        crate_name,
                        version.version_string,
                        prev_tag_name
                    );
                } else {
                    log::debug!(
                        "Files changed in {} since {}: {:#?}",
                        crate_name,
                        prev_tag_name,
                        changed
                    );
                }
            } else {
                log::debug!(
                    "Cannot detect changes for {} because tag {} is missing. Try setting `--prev-tag-name <TAG>`.",
                    crate_name,
                    prev_tag_name
                );
            }
        }
    }

    let git_remote = ws_config.push_remote();
    let branch = git::current_branch(&ws_meta.workspace_root)?;
    if branch == "HEAD" {
        log::warn!("Releasing from a detached HEAD");
    }
    git::fetch(&ws_meta.workspace_root, git_remote, &branch)?;
    if git::is_behind_remote(&ws_meta.workspace_root, git_remote, &branch)? {
        log::warn!("{} is behind {}/{}", branch, git_remote, branch);
    }

    // STEP 1: Release Confirmation
    if !dry_run && !args.no_confirm {
        let prompt = if pkgs.len() == 1 {
            let pkg = pkgs[0];
            let crate_name = pkg.meta.name.as_str();
            let base = pkg.version.as_ref().unwrap_or(&pkg.prev_version);
            format!("Release {} {}?", crate_name, base.version_string)
        } else {
            let mut buffer: Vec<u8> = vec![];
            writeln!(&mut buffer, "Release").unwrap();
            for pkg in pkgs {
                let crate_name = pkg.meta.name.as_str();
                let base = pkg.version.as_ref().unwrap_or(&pkg.prev_version);
                writeln!(&mut buffer, "  {} {}", crate_name, base.version_string).unwrap();
            }
            write!(&mut buffer, "?").unwrap();
            String::from_utf8(buffer).expect("Only valid UTF-8 has been written")
        };

        let confirmed = shell::confirm(&prompt);
        if !confirmed {
            return Ok(0);
        }
    }

    // STEP 2: update current version, save and commit
    let mut shared_commit = false;
    for pkg in pkgs {
        let dry_run = args.dry_run;
        let cwd = pkg.package_path;
        let crate_name = pkg.meta.name.as_str();

        if let Some(version) = pkg.version.as_ref() {
            let new_version_string = version.version_string.as_str();
            log::info!("Update {} to version {}", crate_name, new_version_string);
            if !dry_run {
                cargo::set_package_version(pkg.manifest_path, new_version_string)?;
            }
            update_dependent_versions(pkg, version, dry_run)?;
            if dry_run {
                log::debug!("Updating lock file");
            } else {
                cargo::update_lock(pkg.manifest_path)?;
            }

            if !pkg.config.pre_release_replacements().is_empty() {
                // try replacing text in configured files
                let template = Template {
                    prev_version: Some(&pkg.prev_version.version_string),
                    version: Some(new_version_string),
                    crate_name: Some(crate_name),
                    date: Some(NOW.as_str()),
                    tag_name: pkg.tag.as_deref(),
                    ..Default::default()
                };
                let prerelease = !version.version.pre.is_empty();
                do_file_replacements(
                    pkg.config.pre_release_replacements(),
                    &template,
                    cwd,
                    prerelease,
                    dry_run,
                )?;
            }

            // pre-release hook
            if let Some(pre_rel_hook) = pkg.config.pre_release_hook() {
                let pre_rel_hook = pre_rel_hook.args();
                log::debug!("Calling pre-release hook: {:?}", pre_rel_hook);
                let envs = maplit::btreemap! {
                    OsStr::new("PREV_VERSION") => pkg.prev_version.version_string.as_ref(),
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

            if ws_config.consolidate_commits() {
                shared_commit = true;
            } else {
                let template = Template {
                    prev_version: Some(&pkg.prev_version.version_string),
                    version: Some(new_version_string),
                    crate_name: Some(crate_name),
                    date: Some(NOW.as_str()),
                    ..Default::default()
                };
                let commit_msg = template.render(pkg.config.pre_release_commit_message());
                let sign = pkg.config.sign_commit();
                if !git::commit_all(cwd, &commit_msg, sign, dry_run)? {
                    // commit failed, abort release
                    return Ok(102);
                }
            }
        }
    }
    if shared_commit {
        let shared_commit_msg = {
            let template = Template {
                date: Some(NOW.as_str()),
                ..Default::default()
            };
            template.render(ws_config.pre_release_commit_message())
        };
        if !git::commit_all(
            &ws_meta.workspace_root,
            &shared_commit_msg,
            ws_config.sign_commit(),
            dry_run,
        )? {
            // commit failed, abort release
            return Ok(102);
        }
    }

    // STEP 3: cargo publish
    for pkg in pkgs {
        if !pkg.config.disable_publish() {
            let crate_name = pkg.meta.name.as_str();
            let base = pkg.version.as_ref().unwrap_or(&pkg.prev_version);

            log::info!("Running cargo publish on {}", crate_name);
            // feature list to release
            let features = &pkg.features;
            if !cargo::publish(
                dry_run,
                pkg.manifest_path,
                features,
                pkg.config.registry(),
                args.config.token.as_ref().map(AsRef::as_ref),
            )? {
                return Ok(103);
            }
            let timeout = std::time::Duration::from_secs(300);

            if pkg.config.registry().is_none() {
                cargo::wait_for_publish(crate_name, &base.version_string, timeout, dry_run)?;
                // HACK: Even once the index is updated, there seems to be another step before the publish is fully ready.
                // We don't have a way yet to check for that, so waiting for now in hopes everything is ready
                if !dry_run {
                    let publish_grace_sleep = std::env::var("PUBLISH_GRACE_SLEEP")
                        .unwrap_or_else(|_| String::from("5"))
                        .parse()
                        .unwrap_or(5);
                    log::info!(
                        "Waiting an additional {} seconds for crates.io to update its indices...",
                        publish_grace_sleep
                    );
                    std::thread::sleep(std::time::Duration::from_secs(publish_grace_sleep));
                }
            } else {
                log::debug!("Not waiting for publish because the registry is not crates.io and doesn't get updated automatically");
            }
        }
    }

    // STEP 5: Tag
    for pkg in pkgs {
        if let Some(tag_name) = pkg.tag.as_ref() {
            let sign = pkg.config.sign_commit() || pkg.config.sign_tag();

            // FIXME: remove when the meaning of sign_commit is changed
            if !pkg.config.sign_tag() && pkg.config.sign_commit() {
                log::warn!("In next minor release, `sign-commit` will only be used to control git commit signing. Use option `sign-tag` for tag signing.");
            }

            let cwd = pkg.package_path;
            let crate_name = pkg.meta.name.as_str();

            let base = pkg.version.as_ref().unwrap_or(&pkg.prev_version);
            let template = Template {
                prev_version: Some(&pkg.prev_version.version_string),
                version: Some(&base.version_string),
                crate_name: Some(crate_name),
                tag_name: Some(tag_name),
                date: Some(NOW.as_str()),
                ..Default::default()
            };
            let tag_message = template.render(pkg.config.tag_message());

            log::debug!("Creating git tag {}", tag_name);
            if !git::tag(cwd, tag_name, &tag_message, sign, dry_run)? {
                // tag failed, abort release
                return Ok(104);
            }
        }
    }

    // STEP 6: bump version
    let mut shared_commit = false;
    for pkg in pkgs {
        if let Some(version) = pkg.post_version.as_ref() {
            let cwd = pkg.package_path;
            let crate_name = pkg.meta.name.as_str();

            let updated_version_string = version.version_string.as_ref();
            log::info!(
                "Starting {}'s next development iteration {}",
                crate_name,
                updated_version_string,
            );
            update_dependent_versions(pkg, version, dry_run)?;
            if !dry_run {
                cargo::set_package_version(pkg.manifest_path, updated_version_string)?;
                cargo::update_lock(pkg.manifest_path)?;
            }
            let base = pkg.version.as_ref().unwrap_or(&pkg.prev_version);
            let template = Template {
                prev_version: Some(&pkg.prev_version.version_string),
                version: Some(&base.version_string),
                crate_name: Some(crate_name),
                date: Some(NOW.as_str()),
                tag_name: pkg.tag.as_deref(),
                next_version: Some(updated_version_string),
                ..Default::default()
            };
            if !pkg.config.post_release_replacements().is_empty() {
                // try replacing text in configured files
                do_file_replacements(
                    pkg.config.post_release_replacements(),
                    &template,
                    cwd,
                    false, // post-release replacements should always be applied
                    dry_run,
                )?;
            }
            let commit_msg = template.render(pkg.config.post_release_commit_message());

            if ws_config.consolidate_commits() {
                shared_commit = true;
            } else {
                let sign = pkg.config.sign_commit();
                if !git::commit_all(cwd, &commit_msg, sign, dry_run)? {
                    return Ok(105);
                }
            }
        }
    }
    if shared_commit {
        let shared_commit_msg = {
            let template = Template {
                date: Some(NOW.as_str()),
                ..Default::default()
            };
            template.render(ws_config.post_release_commit_message())
        };
        if !git::commit_all(
            &ws_meta.workspace_root,
            &shared_commit_msg,
            ws_config.sign_commit(),
            dry_run,
        )? {
            // commit failed, abort release
            return Ok(102);
        }
    }

    // STEP 7: git push
    if !ws_config.disable_push() {
        let shared_push = ws_config.consolidate_pushes();

        for pkg in pkgs {
            if pkg.config.disable_push() {
                continue;
            }

            let cwd = pkg.package_path;
            if let Some(tag_name) = pkg.tag.as_ref() {
                log::info!("Pushing {} to {}", tag_name, git_remote);
                if !git::push_tag(cwd, git_remote, tag_name, dry_run)? {
                    return Ok(106);
                }
            }

            if !shared_push {
                log::info!("Pushing HEAD to {}", git_remote);
                if !git::push(cwd, git_remote, pkg.config.push_options(), dry_run)? {
                    return Ok(106);
                }
            }
        }

        if shared_push {
            log::info!("Pushing HEAD to {}", git_remote);
            if !git::push(
                &ws_meta.workspace_root,
                git_remote,
                ws_config.push_options(),
                dry_run,
            )? {
                return Ok(106);
            }
        }
    }

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

    /// Release level or version: bumping specified version field or remove prerelease extensions by default. Possible level value: major, minor, patch, release, rc, beta, alpha or any valid semver version that is greater than current version
    #[structopt(default_value)]
    level_or_version: TargetVersion,

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

    #[structopt(long)]
    /// The name of tag for the previous release.
    prev_tag_name: Option<String>,

    #[structopt(flatten)]
    logging: Verbosity,
}

#[derive(StructOpt, Debug, Clone)]
pub struct Verbosity {
    /// Pass many times for less log output
    #[structopt(long, short = "q", parse(from_occurrences))]
    quiet: i8,

    /// Pass many times for more log output
    ///
    /// By default, it'll report info. Passing `-v` one time also prints
    /// warnings, `-vv` enables info logging, `-vvv` debug, and `-vvvv` trace.
    #[structopt(long, short = "v", parse(from_occurrences))]
    verbose: i8,
}

impl Verbosity {
    /// Get the log level.
    pub fn log_level(&self) -> log::Level {
        let verbosity = 2 - self.quiet + self.verbose;

        match verbosity {
            std::i8::MIN..=0 => log::Level::Error,
            1 => log::Level::Warn,
            2 => log::Level::Info,
            3 => log::Level::Debug,
            4..=std::i8::MAX => log::Level::Trace,
        }
    }
}

#[derive(Debug, StructOpt)]
struct ConfigArgs {
    #[structopt(long)]
    /// Sign both git commit and tag,
    sign: bool,

    #[structopt(long)]
    /// Sign git commit
    sign_commit: bool,

    #[structopt(long)]
    /// Sign git tag
    sign_tag: bool,

    #[structopt(long)]
    /// Git remote to push
    push_remote: Option<String>,

    #[structopt(long)]
    /// Cargo registry to upload to
    registry: Option<String>,

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

    #[structopt(long)]
    /// Token to use when uploading
    token: Option<String>,
}

impl config::ConfigSource for ConfigArgs {
    fn sign_commit(&self) -> Option<bool> {
        self.sign
            .as_some(true)
            .or_else(|| self.sign_commit.as_some(true))
    }

    fn sign_tag(&self) -> Option<bool> {
        self.sign
            .as_some(true)
            .or_else(|| self.sign_tag.as_some(true))
    }

    fn push_remote(&self) -> Option<&str> {
        self.push_remote.as_deref()
    }

    fn registry(&self) -> Option<&str> {
        self.registry.as_deref()
    }

    fn disable_publish(&self) -> Option<bool> {
        self.skip_publish.as_some(true)
    }

    fn disable_push(&self) -> Option<bool> {
        self.skip_push.as_some(true)
    }

    fn dev_version_ext(&self) -> Option<&str> {
        self.dev_version_ext.as_deref()
    }

    fn no_dev_version(&self) -> Option<bool> {
        self.no_dev_version.as_some(true)
    }

    fn tag_prefix(&self) -> Option<&str> {
        self.tag_prefix.as_deref()
    }

    fn tag_name(&self) -> Option<&str> {
        self.tag_name.as_deref()
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
#[structopt(
    setting = structopt::clap::AppSettings::UnifiedHelpMessage,
    setting = structopt::clap::AppSettings::DeriveDisplayOrder,
    setting = structopt::clap::AppSettings::DontCollapseArgsInUsage
)]
enum Command {
    #[structopt(name = "release")]
    #[structopt(
        setting = structopt::clap::AppSettings::UnifiedHelpMessage,
        setting = structopt::clap::AppSettings::DeriveDisplayOrder,
        setting = structopt::clap::AppSettings::DontCollapseArgsInUsage
    )]
    Release(ReleaseOpt),
}

#[derive(Clone, Debug)]
enum TargetVersion {
    Relative(version::BumpLevel),
    Absolute(semver::Version),
}

impl TargetVersion {
    fn bump(
        &self,
        current: &semver::Version,
        metadata: Option<&str>,
    ) -> Result<Option<Version>, FatalError> {
        match self {
            TargetVersion::Relative(bump_level) => {
                let mut potential_version = current.to_owned();
                if bump_level.bump_version(&mut potential_version, metadata)? {
                    let version = potential_version;
                    let version_string = version.to_string();
                    Ok(Some(Version {
                        version,
                        version_string,
                    }))
                } else {
                    Ok(None)
                }
            }
            TargetVersion::Absolute(version) => {
                if current < version {
                    Ok(Some(Version {
                        version: version.to_owned(),
                        version_string: version.to_string(),
                    }))
                } else if current == version {
                    Ok(None)
                } else {
                    return Err(error::FatalError::UnsupportedVersionReq(
                        "Cannot release version smaller than current one".to_owned(),
                    ));
                }
            }
        }
    }
}

impl Default for TargetVersion {
    fn default() -> Self {
        TargetVersion::Relative(version::BumpLevel::Release)
    }
}

impl std::fmt::Display for TargetVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            TargetVersion::Relative(bump_level) => {
                write!(f, "{}", bump_level)
            }
            TargetVersion::Absolute(version) => {
                write!(f, "{}", version)
            }
        }
    }
}

impl std::str::FromStr for TargetVersion {
    type Err = FatalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(bump_level) = version::BumpLevel::from_str(s) {
            Ok(TargetVersion::Relative(bump_level))
        } else {
            Ok(TargetVersion::Absolute(semver::Version::parse(s)?))
        }
    }
}

pub fn get_logging(level: log::Level) -> env_logger::Builder {
    let mut builder = env_logger::Builder::new();

    builder.filter(None, level.to_level_filter());

    builder.format_timestamp_secs().format_module_path(false);

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
