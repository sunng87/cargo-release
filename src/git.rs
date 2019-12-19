use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use bstr::ByteSlice;

use crate::cmd::call_on_path;
use crate::error::FatalError;

pub fn is_dirty(dir: &Path) -> Result<bool, FatalError> {
    log::debug!("Checking if path {:?} is dirty", dir);
    let output = Command::new("git")
        .arg("diff")
        .arg("HEAD")
        .arg("--exit-code")
        .arg("--name-only")
        .current_dir(dir)
        .output()
        .map_err(FatalError::from)?;
    let tracked_unclean = !output.status.success();

    log::info!("Unclean={}", tracked_unclean);

    let output = Command::new("git")
        .arg("ls-files")
        .arg("--exclude-standard")
        .arg("--others")
        .current_dir(dir)
        .output()
        .map_err(FatalError::from)?;
    let untracked_files = String::from_utf8_lossy(&output.stdout);
    let untracked = !untracked_files.as_ref().trim().is_empty();

    log::info!("Untracked Files={}", untracked_files);
    log::info!("Untracked={}", untracked);

    Ok(tracked_unclean || untracked)
}

pub fn changed_files(dir: &Path, tag: &str) -> Result<Option<Vec<PathBuf>>, FatalError> {
    let output = Command::new("git")
        .arg("diff")
        .arg(&format!("{}..HEAD", tag))
        .arg("--name-only")
        .arg("--exit-code")
        .arg(".")
        .current_dir(dir)
        .output()
        .map_err(FatalError::from)?;
    match output.status.code() {
        Some(0) => Ok(Some(Vec::new())),
        Some(1) => {
            let paths = output
                .stdout
                .lines()
                .map(|l| dir.join(l.to_path_lossy()))
                .collect();
            Ok(Some(paths))
        }
        _ => Ok(None), // For cases like non-existent tag
    }
}

pub fn commit_all(dir: &Path, msg: &str, sign: bool, dry_run: bool) -> Result<bool, FatalError> {
    call_on_path(
        vec!["git", "commit", if sign { "-S" } else { "" }, "-am", msg],
        dir,
        dry_run,
    )
}

pub fn tag(
    dir: &Path,
    name: &str,
    msg: &str,
    sign: bool,
    dry_run: bool,
) -> Result<bool, FatalError> {
    call_on_path(
        vec![
            "git",
            "tag",
            "-a",
            name,
            "-m",
            msg,
            if sign { "-s" } else { "" },
        ],
        dir,
        dry_run,
    )
}

pub fn push(dir: &Path, remote: &str, dry_run: bool) -> Result<bool, FatalError> {
    call_on_path(vec!["git", "push", remote], dir, dry_run)
}

pub fn push_tag(dir: &Path, remote: &str, tag: &str, dry_run: bool) -> Result<bool, FatalError> {
    call_on_path(vec!["git", "push", remote, tag], dir, dry_run)
}

pub fn top_level(dir: &Path) -> Result<PathBuf, FatalError> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(dir)
        .output()
        .map_err(FatalError::from)?;
    let path = std::str::from_utf8(&output.stdout)
        .map_err(FatalError::from)?
        .trim_end();
    Ok(Path::new(path).to_owned())
}

pub(crate) fn git_version() -> Result<(), FatalError> {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|_| ())
        .map_err(|_| FatalError::GitError)
}
