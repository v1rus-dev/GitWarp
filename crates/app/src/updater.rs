//! In-app updates, step one: pick the release file that fits this installation,
//! download it into a cache directory and verify it. Nothing is applied here.
//!
//! Trust chain: the release carries `SHA256SUMS.txt` and a minisign signature of
//! it (`SHA256SUMS.txt.minisig`). The signature is checked against the public key
//! embedded in the app, then the downloaded file must match its line in the sums.
//! There is no way to skip either check.

use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use forge::api::{ApiError, Client, ReleaseAsset};
use sha2::{Digest, Sha256};

use crate::update_check::{parse_version, Version};

/// Minisign public key (the base64 line of `minisign.pub`) the release checksums
/// must be signed with.
// TODO(U4): PLACEHOLDER. Replace with the real release public key when CI
// signing lands; until then no signature verifies, so nothing is ever staged.
const UPDATE_PUBLIC_KEY: &str = "RWQ/3TShtH3rLS1xFkK3JrBEAWJ8qfusMvXIUw+xkDzE2wIlhxeSGkiB";

const SUMS_NAME: &str = "SHA256SUMS.txt";
const SUMS_SIG_NAME: &str = "SHA256SUMS.txt.minisig";
/// Asset downloads must come from here (GitHub then redirects to its CDN).
const TRUSTED_URL_PREFIX: &str = "https://github.com/";

/// How this copy of GitWarp was installed; decides which release file fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    WindowsInstaller,
    WindowsPortable,
    MacApp,
    MacHomebrew,
    LinuxAppImage,
    LinuxPackage,
    Unknown,
}

/// Detects the install kind of the running executable.
pub fn detect_install_kind() -> InstallKind {
    let Ok(exe) = std::env::current_exe() else {
        return InstallKind::Unknown;
    };
    let appimage = std::env::var("APPIMAGE").ok();
    let local_app_data = std::env::var("LOCALAPPDATA").ok();
    let has_uninstaller = exe
        .parent()
        .is_some_and(|dir| dir.join("unins000.exe").exists());
    let exe_text = exe.to_string_lossy();
    let mut kind = classify(
        &exe_text,
        appimage.as_deref(),
        local_app_data.as_deref(),
        has_uninstaller,
    );
    // A Homebrew `.app` may be a symlink into the Caskroom.
    if kind == InstallKind::MacApp {
        if let Ok(resolved) = fs::canonicalize(&exe) {
            if resolved.to_string_lossy().contains("/Caskroom/") {
                kind = InstallKind::MacHomebrew;
            }
        }
    }
    // The release files are built for one architecture per platform.
    let arch = std::env::consts::ARCH;
    let arch_ok = match kind {
        InstallKind::WindowsInstaller
        | InstallKind::WindowsPortable
        | InstallKind::LinuxAppImage => arch == "x86_64",
        InstallKind::MacApp => arch == "aarch64",
        _ => true,
    };
    if arch_ok {
        kind
    } else {
        InstallKind::Unknown
    }
}

/// Pure classification, on path text so it can be tested on any OS.
///
/// * `exe`: the running executable's path.
/// * `appimage`: the `APPIMAGE` environment variable (Linux).
/// * `local_app_data`: `%LOCALAPPDATA%` (Windows).
/// * `has_uninstaller`: `unins000.exe` sits next to the executable (the Inno
///   Setup uninstaller; the reliable sign of an installed copy).
fn classify(
    exe: &str,
    appimage: Option<&str>,
    local_app_data: Option<&str>,
    has_uninstaller: bool,
) -> InstallKind {
    if appimage.is_some_and(|p| !p.is_empty()) {
        return InstallKind::LinuxAppImage;
    }
    let path = exe.replace('\\', "/");
    if path.contains(".app/Contents/MacOS") {
        return if path.contains("/Caskroom/") {
            InstallKind::MacHomebrew
        } else {
            InstallKind::MacApp
        };
    }
    let lower = path.to_ascii_lowercase();
    let windows_path = lower.as_bytes().get(1) == Some(&b':') || lower.starts_with("//");
    if windows_path {
        let programs = local_app_data.filter(|p| !p.is_empty()).map(|p| {
            format!("{}/programs/", p.replace('\\', "/").trim_end_matches('/')).to_ascii_lowercase()
        });
        let installed = has_uninstaller
            || lower.contains("/program files")
            || programs.is_some_and(|prefix| lower.starts_with(&prefix));
        return if installed {
            InstallKind::WindowsInstaller
        } else {
            InstallKind::WindowsPortable
        };
    }
    if path.starts_with("/usr/bin/") {
        return InstallKind::LinuxPackage;
    }
    InstallKind::Unknown
}

/// The release file for `kind` and `version` (without a leading `v`); `None`
/// for kinds that stay notification-only.
pub fn asset_for<'a>(
    kind: InstallKind,
    version: &str,
    assets: &'a [ReleaseAsset],
) -> Option<&'a ReleaseAsset> {
    let name = match kind {
        InstallKind::WindowsInstaller => format!("gitwarp-{version}-windows-x86_64-setup.exe"),
        InstallKind::WindowsPortable => format!("gitwarp-{version}-windows-x86_64.zip"),
        InstallKind::MacApp => format!("gitwarp-{version}-macos-aarch64.zip"),
        InstallKind::LinuxAppImage => format!("gitwarp-{version}-x86_64.AppImage"),
        InstallKind::MacHomebrew | InstallKind::LinuxPackage | InstallKind::Unknown => {
            return None;
        }
    };
    assets.iter().find(|asset| asset.name == name)
}

/// Why a downloaded file was not accepted.
#[derive(Debug)]
pub enum VerifyError {
    /// The embedded public key does not decode.
    BadKey,
    /// The signature file does not decode or does not match the sums text.
    BadSignature,
    /// The sums list has no line for the file.
    MissingEntry,
    /// The file's SHA-256 differs from the signed one.
    HashMismatch,
    /// The file could not be read.
    Io(String),
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifyError::BadKey => f.write_str("the embedded update key is invalid"),
            VerifyError::BadSignature => f.write_str("the checksum signature is invalid"),
            VerifyError::MissingEntry => f.write_str("the file is not listed in the checksums"),
            VerifyError::HashMismatch => f.write_str("the file does not match its checksum"),
            VerifyError::Io(why) => write!(f, "cannot read the file: {why}"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Verifies `file` (published as `name`) against the signed checksums.
// `stage` goes through `verify_with_key` with its key parameter; this is the
// fixed-key entry point for the apply steps (U2/U3).
#[allow(dead_code)]
pub fn verify(file: &Path, name: &str, sums_text: &str, sums_sig: &str) -> Result<(), VerifyError> {
    verify_with_key(UPDATE_PUBLIC_KEY, file, name, sums_text, sums_sig)
}

/// [`verify`] with an explicit minisign public key (base64 line).
fn verify_with_key(
    key: &str,
    file: &Path,
    name: &str,
    sums_text: &str,
    sums_sig: &str,
) -> Result<(), VerifyError> {
    let key = minisign_verify::PublicKey::from_base64(key).map_err(|_| VerifyError::BadKey)?;
    let signature =
        minisign_verify::Signature::decode(sums_sig).map_err(|_| VerifyError::BadSignature)?;
    key.verify(sums_text.as_bytes(), &signature, false)
        .map_err(|_| VerifyError::BadSignature)?;

    let expected = sums_text
        .lines()
        .filter_map(|line| {
            // `sha256sum` output: `<hex>  <name>` (text) or `<hex> *<name>` (binary).
            let (hash, rest) = line.split_once(' ')?;
            let listed = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('*'))?;
            (listed == name).then_some(hash)
        })
        .collect::<Vec<_>>();
    // Zero lines: not signed for. Several: ambiguous, never guess.
    let [expected] = expected.as_slice() else {
        return Err(VerifyError::MissingEntry);
    };

    let actual = sha256_hex(file).map_err(|e| VerifyError::Io(e.to_string()))?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(VerifyError::HashMismatch)
    }
}

fn sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// A downloaded and verified release file, ready to be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // read by the apply steps (U2/U3)
pub struct StagedUpdate {
    pub version: String,
    pub kind: InstallKind,
    pub path: PathBuf,
}

/// Why staging failed.
#[derive(Debug)]
pub enum UpdateError {
    /// The tag is not SemVer.
    BadTag,
    /// The release is not newer than the running version.
    NotNewer,
    /// No file in the release fits this installation.
    NoAsset,
    /// The release has no (signed) checksums.
    NoChecksums,
    /// An asset URL is not a github.com one.
    UntrustedUrl,
    NoCacheDir,
    Download(ApiError),
    Verify(VerifyError),
    Io(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateError::BadTag => f.write_str("the release tag is not a version"),
            UpdateError::NotNewer => f.write_str("the release is not newer than this version"),
            UpdateError::NoAsset => f.write_str("the release has no file for this installation"),
            UpdateError::NoChecksums => f.write_str("the release has no signed checksums"),
            UpdateError::UntrustedUrl => f.write_str("the release file is not hosted on GitHub"),
            UpdateError::NoCacheDir => f.write_str("there is no cache directory"),
            UpdateError::Download(e) => write!(f, "download failed: {e}"),
            UpdateError::Verify(e) => write!(f, "verification failed: {e}"),
            UpdateError::Io(why) => write!(f, "file error: {why}"),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<VerifyError> for UpdateError {
    fn from(e: VerifyError) -> Self {
        UpdateError::Verify(e)
    }
}

fn io_error(e: std::io::Error) -> UpdateError {
    UpdateError::Io(e.to_string())
}

/// Where staged updates live: `<config dir>/updates`.
fn cache_root() -> Option<PathBuf> {
    crate::settings::config_dir().map(|dir| dir.join("updates"))
}

/// Downloads and verifies the update in release `release_tag` that fits `kind`
/// into `<config dir>/updates/<version>/`. A cached file that still verifies is
/// reused; older cached versions are deleted afterwards. Blocking: run it on a
/// background thread. `progress(done, total)` reports the main file only.
pub fn stage(
    release_tag: &str,
    assets: &[ReleaseAsset],
    kind: InstallKind,
    progress: impl FnMut(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<StagedUpdate, UpdateError> {
    let running = parse_version(env!("CARGO_PKG_VERSION")).ok_or(UpdateError::BadTag)?;
    let root = cache_root().ok_or(UpdateError::NoCacheDir)?;
    stage_in(
        &Client::new(),
        &root,
        &running,
        UPDATE_PUBLIC_KEY,
        release_tag,
        assets,
        kind,
        progress,
        cancel,
    )
}

#[allow(clippy::too_many_arguments)]
fn stage_in(
    client: &Client,
    root: &Path,
    running: &Version,
    key: &str,
    release_tag: &str,
    assets: &[ReleaseAsset],
    kind: InstallKind,
    progress: impl FnMut(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<StagedUpdate, UpdateError> {
    let version = parse_version(release_tag).ok_or(UpdateError::BadTag)?;
    // Downgrade protection: an equal or older version is never staged.
    if version <= *running {
        return Err(UpdateError::NotNewer);
    }
    let version_text = version.to_string();
    let asset = asset_for(kind, &version_text, assets).ok_or(UpdateError::NoAsset)?;
    let find = |name: &str| assets.iter().find(|a| a.name == name);
    let (sums, sig) = find(SUMS_NAME)
        .zip(find(SUMS_SIG_NAME))
        .ok_or(UpdateError::NoChecksums)?;
    for url in [
        &asset.browser_download_url,
        &sums.browser_download_url,
        &sig.browser_download_url,
    ] {
        if !url.starts_with(TRUSTED_URL_PREFIX) {
            return Err(UpdateError::UntrustedUrl);
        }
    }

    let dir = root.join(&version_text);
    let fetch = |url: &str, name: &str, on_progress: &mut dyn FnMut(u64, Option<u64>)| {
        let dest = dir.join(name);
        let _ = fs::remove_file(&dest);
        client
            .download(url, &dest, on_progress, cancel)
            .map_err(UpdateError::Download)?;
        Ok::<PathBuf, UpdateError>(dest)
    };
    let sums_path = fetch(&sums.browser_download_url, SUMS_NAME, &mut |_, _| {})?;
    let sig_path = fetch(&sig.browser_download_url, SUMS_SIG_NAME, &mut |_, _| {})?;
    let sums_text = fs::read_to_string(&sums_path).map_err(io_error)?;
    let sig_text = fs::read_to_string(&sig_path).map_err(io_error)?;

    let path = dir.join(&asset.name);
    let cached_ok =
        path.is_file() && verify_with_key(key, &path, &asset.name, &sums_text, &sig_text).is_ok();
    if !cached_ok {
        let mut progress = progress;
        fetch(&asset.browser_download_url, &asset.name, &mut progress)?;
        if let Err(e) = verify_with_key(key, &path, &asset.name, &sums_text, &sig_text) {
            // Never leave an unverified file behind.
            let _ = fs::remove_file(&path);
            return Err(e.into());
        }
    }

    remove_other_versions(root, &version_text);
    Ok(StagedUpdate {
        version: version_text,
        kind,
        path,
    })
}

/// Deletes every cached version directory except `keep`; best effort.
fn remove_other_versions(root: &Path, keep: &str) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() != keep && entry.path().is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test key pairs and signatures made with a reference Ed25519 /
    // minisign-format generator; the secret keys are not kept.
    const KEY_A: &str = "RWQBAgMEBQYHCAOhB7/zzhC+HXDdGOdLwJln5NYwm6UNXx3chmQSVTG4";
    const SUMS: &str = "239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5  gitwarp-1.2.3-x86_64.AppImage\nd9298a10d1b0735837dc4bd85dac641b0f3cef27a47e5d53a54f2f3f5b2fcffa  other.bin\n";
    const NAME: &str = "gitwarp-1.2.3-x86_64.AppImage";
    /// `SUMS` signed by key A.
    const SIG_A: &str = "untrusted comment: signature from minisign secret key\nRUQBAgMEBQYHCG3fbBFRnjf08tjXAVDhR0ZhYdhPMiKclzoNOBK4wtZYI1aJdPIWWoDRDkMykykdp5RviwKHEU7ub5g7FermTQc=\ntrusted comment: timestamp:1\tfile:SHA256SUMS.txt\nqZa3lTipNLDerUMFjDVrvwJoJyW6HV/n/VNR+P09zIRcw6cPl+s1USDj4hKVMyO+5oJw2Q57g8UC3ee3wl/UCg==\n";
    /// `SUMS` signed by key B.
    const SIG_B: &str = "untrusted comment: signature from minisign secret key\nRUQyMzQ1Njc4OXnccB1Xaw+lAUZYT4jkF2qukEVDf9XZey29CPeqzHKFrmfG/3dFD9RQXL3qKSfBqkF1Y8wmaiurmHUrfLj/ZgU=\ntrusted comment: timestamp:1\tfile:SHA256SUMS.txt\nNU25Y33ad9NuH9G4HMRG6y2lMyvDyR731qLwLBET0fAXAe+7qtY7e0Mt8V//wSTxbzN6cph5QlorSWEOifYbBQ==\n";

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gitwarp-updater-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            size: 1,
            browser_download_url: format!("https://github.com/o/r/releases/download/v1/{name}"),
        }
    }

    #[test]
    fn classify_table() {
        use InstallKind::*;
        let lad = Some(r"C:\Users\me\AppData\Local");
        type Case<'a> = (&'a str, Option<&'a str>, Option<&'a str>, bool, InstallKind);
        let cases: &[Case] = &[
            (
                r"C:\Users\me\AppData\Local\Programs\GitWarp\gitwarp.exe",
                None,
                lad,
                false,
                WindowsInstaller,
            ),
            (
                r"c:\users\me\appdata\local\programs\gitwarp\gitwarp.exe",
                None,
                lad,
                false,
                WindowsInstaller,
            ),
            (
                r"C:\Program Files\GitWarp\gitwarp.exe",
                None,
                lad,
                false,
                WindowsInstaller,
            ),
            (
                r"D:\Tools\GitWarp\gitwarp.exe",
                None,
                lad,
                true,
                WindowsInstaller,
            ),
            (
                r"D:\Tools\GitWarp\gitwarp.exe",
                None,
                lad,
                false,
                WindowsPortable,
            ),
            (
                r"C:\Users\me\Downloads\gitwarp.exe",
                None,
                None,
                false,
                WindowsPortable,
            ),
            (
                "/Applications/GitWarp.app/Contents/MacOS/gitwarp",
                None,
                None,
                false,
                MacApp,
            ),
            (
                "/opt/homebrew/Caskroom/gitwarp/1.0/GitWarp.app/Contents/MacOS/gitwarp",
                None,
                None,
                false,
                MacHomebrew,
            ),
            (
                "/tmp/.mount_x/usr/bin/gitwarp",
                Some("/home/me/GitWarp.AppImage"),
                None,
                false,
                LinuxAppImage,
            ),
            ("/usr/bin/gitwarp", None, None, false, LinuxPackage),
            ("/usr/bin/gitwarp", Some(""), None, false, LinuxPackage),
            ("/home/me/gitwarp/gitwarp", None, None, false, Unknown),
            ("/usr/local/bin/gitwarp", None, None, false, Unknown),
        ];
        for (exe, appimage, lad, uninstaller, expected) in cases {
            assert_eq!(
                classify(exe, *appimage, *lad, *uninstaller),
                *expected,
                "{exe}"
            );
        }
    }

    #[test]
    fn asset_for_picks_the_file_of_each_kind() {
        use InstallKind::*;
        let assets: Vec<_> = [
            "gitwarp-1.2.3-windows-x86_64-setup.exe",
            "gitwarp-1.2.3-windows-x86_64.zip",
            "gitwarp-1.2.3-macos-aarch64.zip",
            "gitwarp-1.2.3-macos-aarch64.dmg",
            "gitwarp-1.2.3-x86_64.AppImage",
            "gitwarp_1.2.3_amd64.deb",
            "SHA256SUMS.txt",
        ]
        .map(asset)
        .into();
        let pick = |kind| asset_for(kind, "1.2.3", &assets).map(|a| a.name.as_str());
        assert_eq!(
            pick(WindowsInstaller),
            Some("gitwarp-1.2.3-windows-x86_64-setup.exe")
        );
        assert_eq!(
            pick(WindowsPortable),
            Some("gitwarp-1.2.3-windows-x86_64.zip")
        );
        assert_eq!(pick(MacApp), Some("gitwarp-1.2.3-macos-aarch64.zip"));
        assert_eq!(pick(LinuxAppImage), Some("gitwarp-1.2.3-x86_64.AppImage"));
        assert_eq!(pick(MacHomebrew), None);
        assert_eq!(pick(LinuxPackage), None);
        assert_eq!(pick(Unknown), None);
        // Another version's files are not picked.
        assert!(asset_for(WindowsInstaller, "1.2.4", &assets).is_none());
        assert!(asset_for(WindowsInstaller, "1.2.3", &[]).is_none());
    }

    fn payload_file(dir: &Path, content: &[u8]) -> PathBuf {
        let path = dir.join(NAME);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn verify_accepts_a_signed_matching_file() {
        let dir = temp_dir("verify-ok");
        let file = payload_file(&dir, b"payload");
        verify_with_key(KEY_A, &file, NAME, SUMS, SIG_A).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_rejects_a_wrong_file_hash() {
        let dir = temp_dir("verify-hash");
        let file = payload_file(&dir, b"tampered");
        assert!(matches!(
            verify_with_key(KEY_A, &file, NAME, SUMS, SIG_A),
            Err(VerifyError::HashMismatch)
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_rejects_a_wrong_signature() {
        let dir = temp_dir("verify-sig");
        let file = payload_file(&dir, b"payload");
        // Signed by another key.
        assert!(matches!(
            verify_with_key(KEY_A, &file, NAME, SUMS, SIG_B),
            Err(VerifyError::BadSignature)
        ));
        // Sums edited after signing (hash swapped to match a tampered file).
        let forged = SUMS.replace("239f59ed", "239f59ee");
        assert!(matches!(
            verify_with_key(KEY_A, &file, NAME, &forged, SIG_A),
            Err(VerifyError::BadSignature)
        ));
        // Garbage signature and garbage key.
        assert!(matches!(
            verify_with_key(KEY_A, &file, NAME, SUMS, "nonsense"),
            Err(VerifyError::BadSignature)
        ));
        assert!(matches!(
            verify_with_key("not a key", &file, NAME, SUMS, SIG_A),
            Err(VerifyError::BadKey)
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_rejects_a_name_missing_from_the_sums() {
        let dir = temp_dir("verify-name");
        let file = payload_file(&dir, b"payload");
        assert!(matches!(
            verify_with_key(KEY_A, &file, "gitwarp-9.9.9-x86_64.AppImage", SUMS, SIG_A),
            Err(VerifyError::MissingEntry)
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_embedded_placeholder_key_verifies_nothing() {
        let dir = temp_dir("verify-placeholder");
        let file = payload_file(&dir, b"payload");
        assert!(verify(&file, NAME, SUMS, SIG_A).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn equal_older_and_invalid_versions_are_never_staged() {
        let running = parse_version("1.2.3").unwrap();
        let assets = vec![asset("gitwarp-1.2.3-x86_64.AppImage")];
        let root = temp_dir("downgrade");
        let cancel = AtomicBool::new(false);
        for (tag, expect_not_newer) in [
            ("v1.2.3", true),
            ("v1.2.2", true),
            ("v1.2.3-rc.1", true),
            ("nightly", false),
        ] {
            let result = stage_in(
                &Client::new(),
                &root,
                &running,
                KEY_A,
                tag,
                &assets,
                InstallKind::LinuxAppImage,
                |_, _| {},
                &cancel,
            );
            match result {
                Err(UpdateError::NotNewer) => assert!(expect_not_newer, "{tag}"),
                Err(UpdateError::BadTag) => assert!(!expect_not_newer, "{tag}"),
                other => panic!("{tag}: {other:?}"),
            }
        }
        // A newer release without the checksum files is refused before any download.
        let result = stage_in(
            &Client::new(),
            &root,
            &running,
            KEY_A,
            "v1.2.4",
            &[asset("gitwarp-1.2.4-x86_64.AppImage")],
            InstallKind::LinuxAppImage,
            |_, _| {},
            &cancel,
        );
        assert!(
            matches!(result, Err(UpdateError::NoChecksums)),
            "{result:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn older_cached_versions_are_removed() {
        let root = temp_dir("cleanup");
        for v in ["1.0.0", "1.1.0", "1.2.0"] {
            fs::create_dir_all(root.join(v)).unwrap();
            fs::write(root.join(v).join("f"), b"x").unwrap();
        }
        remove_other_versions(&root, "1.2.0");
        assert!(root.join("1.2.0").join("f").exists());
        assert!(!root.join("1.0.0").exists());
        assert!(!root.join("1.1.0").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
