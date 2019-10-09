use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use crate::cmd::call_on_path;
use crate::error::FatalError;

pub fn is_dirty(dir: &Path) -> Result<bool, FatalError> {
    let output = Command::new("git")
        .arg("diff")
        .arg("HEAD")
        .arg("--exit-code")
        .arg("--name-only")
        .current_dir(dir)
        .output()
        .map_err(FatalError::from)?;
    let tracked_unclean = !output.status.success();

    let output = Command::new("git")
        .arg("ls-files")
        .arg("--exclude-standard")
        .arg("--others")
        .current_dir(dir)
        .output()
        .map_err(FatalError::from)?;
    let untracked_files = String::from_utf8_lossy(&output.stdout);
    let untracked = !untracked_files.as_ref().trim().is_empty();

    Ok(tracked_unclean || untracked)
}

pub fn changed_from(dir: &Path, tag: &str) -> Result<Option<bool>, FatalError> {
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
        Some(0) => Ok(Some(false)),
        Some(1) => Ok(Some(true)),
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

pub fn check_remote(dir: &Path, remote: &str) -> bool {
    Command::new("git")
        .arg("remote")
        .arg("get-url")
        .arg(remote)
        .current_dir(dir)
        .output()
        .map(|r| r.status.success())
        .unwrap_or(false)
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

pub fn origin_url(dir: &Path) -> Result<String, FatalError> {
    let output = Command::new("git")
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .current_dir(dir)
        .output()
        .map_err(FatalError::from)?;
    String::from_utf8(output.stdout).map_err(FatalError::from)
}

pub fn init(dir: &Path, dry_run: bool) -> Result<bool, FatalError> {
    call_on_path(vec!["git", "init"], dir, dry_run)
}

pub fn add_all(dir: &Path, dry_run: bool) -> Result<bool, FatalError> {
    call_on_path(vec!["git", "add", "."], dir, dry_run)
}

pub fn force_push(
    dir: &Path,
    remote: &str,
    refspec: &str,
    dry_run: bool,
) -> Result<bool, FatalError> {
    call_on_path(vec!["git", "push", "-f", remote, refspec], dir, dry_run)
}

pub(crate) fn git_version() -> Result<(), FatalError> {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|_| ())
        .map_err(|_| FatalError::GitError)
}
