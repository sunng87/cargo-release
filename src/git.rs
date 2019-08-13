use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use crate::cmd::call_on_path;
use crate::error::FatalError;

pub fn status(dir: &Path) -> Result<bool, FatalError> {
    let output = Command::new("git")
        .arg("diff")
        .arg("HEAD")
        .arg("--exit-code")
        .current_dir(dir)
        .output()
        .map_err(FatalError::from)?;
    Ok(output.status.success())
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
    edit: bool,
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
            if edit { "-e" } else { "" },
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
