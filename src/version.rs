use clap::arg_enum;
use semver::{Identifier, Version};

use crate::error::FatalError;

static VERSION_ALPHA: &str = "alpha";
static VERSION_BETA: &str = "beta";
static VERSION_RC: &str = "rc";

arg_enum! {
    #[derive(Debug, Clone, Copy)]
    pub enum BumpLevel {
        Major,
        Minor,
        Patch,
        Rc,
        Beta,
        Alpha,
        Release,
    }
}

impl BumpLevel {
    pub fn bump_version(
        self,
        version: &mut Version,
        metadata: Option<&str>,
    ) -> Result<bool, FatalError> {
        let mut need_commit = false;
        match self {
            BumpLevel::Major => {
                version.increment_major();
                need_commit = true;
            }
            BumpLevel::Minor => {
                version.increment_minor();
                need_commit = true;
            }
            BumpLevel::Patch => {
                if !version.is_prerelease() {
                    version.increment_patch();
                } else {
                    version.pre.clear();
                }
                need_commit = true;
            }
            BumpLevel::Rc => {
                version.increment_rc()?;
                need_commit = true;
            }
            BumpLevel::Beta => {
                version.increment_beta()?;
                need_commit = true;
            }
            BumpLevel::Alpha => {
                version.increment_alpha()?;
                need_commit = true;
            }
            BumpLevel::Release => {
                if version.is_prerelease() {
                    version.pre.clear();
                    need_commit = true;
                }
            }
        };

        if let Some(metadata) = metadata {
            version.metadata(metadata)?;
        }

        Ok(need_commit)
    }
}

trait VersionExt {
    fn increment_alpha(&mut self) -> Result<(), FatalError>;
    fn increment_beta(&mut self) -> Result<(), FatalError>;
    fn increment_rc(&mut self) -> Result<(), FatalError>;
    fn prerelease_id_version(&self) -> Result<Option<(String, Option<u64>)>, FatalError>;
    fn metadata(&mut self, metadata: &str) -> Result<(), FatalError>;
}

impl VersionExt for Version {
    fn prerelease_id_version(&self) -> Result<Option<(String, Option<u64>)>, FatalError> {
        if !self.pre.is_empty() {
            let e = match self.pre[0] {
                Identifier::AlphaNumeric(ref s) => s.to_owned(),
                Identifier::Numeric(_) => {
                    return Err(FatalError::UnsupportedPrereleaseVersionScheme);
                }
            };
            let v = if let Some(v) = self.pre.get(1) {
                if let Identifier::Numeric(v) = *v {
                    Some(v)
                } else {
                    return Err(FatalError::UnsupportedPrereleaseVersionScheme);
                }
            } else {
                None
            };
            Ok(Some((e, v)))
        } else {
            Ok(None)
        }
    }

    fn increment_alpha(&mut self) -> Result<(), FatalError> {
        if let Some((pre_ext, pre_ext_ver)) = self.prerelease_id_version()? {
            if pre_ext == VERSION_BETA || pre_ext == VERSION_RC {
                Err(FatalError::InvalidReleaseLevel(VERSION_ALPHA.to_owned()))
            } else {
                let new_ext_ver = if pre_ext == VERSION_ALPHA {
                    pre_ext_ver.unwrap_or(0) + 1
                } else {
                    1
                };
                self.pre = vec![
                    Identifier::AlphaNumeric(VERSION_ALPHA.to_owned()),
                    Identifier::Numeric(new_ext_ver),
                ];
                Ok(())
            }
        } else {
            self.increment_patch();
            self.pre = vec![
                Identifier::AlphaNumeric(VERSION_ALPHA.to_owned()),
                Identifier::Numeric(1),
            ];
            Ok(())
        }
    }

    fn increment_beta(&mut self) -> Result<(), FatalError> {
        if let Some((pre_ext, pre_ext_ver)) = self.prerelease_id_version()? {
            if pre_ext == VERSION_RC {
                Err(FatalError::InvalidReleaseLevel(VERSION_BETA.to_owned()))
            } else {
                let new_ext_ver = if pre_ext == VERSION_BETA {
                    pre_ext_ver.unwrap_or(0) + 1
                } else {
                    1
                };
                self.pre = vec![
                    Identifier::AlphaNumeric(VERSION_BETA.to_owned()),
                    Identifier::Numeric(new_ext_ver),
                ];
                Ok(())
            }
        } else {
            self.increment_patch();
            self.pre = vec![
                Identifier::AlphaNumeric(VERSION_BETA.to_owned()),
                Identifier::Numeric(1),
            ];
            Ok(())
        }
    }

    fn increment_rc(&mut self) -> Result<(), FatalError> {
        if let Some((pre_ext, pre_ext_ver)) = self.prerelease_id_version()? {
            let new_ext_ver = if pre_ext == VERSION_RC {
                pre_ext_ver.unwrap_or(0) + 1
            } else {
                1
            };
            self.pre = vec![
                Identifier::AlphaNumeric(VERSION_RC.to_owned()),
                Identifier::Numeric(new_ext_ver),
            ];
            Ok(())
        } else {
            self.increment_patch();
            self.pre = vec![
                Identifier::AlphaNumeric(VERSION_RC.to_owned()),
                Identifier::Numeric(1),
            ];
            Ok(())
        }
    }

    fn metadata(&mut self, build: &str) -> Result<(), FatalError> {
        self.build = vec![Identifier::AlphaNumeric(build.to_owned())];
        Ok(())
    }
}

pub fn set_requirement(
    req: &semver::VersionReq,
    version: &semver::Version,
) -> Result<Option<String>, FatalError> {
    let req_text = req.to_string();
    let raw_req = semver_parser::range::parse(&req_text)
        .expect("semver to generate valid version requirements");
    if raw_req.predicates.is_empty() {
        // Empty matches everything, no-change.
        Ok(None)
    } else {
        let predicates: Result<Vec<_>, _> = raw_req
            .predicates
            .into_iter()
            .map(|p| set_predicate(p, version))
            .collect();
        let predicates = predicates?;
        let new_req = semver_parser::range::VersionReq { predicates };
        let new_req_text = display::DisplayVersionReq::new(&new_req).to_string();
        // Validate contract
        #[cfg(debug_assert)]
        {
            let req = semver::VersionReq::parse(new_req_text).unwrap();
            assert!(
                req.matches(version),
                "Invalid req created: {}",
                new_req_text
            )
        }
        if new_req_text == req_text {
            Ok(None)
        } else {
            Ok(Some(new_req_text))
        }
    }
}

fn set_predicate(
    mut pred: semver_parser::range::Predicate,
    version: &semver::Version,
) -> Result<semver_parser::range::Predicate, FatalError> {
    match pred.op {
        semver_parser::range::Op::Wildcard(semver_parser::range::WildcardVersion::Minor) => {
            pred.major = version.major;
            Ok(pred)
        }
        semver_parser::range::Op::Wildcard(semver_parser::range::WildcardVersion::Patch) => {
            pred.major = version.major;
            if pred.minor.is_some() {
                pred.minor = Some(version.minor);
            }
            Ok(pred)
        }
        semver_parser::range::Op::Ex => assign_partial_req(version, pred),
        semver_parser::range::Op::Gt
        | semver_parser::range::Op::GtEq
        | semver_parser::range::Op::Lt
        | semver_parser::range::Op::LtEq => {
            let user_pred = display::DisplayPredicate::new(&pred).to_string();
            Err(FatalError::UnsupportedVersionReq(user_pred))
        }
        semver_parser::range::Op::Tilde => assign_partial_req(version, pred),
        semver_parser::range::Op::Compatible => assign_partial_req(version, pred),
    }
}

fn assign_partial_req(
    version: &semver::Version,
    mut pred: semver_parser::range::Predicate,
) -> Result<semver_parser::range::Predicate, FatalError> {
    pred.major = version.major;
    if pred.minor.is_some() {
        pred.minor = Some(version.minor);
    }
    if pred.patch.is_some() {
        pred.patch = Some(version.patch);
    }
    pred.pre = version
        .pre
        .iter()
        .map(|i| match i {
            semver::Identifier::Numeric(n) => semver_parser::version::Identifier::Numeric(*n),
            semver::Identifier::AlphaNumeric(s) => {
                semver_parser::version::Identifier::AlphaNumeric(s.clone())
            }
        })
        .collect();
    Ok(pred)
}

// imo this should be moved to semver_parser, see
// https://github.com/steveklabnik/semver-parser/issues/45
mod display {
    use std::fmt;

    use semver_parser::range::Op::{Compatible, Ex, Gt, GtEq, Lt, LtEq, Tilde, Wildcard};
    use semver_parser::range::WildcardVersion::{Minor, Patch};

    pub(crate) struct DisplayVersionReq<'v>(&'v semver_parser::range::VersionReq);

    impl<'v> DisplayVersionReq<'v> {
        pub(crate) fn new(req: &'v semver_parser::range::VersionReq) -> Self {
            Self(req)
        }
    }

    impl<'v> fmt::Display for DisplayVersionReq<'v> {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            if self.0.predicates.is_empty() {
                write!(fmt, "*")?;
            } else {
                for (i, pred) in self.0.predicates.iter().enumerate() {
                    if i == 0 {
                        write!(fmt, "{}", DisplayPredicate(pred))?;
                    } else {
                        write!(fmt, ", {}", DisplayPredicate(pred))?;
                    }
                }
            }

            Ok(())
        }
    }

    pub(crate) struct DisplayPredicate<'v>(&'v semver_parser::range::Predicate);

    impl<'v> DisplayPredicate<'v> {
        pub(crate) fn new(pred: &'v semver_parser::range::Predicate) -> Self {
            Self(pred)
        }
    }

    impl<'v> fmt::Display for DisplayPredicate<'v> {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            match &self.0.op {
                Wildcard(Minor) => write!(fmt, "{}.*", self.0.major)?,
                Wildcard(Patch) => {
                    if let Some(minor) = self.0.minor {
                        write!(fmt, "{}.{}.*", self.0.major, minor)?
                    } else {
                        write!(fmt, "{}.*.*", self.0.major)?
                    }
                }
                _ => {
                    write!(fmt, "{}{}", DisplayOp(&self.0.op), self.0.major)?;

                    if let Some(v) = self.0.minor {
                        write!(fmt, ".{}", v)?;
                    }

                    if let Some(v) = self.0.patch {
                        write!(fmt, ".{}", v)?;
                    }

                    if !self.0.pre.is_empty() {
                        write!(fmt, "-")?;
                        for (i, x) in self.0.pre.iter().enumerate() {
                            if i != 0 {
                                write!(fmt, ".")?
                            }
                            write!(fmt, "{}", x)?;
                        }
                    }
                }
            }

            Ok(())
        }
    }

    pub(crate) struct DisplayOp<'v>(&'v semver_parser::range::Op);

    impl<'v> fmt::Display for DisplayOp<'v> {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.0 {
                Ex => write!(fmt, "= ")?,
                Gt => write!(fmt, "> ")?,
                GtEq => write!(fmt, ">= ")?,
                Lt => write!(fmt, "< ")?,
                LtEq => write!(fmt, "<= ")?,
                Tilde => write!(fmt, "~")?,
                Compatible => write!(fmt, "^")?,
                // gets handled specially in Predicate::fmt
                Wildcard(_) => write!(fmt, "")?,
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    mod increment {
        use super::*;

        #[test]
        fn alpha() {
            let mut v = Version::parse("1.0.0").unwrap();
            let _ = v.increment_alpha();
            assert_eq!(v, Version::parse("1.0.1-alpha.1").unwrap());

            let mut v2 = Version::parse("1.0.1-dev").unwrap();
            let _ = v2.increment_alpha();
            assert_eq!(v2, Version::parse("1.0.1-alpha.1").unwrap());

            let mut v3 = Version::parse("1.0.1-alpha.1").unwrap();
            let _ = v3.increment_alpha();
            assert_eq!(v3, Version::parse("1.0.1-alpha.2").unwrap());

            let mut v4 = Version::parse("1.0.1-beta.1").unwrap();
            assert!(v4.increment_alpha().is_err());

            let mut v5 = Version::parse("1.0.1-1").unwrap();
            assert!(v5.increment_alpha().is_err());
        }

        #[test]
        fn beta() {
            let mut v = Version::parse("1.0.0").unwrap();
            let _ = v.increment_beta();
            assert_eq!(v, Version::parse("1.0.1-beta.1").unwrap());

            let mut v2 = Version::parse("1.0.1-dev").unwrap();
            let _ = v2.increment_beta();
            assert_eq!(v2, Version::parse("1.0.1-beta.1").unwrap());

            let mut v2 = Version::parse("1.0.1-alpha.1").unwrap();
            let _ = v2.increment_beta();
            assert_eq!(v2, Version::parse("1.0.1-beta.1").unwrap());

            let mut v3 = Version::parse("1.0.1-beta.1").unwrap();
            let _ = v3.increment_beta();
            assert_eq!(v3, Version::parse("1.0.1-beta.2").unwrap());

            let mut v4 = Version::parse("1.0.1-rc.1").unwrap();
            assert!(v4.increment_beta().is_err());

            let mut v5 = Version::parse("1.0.1-1").unwrap();
            assert!(v5.increment_beta().is_err());
        }

        #[test]
        fn rc() {
            let mut v = Version::parse("1.0.0").unwrap();
            let _ = v.increment_rc();
            assert_eq!(v, Version::parse("1.0.1-rc.1").unwrap());

            let mut v2 = Version::parse("1.0.1-dev").unwrap();
            let _ = v2.increment_rc();
            assert_eq!(v2, Version::parse("1.0.1-rc.1").unwrap());

            let mut v3 = Version::parse("1.0.1-rc.1").unwrap();
            let _ = v3.increment_rc();
            assert_eq!(v3, Version::parse("1.0.1-rc.2").unwrap());
        }

        #[test]
        fn metadata() {
            let mut v = Version::parse("1.0.0").unwrap();
            let _ = v.metadata("git.123456");
            assert_eq!(v, Version::parse("1.0.0+git.123456").unwrap());
        }
    }

    mod set_requirement {
        use super::*;

        fn assert_req_bump<'a, O: Into<Option<&'a str>>>(version: &str, req: &str, expected: O) {
            let version = Version::parse(version).unwrap();
            let req = semver::VersionReq::parse(req).unwrap();
            let actual = set_requirement(&req, &version).unwrap();
            let expected = expected.into();
            assert_eq!(actual.as_deref(), expected);
        }

        #[test]
        fn wildcard_major() {
            assert_req_bump("1.0.0", "*", None);
        }

        #[test]
        fn wildcard_minor() {
            assert_req_bump("1.0.0", "1.*", None);
            assert_req_bump("1.1.0", "1.*", None);
            assert_req_bump("2.0.0", "1.*", "2.*");
        }

        #[test]
        fn wildcard_patch() {
            assert_req_bump("1.0.0", "1.0.*", None);
            assert_req_bump("1.1.0", "1.0.*", "1.1.*");
            assert_req_bump("1.1.1", "1.0.*", "1.1.*");
            assert_req_bump("2.0.0", "1.0.*", "2.0.*");
        }

        #[test]
        fn caret_major() {
            assert_req_bump("1.0.0", "1", None);
            assert_req_bump("1.0.0", "^1", None);

            assert_req_bump("1.1.0", "1", None);
            assert_req_bump("1.1.0", "^1", None);

            assert_req_bump("2.0.0", "1", "^2");
            assert_req_bump("2.0.0", "^1", "^2");
        }

        #[test]
        fn caret_minor() {
            assert_req_bump("1.0.0", "1.0", None);
            assert_req_bump("1.0.0", "^1.0", None);

            assert_req_bump("1.1.0", "1.0", "^1.1");
            assert_req_bump("1.1.0", "^1.0", "^1.1");

            assert_req_bump("1.1.1", "1.0", "^1.1");
            assert_req_bump("1.1.1", "^1.0", "^1.1");

            assert_req_bump("2.0.0", "1.0", "^2.0");
            assert_req_bump("2.0.0", "^1.0", "^2.0");
        }

        #[test]
        fn caret_patch() {
            assert_req_bump("1.0.0", "1.0.0", None);
            assert_req_bump("1.0.0", "^1.0.0", None);

            assert_req_bump("1.1.0", "1.0.0", "^1.1.0");
            assert_req_bump("1.1.0", "^1.0.0", "^1.1.0");

            assert_req_bump("1.1.1", "1.0.0", "^1.1.1");
            assert_req_bump("1.1.1", "^1.0.0", "^1.1.1");

            assert_req_bump("2.0.0", "1.0.0", "^2.0.0");
            assert_req_bump("2.0.0", "^1.0.0", "^2.0.0");
        }

        #[test]
        fn tilde_major() {
            assert_req_bump("1.0.0", "~1", None);
            assert_req_bump("1.1.0", "~1", None);
            assert_req_bump("2.0.0", "~1", "~2");
        }

        #[test]
        fn tilde_minor() {
            assert_req_bump("1.0.0", "~1.0", None);
            assert_req_bump("1.1.0", "~1.0", "~1.1");
            assert_req_bump("1.1.1", "~1.0", "~1.1");
            assert_req_bump("2.0.0", "~1.0", "~2.0");
        }

        #[test]
        fn tilde_patch() {
            assert_req_bump("1.0.0", "~1.0.0", None);
            assert_req_bump("1.1.0", "~1.0.0", "~1.1.0");
            assert_req_bump("1.1.1", "~1.0.0", "~1.1.1");
            assert_req_bump("2.0.0", "~1.0.0", "~2.0.0");
        }

        #[test]
        fn equal_major() {
            assert_req_bump("1.0.0", "= 1", None);
            assert_req_bump("1.1.0", "= 1", None);
            assert_req_bump("2.0.0", "= 1", "= 2");
        }

        #[test]
        fn equal_minor() {
            assert_req_bump("1.0.0", "= 1.0", None);
            assert_req_bump("1.1.0", "= 1.0", "= 1.1");
            assert_req_bump("1.1.1", "= 1.0", "= 1.1");
            assert_req_bump("2.0.0", "= 1.0", "= 2.0");
        }

        #[test]
        fn equal_patch() {
            assert_req_bump("1.0.0", "= 1.0.0", None);
            assert_req_bump("1.1.0", "= 1.0.0", "= 1.1.0");
            assert_req_bump("1.1.1", "= 1.0.0", "= 1.1.1");
            assert_req_bump("2.0.0", "= 1.0.0", "= 2.0.0");
        }
    }
}
