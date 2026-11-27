use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use client_ui::UpdateRelease;
use reqwest::{StatusCode, Url, blocking::Client, redirect};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use vpn_application::updates::{BuildIdentity, ReleaseChannel, ReleaseIdentity, ReleaseVersion};
use vpn_application::{APP_PREVIEW, APP_VERSION};

use super::setup::{MAX_SETUP_BYTES, valid_digest};

const REPOSITORY: &str = "https://github.com/KobaProduction/Blazer";
const API: &str = "https://api.github.com/repos/KobaProduction/Blazer/releases";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024;
const MAX_RELEASE_NOTES_BYTES: usize = 16 * 1024;

#[derive(Clone)]
pub(super) struct ReleaseAsset {
    pub release: UpdateRelease,
    pub name: String,
    pub download_url: String,
    pub size: u64,
    pub sha256: String,
}

pub(super) enum CheckResult {
    Current,
    Unpublished,
    Available(ReleaseAsset),
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    body: Option<String>,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    version: String,
    channel: String,
    sequence: u32,
    commit: String,
    setup: Setup,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Setup {
    name: String,
    size: u64,
    sha256: String,
}

struct Installed {
    version: ReleaseVersion,
    channel: ReleaseChannel,
    build: Option<BuildIdentity>,
}

pub(super) fn check() -> Result<CheckResult, String> {
    let installed = installed()?;
    if let Some(custom) = option_env!("BLAZER_RELEASE_FEED_URL") {
        return check_endpoint(custom, &installed);
    }
    check_channels(&installed, |endpoint| check_endpoint(endpoint, &installed))
}

fn check_channels(
    installed: &Installed,
    mut fetch: impl FnMut(&str) -> Result<CheckResult, String>,
) -> Result<CheckResult, String> {
    let stable = fetch(&format!("{API}/latest"))?;
    if installed.channel != ReleaseChannel::Preview || matches!(stable, CheckResult::Available(_)) {
        return Ok(stable);
    }
    fetch(&format!("{API}/tags/preview"))
}

fn installed() -> Result<Installed, String> {
    let build = option_env!("BLAZER_RELEASE_COMMIT")
        .zip(option_env!("BLAZER_RELEASE_SEQUENCE"))
        .and_then(|(commit, sequence)| BuildIdentity::parse(commit, sequence.parse().ok()?));
    if APP_PREVIEW && build.is_none() {
        return Err(
            "InstalledBuildIdentity: preview source identity is missing; reinstall using Setup"
                .into(),
        );
    }
    Ok(Installed {
        version: ReleaseVersion::parse(APP_VERSION).ok_or("InstalledVersion: invalid X.Y")?,
        channel: if APP_PREVIEW {
            ReleaseChannel::Preview
        } else {
            ReleaseChannel::Stable
        },
        build,
    })
}

fn check_endpoint(endpoint: &str, installed: &Installed) -> Result<CheckResult, String> {
    let url = feed_endpoint(endpoint).ok_or("UpdateFeed: invalid HTTPS endpoint")?;
    let response = client()
        .redirect(redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(network)?
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(network)?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(CheckResult::Unpublished);
    }
    let bytes = read_response(response, 512 * 1024)?;
    let release: Release =
        serde_json::from_slice(&bytes).map_err(|_| "UpdateFeed: invalid release JSON")?;
    let Some(metadata) = metadata_asset(&release, installed.channel)? else {
        return Ok(CheckResult::Unpublished);
    };
    let response = download_response(&metadata.browser_download_url)?;
    let bytes = read_response(response, MAX_MANIFEST_BYTES)?;
    verify_metadata(metadata, &bytes)?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|_| "UpdateManifest: invalid JSON")?;
    evaluate(&release, manifest, &metadata.name, installed)
}

fn metadata_asset(release: &Release, channel: ReleaseChannel) -> Result<Option<&Asset>, String> {
    if release.draft {
        return Err("UpdateFeed: draft release rejected".into());
    }
    let preview = release.tag_name == "preview" && release.prerelease;
    if preview && channel != ReleaseChannel::Preview {
        return Ok(None);
    }
    if !preview && (release.prerelease || !release.tag_name.starts_with('v')) {
        return Err("UpdateFeed: release channel mismatch".into());
    }
    let mut candidates: Vec<_> = release
        .assets
        .iter()
        .filter_map(|asset| metadata_order(&asset.name, release).map(|sequence| (sequence, asset)))
        .collect();
    candidates.sort_by_key(|candidate| candidate.0);
    let asset = candidates.last().map(|candidate| candidate.1);
    if let Some(asset) = asset {
        validate_asset(asset, &release.tag_name, MAX_MANIFEST_BYTES)?;
    }
    Ok(asset)
}

fn metadata_order(name: &str, release: &Release) -> Option<u32> {
    if release.tag_name != "preview" {
        let version = release.tag_name.strip_prefix('v')?;
        ReleaseVersion::parse(version)?;
        return (name == format!("Blazer-{version}-windows-x86_64-update.json")).then_some(1);
    }
    let identity = name
        .strip_prefix("Blazer-preview-")?
        .strip_suffix("-windows-x86_64-update.json")?;
    let (commit, sequence) = identity.split_once('-')?;
    if commit.len() != 12
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let parsed: u32 = sequence.parse().ok()?;
    (parsed > 0 && parsed <= i32::MAX as u32 && parsed.to_string() == sequence).then_some(parsed)
}

fn evaluate(
    release: &Release,
    manifest: Manifest,
    metadata_name: &str,
    installed: &Installed,
) -> Result<CheckResult, String> {
    let identity = manifest_identity(&manifest, release)?;
    let stem = if identity.channel == ReleaseChannel::Preview {
        format!(
            "Blazer-preview-{}-{}-windows-x86_64",
            &manifest.commit[..12],
            manifest.sequence
        )
    } else {
        format!("Blazer-{}-windows-x86_64", manifest.version)
    };
    if metadata_name != format!("{stem}-update.json")
        || manifest.setup.name != format!("{stem}-setup.exe")
    {
        return Err("UpdateManifest: build and asset names differ".into());
    }
    let assets: Vec<_> = release
        .assets
        .iter()
        .filter(|asset| asset.name == manifest.setup.name)
        .collect();
    if assets.len() != 1 {
        return Err("UpdateManifest: expected one matching setup asset".into());
    }
    let asset = assets[0];
    validate_asset(asset, &release.tag_name, MAX_SETUP_BYTES)?;
    if asset.size != manifest.setup.size
        || asset.digest.as_deref() != Some(&format!("sha256:{}", manifest.setup.sha256))
    {
        return Err("UpdateManifest: setup digest or size differs from GitHub".into());
    }
    if !identity.is_update_for(
        installed.version,
        installed.channel,
        installed.build.as_ref(),
    ) {
        return Ok(CheckResult::Current);
    }
    let display = if identity.channel == ReleaseChannel::Preview {
        format!("{} · preview {}", manifest.version, manifest.sequence)
    } else {
        manifest.version
    };
    Ok(CheckResult::Available(ReleaseAsset {
        release: UpdateRelease {
            version: display,
            release_url: format!("{REPOSITORY}/releases/tag/{}", release.tag_name),
            notes: release_notes(release.body.as_deref()),
        },
        name: asset.name.clone(),
        download_url: asset.browser_download_url.clone(),
        size: asset.size,
        sha256: manifest.setup.sha256,
    }))
}

fn release_notes(body: Option<&str>) -> Arc<str> {
    let text = body.unwrap_or_default();
    let limited = &text[..text.floor_char_boundary(MAX_RELEASE_NOTES_BYTES)];
    let cleaned: String = limited
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect();
    Arc::from(cleaned.trim())
}

fn manifest_identity(manifest: &Manifest, release: &Release) -> Result<ReleaseIdentity, String> {
    let channel = match manifest.channel.as_str() {
        "preview" if release.prerelease && release.tag_name == "preview" => ReleaseChannel::Preview,
        "stable" if !release.prerelease && release.tag_name == format!("v{}", manifest.version) => {
            ReleaseChannel::Stable
        }
        _ => return Err("UpdateManifest: channel mismatch".into()),
    };
    if manifest.schema != 1 {
        return Err("UpdateManifest: unsupported schema".into());
    }
    Ok(ReleaseIdentity {
        version: ReleaseVersion::parse(&manifest.version)
            .ok_or("UpdateManifest: invalid X.Y version")?,
        channel,
        build: BuildIdentity::parse(&manifest.commit, manifest.sequence)
            .ok_or("UpdateManifest: invalid build identity")?,
    })
}

fn verify_metadata(asset: &Asset, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 != asset.size
        || asset.digest.as_deref() != Some(&format!("sha256:{:x}", Sha256::digest(bytes)))
    {
        return Err("UpdateManifest: SHA-256 or size mismatch".into());
    }
    Ok(())
}

fn validate_asset(asset: &Asset, tag: &str, maximum: u64) -> Result<(), String> {
    let expected_url = format!("{REPOSITORY}/releases/download/{tag}/{}", asset.name);
    let digest = asset
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"));
    if asset.browser_download_url != expected_url
        || asset.size == 0
        || asset.size > maximum
        || !digest.is_some_and(valid_digest)
    {
        return Err("UpdateAsset: invalid repository URL, size or digest".into());
    }
    Ok(())
}

pub(super) fn download_response(url: &str) -> Result<reqwest::blocking::Response, String> {
    client()
        .redirect(redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < 5 && allowed_redirect(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("unexpected GitHub asset redirect")
            }
        }))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(network)?
        .get(url)
        .send()
        .map_err(network)?
        .error_for_status()
        .map_err(network)
}

fn read_response(response: reqwest::blocking::Response, limit: u64) -> Result<Vec<u8>, String> {
    if !response.status().is_success() {
        return Err(format!("UpdateHTTP: {}", response.status()));
    }
    let mut bytes = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("UpdateRead: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err("UpdateRead: response exceeds the size limit".into());
    }
    Ok(bytes)
}

fn client() -> reqwest::blocking::ClientBuilder {
    Client::builder()
        .https_only(true)
        .user_agent(format!("Blazer/{APP_VERSION}"))
}

fn feed_endpoint(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    (url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn allowed_redirect(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
        && url.username().is_empty()
        && url.password().is_none()
}

fn network(error: reqwest::Error) -> String {
    format!("UpdateNetwork: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(preview: bool) -> (Release, Manifest, String, Installed) {
        let channel = if preview { "preview" } else { "stable" };
        let stem = if preview {
            format!("Blazer-preview-{}-11-windows-x86_64", "b".repeat(12))
        } else {
            "Blazer-0.1-windows-x86_64".into()
        };
        let tag = if preview { "preview" } else { "v0.1" };
        let name = format!("{stem}-setup.exe");
        let digest = "a".repeat(64);
        let release = Release {
            tag_name: tag.into(),
            draft: false,
            prerelease: preview,
            body: preview.then(|| "## Changes\r\n\r\n- Improved updating".into()),
            assets: vec![Asset {
                name: name.clone(),
                size: 100,
                browser_download_url: format!("{REPOSITORY}/releases/download/{tag}/{name}"),
                digest: Some(format!("sha256:{digest}")),
            }],
        };
        let manifest = Manifest {
            schema: 1,
            version: "0.1".into(),
            channel: channel.into(),
            sequence: 11,
            commit: "b".repeat(40),
            setup: Setup {
                name,
                size: 100,
                sha256: digest,
            },
        };
        let installed = Installed {
            version: ReleaseVersion::parse("0.1").unwrap(),
            channel: ReleaseChannel::Preview,
            build: BuildIdentity::parse(&"a".repeat(40), 10),
        };
        (release, manifest, format!("{stem}-update.json"), installed)
    }

    #[test]
    fn preview_metadata_offers_same_xy_new_commit_and_stable_promotion() {
        let (release, manifest, name, installed) = fixture(true);
        let CheckResult::Available(preview) =
            evaluate(&release, manifest, &name, &installed).unwrap()
        else {
            panic!("expected preview update");
        };
        assert_eq!(&*preview.release.notes, "## Changes\n\n- Improved updating");
        let (release, manifest, name, installed) = fixture(false);
        let CheckResult::Available(stable) =
            evaluate(&release, manifest, &name, &installed).unwrap()
        else {
            panic!("expected stable update");
        };
        assert!(stable.release.notes.is_empty());
        let (release, manifest, name, mut installed) = fixture(true);
        installed.channel = ReleaseChannel::Stable;
        assert!(matches!(
            evaluate(&release, manifest, &name, &installed),
            Ok(CheckResult::Current)
        ));
    }

    #[test]
    fn release_body_is_optional_and_notes_are_bounded_at_a_utf8_boundary() {
        let input = r#"{"tag_name":"v0.1","draft":false,"prerelease":false,"assets":[]}"#;
        let release: Release = serde_json::from_str(input).unwrap();
        assert!(release_notes(release.body.as_deref()).is_empty());

        let prefix = "x".repeat(MAX_RELEASE_NOTES_BYTES - 2);
        let input = serde_json::json!({
            "tag_name": "v0.1",
            "draft": false,
            "prerelease": false,
            "assets": [],
            "body": format!("\0{prefix}Ж")
        });
        let release: Release = serde_json::from_value(input).unwrap();
        assert_eq!(&*release_notes(release.body.as_deref()), prefix);
    }

    #[test]
    fn manifest_cannot_substitute_setup_identity_repository_or_source() {
        let (mut release, manifest, name, installed) = fixture(true);
        release.assets[0].browser_download_url = "https://example.invalid/setup.exe".into();
        assert!(evaluate(&release, manifest, &name, &installed).is_err());
        let (release, mut manifest, name, installed) = fixture(true);
        manifest.setup.sha256 = "c".repeat(64);
        assert!(evaluate(&release, manifest, &name, &installed).is_err());
        let (release, mut manifest, name, installed) = fixture(true);
        manifest.commit = "B".repeat(40);
        assert!(evaluate(&release, manifest, &name, &installed).is_err());
        let (mut release, manifest, name, installed) = fixture(true);
        release.tag_name = "v0.1".into();
        assert!(evaluate(&release, manifest, &name, &installed).is_err());
        let (release, mut manifest, name, installed) = fixture(true);
        manifest.setup.name = "../setup.exe".into();
        assert!(evaluate(&release, manifest, &name, &installed).is_err());
        let (release, mut manifest, name, installed) = fixture(true);
        manifest.setup.size += 1;
        assert!(evaluate(&release, manifest, &name, &installed).is_err());
    }

    #[test]
    fn metadata_digest_and_network_destinations_fail_closed() {
        let asset = Asset {
            name: String::new(),
            browser_download_url: String::new(),
            size: 2,
            digest: Some(format!("sha256:{:x}", Sha256::digest(b"{}"))),
        };
        assert!(verify_metadata(&asset, b"{}").is_ok());
        assert!(verify_metadata(&asset, b"[]").is_err());
        assert!(
            feed_endpoint("https://api.github.com/repos/KobaProduction/Blazer/releases/latest")
                .is_some()
        );
        assert!(feed_endpoint("https://private@example.invalid/feed").is_none());
        assert!(!allowed_redirect(
            &Url::parse("https://github.com.example.invalid/setup.exe").unwrap()
        ));
    }

    #[test]
    fn preview_checks_its_tag_after_missing_stable_while_stable_never_does() {
        let (_, _, _, mut installed) = fixture(true);
        let mut calls = Vec::new();
        let result = check_channels(&installed, |endpoint| {
            calls.push(endpoint.to_owned());
            if endpoint.ends_with("/latest") {
                Ok(CheckResult::Unpublished)
            } else {
                Ok(CheckResult::Current)
            }
        });
        assert!(matches!(result, Ok(CheckResult::Current)));
        assert_eq!(
            calls,
            [format!("{API}/latest"), format!("{API}/tags/preview")]
        );
        installed.channel = ReleaseChannel::Stable;
        calls.clear();
        assert!(matches!(
            check_channels(&installed, |endpoint| {
                calls.push(endpoint.to_owned());
                Ok(CheckResult::Unpublished)
            }),
            Ok(CheckResult::Unpublished)
        ));
        assert_eq!(calls.len(), 1);
    }
}
