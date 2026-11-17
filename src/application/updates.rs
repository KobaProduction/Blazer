//! Version ordering for the stable desktop release channel.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReleaseVersion([u32; 2]);

impl ReleaseVersion {
    pub fn parse(version: &str) -> Option<Self> {
        let mut parts = version.split('.');
        let version = Self([component(parts.next()?)?, component(parts.next()?)?]);
        parts.next().is_none().then_some(version)
    }

    pub fn can_install_over(self, installed: Self, preview: bool) -> bool {
        self > installed || (preview && self == installed)
    }
}

fn component(value: &str) -> Option<u32> {
    let parsed: u32 = value.parse().ok()?;
    (parsed.to_string() == value).then_some(parsed)
}

#[cfg(test)]
mod tests {
    use super::ReleaseVersion;

    #[test]
    fn compares_stable_versions_numerically_and_rejects_other_shapes() {
        assert!(ReleaseVersion::parse("1.10").unwrap() > ReleaseVersion::parse("1.2").unwrap());
        for version in ["v1.2", "1", "1.2.3", "1.2-rc.1", "1.02", "1.+2"] {
            assert_eq!(ReleaseVersion::parse(version), None);
        }
        let version = ReleaseVersion::parse("0.1").unwrap();
        assert!(version.can_install_over(version, true));
        assert!(!version.can_install_over(version, false));
    }
}
