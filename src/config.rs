use std::io::prelude::*;
use std::io;
use std::io::BufReader;
use std::fs::{self, File};
use std::path::Path;

use toml::{Parser, Value, Table};
use semver::Version;
use regex::Regex;

use error::FatalError;

pub static SIGN_COMMIT: &'static str = "sign-commit";
pub static UPLOAD_DOC: &'static str = "upload-doc";
pub static UPLOAD_DOC_ONLY: &'static str = "upload-doc-only";
pub static PUSH_REMOTE: &'static str = "push-remote";
pub static DOC_BRANCH: &'static str = "doc-branch";
pub static DISABLE_PUSH: &'static str = "disable-push";
pub static DEV_VERSION_EXT: &'static str = "dev-version-ext";
pub static NO_DEV_VERSION: &'static str = "no-dev-version";
pub static PRE_RELEASE_COMMIT_MESSAGE: &'static str = "pre-release-commit-message";
pub static PRO_RELEASE_COMMIT_MESSAGE: &'static str = "pro-release-commit-message";
pub static TAG_MESSAGE: &'static str = "tag-message";
pub static DOC_COMMIT_MESSAGE: &'static str = "doc-commit-message";

fn load_from_file(path: &Path) -> io::Result<String> {
    let mut file = try!(File::open(path));
    let mut s = String::new();
    try!(file.read_to_string(&mut s));
    Ok(s)
}

fn save_to_file(path: &Path, content: &str) -> io::Result<()> {
    let mut file = try!(File::create(path));
    try!(file.write_all(&content.as_bytes()));
    Ok(())
}

pub fn parse_cargo_config() -> Result<Table, FatalError> {
    let cargo_file_path = Path::new("Cargo.toml");

    let cargo_file_content = try!(load_from_file(&cargo_file_path).map_err(FatalError::from));

    let mut parser = Parser::new(&cargo_file_content);

    match parser.parse() {
        Some(toml) => Ok(toml),
        None => Err(FatalError::InvalidCargoFileFormat),
    }
}

pub fn get_release_config<'a>(config: &'a Table, key: &str) -> Option<&'a Value> {
    config.get("package")
          .and_then(|f| f.as_table())
          .and_then(|f| f.get("metadata"))
          .and_then(|f| f.as_table())
          .and_then(|f| f.get("release"))
          .and_then(|f| f.as_table())
          .and_then(|f| f.get(key))
}

pub fn verify_release_config(config: &Table) -> Option<Vec<&str>> {
    let valid_keys = vec![SIGN_COMMIT,
                          UPLOAD_DOC,
                          UPLOAD_DOC_ONLY,
                          PUSH_REMOTE,
                          DOC_BRANCH,
                          DISABLE_PUSH,
                          DEV_VERSION_EXT,
                          PRE_RELEASE_COMMIT_MESSAGE,
                          PRO_RELEASE_COMMIT_MESSAGE,
                          TAG_MESSAGE,
                          DOC_COMMIT_MESSAGE];
    if let Some(ref r) = config.get("package")
                               .and_then(|f| f.as_table())
                               .and_then(|f| f.get("metadata"))
                               .and_then(|f| f.as_table())
                               .and_then(|f| f.get("release"))
                               .and_then(|f| f.as_table()) {
        let mut invalid_keys = Vec::new();
        for i in r.keys() {
            if !valid_keys.contains(&i.as_ref()) {
                invalid_keys.push(i.as_ref());
            }
        }
        if invalid_keys.is_empty() {
            None
        } else {
            Some(invalid_keys)
        }
    } else {
        None
    }
}

pub fn save_cargo_config(config: &Table) -> Result<(), FatalError> {
    let cargo_file_path = Path::new("Cargo.toml");

    let serialized_data = format!("{}", Value::Table(config.clone()));

    try!(save_to_file(&cargo_file_path, &serialized_data).map_err(FatalError::from));
    Ok(())
}

pub fn rewrite_cargo_version(version: &str) -> Result<(), FatalError> {
    let section_matcher = Regex::new("^\\[.+\\]").unwrap();
    {
        let file_in = try!(File::open("Cargo.toml").map_err(FatalError::from));
        let mut bufreader = BufReader::new(file_in);
        let mut line = String::new();

        let mut file_out = try!(File::create("Cargo.toml.work").map_err(FatalError::from));
        let mut in_package = false;

        loop {
            let b = try!(bufreader.read_line(&mut line).map_err(FatalError::from));
            if b <= 0 {
                break;
            }

            if section_matcher.is_match(&line) {
                in_package = line.trim() == "[package]";
            }

            if in_package && line.starts_with("version") {
                line = format!("version = \"{}\"\n", version);
            }

            try!(file_out.write_all(line.as_bytes()).map_err(FatalError::from));
            line.clear();
        }
    }
    try!(fs::rename("Cargo.toml.work", "Cargo.toml"));

    {
        let file_in = try!(File::open("Cargo.lock").map_err(FatalError::from));
        let mut bufreader = BufReader::new(file_in);
        let mut line = String::new();

        let mut file_out = try!(File::create("Cargo.lock.work").map_err(FatalError::from));
        let mut in_package = false;

        loop {
            let b = try!(bufreader.read_line(&mut line).map_err(FatalError::from));
            if b <= 0 {
                break;
            }

            if section_matcher.is_match(&line) {
                in_package = line.trim() == "[root]";
            }

            if in_package && line.starts_with("version") {
                line = format!("version = \"{}\"\n", version);
            }

            try!(file_out.write_all(line.as_bytes()).map_err(FatalError::from));
            line.clear();
        }
    }

    try!(fs::rename("Cargo.lock.work", "Cargo.lock"));
    Ok(())
}

pub fn parse_version(version: &str) -> Result<Version, FatalError> {
    Version::parse(version).map_err(|e| FatalError::from(e))
}

#[test]
fn test_release_config() {
    if let Ok(cargo_file) = parse_cargo_config() {
        assert!(get_release_config(&cargo_file, "sign-commit")
                    .and_then(|f| f.as_bool())
                    .unwrap_or(false));
    } else {
        panic!("paser cargo file failed");
    }

}
