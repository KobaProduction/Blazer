//! Version and immutable build identity policy for desktop releases.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReleaseVersion([u32; 2]);

impl ReleaseVersion {
    pub fn parse(version: &str) -> Option<Self> {
        let mut parts = version.split('.');
        let version = Self([component(parts.next()?)?, component(parts.next()?)?]);
        parts.next().is_none().then_some(version)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseChannel {
    Stable,
    Preview,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildIdentity {
    pub commit: String,
    pub sequence: u32,
}

impl BuildIdentity {
    pub fn parse(commit: &str, sequence: u32) -> Option<Self> {
        (commit.len() == 40
            && commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && (1..=i32::MAX as u32).contains(&sequence))
        .then(|| Self {
            commit: commit.to_owned(),
            sequence,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseIdentity {
    pub version: ReleaseVersion,
    pub channel: ReleaseChannel,
    pub build: BuildIdentity,
}

impl ReleaseIdentity {
    pub fn is_update_for(
        &self,
        installed: ReleaseVersion,
        channel: ReleaseChannel,
        build: Option<&BuildIdentity>,
    ) -> bool {
        match self.channel {
            ReleaseChannel::Stable => {
                self.version > installed
                    || (channel == ReleaseChannel::Preview && self.version == installed)
            }
            ReleaseChannel::Preview => {
                channel == ReleaseChannel::Preview
                    && self.version >= installed
                    && build.is_some_and(|installed| {
                        self.build.sequence > installed.sequence
                            && self.build.commit != installed.commit
                    })
            }
        }
    }
}

fn component(value: &str) -> Option<u32> {
    let parsed: u32 = value.parse().ok()?;
    (parsed <= u16::MAX as u32 && parsed.to_string() == value).then_some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_stable_versions_numerically_and_rejects_other_shapes() {
        assert!(ReleaseVersion::parse("1.10").unwrap() > ReleaseVersion::parse("1.2").unwrap());
        for version in ["v1.2", "1", "1.2.3", "1.2-rc.1", "1.02", "1.+2", "65536.0"] {
            assert_eq!(ReleaseVersion::parse(version), None);
        }
    }

    #[test]
    fn preview_updates_use_commit_and_order_without_entering_stable_channel() {
        let current = BuildIdentity::parse(&"a".repeat(40), 10).unwrap();
        let mut next = ReleaseIdentity {
            version: ReleaseVersion::parse("0.1").unwrap(),
            channel: ReleaseChannel::Preview,
            build: BuildIdentity::parse(&"b".repeat(40), 11).unwrap(),
        };
        assert!(next.is_update_for(next.version, ReleaseChannel::Preview, Some(&current)));
        assert!(!next.is_update_for(next.version, ReleaseChannel::Stable, Some(&current)));
        next.build.commit = current.commit.clone();
        assert!(!next.is_update_for(next.version, ReleaseChannel::Preview, Some(&current)));
        next.build.commit = "b".repeat(40);
        next.build.sequence = 9;
        assert!(!next.is_update_for(next.version, ReleaseChannel::Preview, Some(&current)));
        next.channel = ReleaseChannel::Stable;
        assert!(next.is_update_for(next.version, ReleaseChannel::Preview, Some(&current)));
        assert!(!next.is_update_for(next.version, ReleaseChannel::Stable, Some(&current)));
    }

    #[test]
    fn build_identity_rejects_unknown_or_unbounded_source_identity() {
        assert!(BuildIdentity::parse(&"a".repeat(40), 1).is_some());
        for (commit, sequence) in [
            ("A".repeat(40), 1),
            ("a".repeat(39), 1),
            ("a".repeat(40), 0),
            ("a".repeat(40), u32::MAX),
        ] {
            assert!(BuildIdentity::parse(&commit, sequence).is_none());
        }
    }
}
