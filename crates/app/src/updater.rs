//! In-app updates: pick the release file that fits this installation, download
//! it into a cache directory, verify it, and apply it (Windows installer and
//! Linux AppImage).
//!
//! Trust chain: the release carries `SHA256SUMS.txt` and a minisign signature of
//! it (`SHA256SUMS.txt.minisig`). The signature is checked against the public key
//! embedded in the app, then the downloaded file must match its line in the sums.
//! There is no way to skip either check.

use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

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
    let name = asset_name(kind, version)?;
    assets.iter().find(|asset| asset.name == name)
}

/// The release file name for `kind` and `version`; `None` for notification-only kinds.
fn asset_name(kind: InstallKind, version: &str) -> Option<String> {
    Some(match kind {
        InstallKind::WindowsInstaller => format!("gitwarp-{version}-windows-x86_64-setup.exe"),
        InstallKind::WindowsPortable => format!("gitwarp-{version}-windows-x86_64.zip"),
        InstallKind::MacApp => format!("gitwarp-{version}-macos-aarch64.zip"),
        InstallKind::LinuxAppImage => format!("gitwarp-{version}-x86_64.AppImage"),
        InstallKind::MacHomebrew | InstallKind::LinuxPackage | InstallKind::Unknown => {
            return None;
        }
    })
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
// `stage` and the apply code go through `verify_with_key` with a key
// parameter; this fixed-key entry point is only exercised by tests.
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
    /// This installation cannot apply the update (or it was already applied).
    NotApplicable,
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
            UpdateError::NotApplicable => f.write_str("this installation cannot apply the update"),
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

// ---------------------------------------------------------------------------
// Applying a staged update (Windows installer, Linux AppImage).
// ---------------------------------------------------------------------------

/// Set once an update was handed over to be installed, so that "Restart to
/// update" followed by the quit hook never applies twice.
static APPLIED: AtomicBool = AtomicBool::new(false);

/// Whether this installation can apply a staged update by itself: a Windows
/// installer copy whose folder the current user may write (an all-users
/// install would raise UAC behind the app's back), or an AppImage whose folder
/// is writable. Everything else stays notification-only.
pub fn can_apply(kind: InstallKind) -> bool {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let appimage = appimage_path();
    can_apply_with(kind, exe_dir.as_deref(), appimage.as_deref(), dir_writable)
}

/// [`can_apply`] with the directories and the "is writable" probe given.
fn can_apply_with(
    kind: InstallKind,
    exe_dir: Option<&Path>,
    appimage: Option<&Path>,
    writable: impl Fn(&Path) -> bool,
) -> bool {
    match kind {
        InstallKind::WindowsInstaller => exe_dir.is_some_and(writable),
        InstallKind::LinuxAppImage => appimage.and_then(Path::parent).is_some_and(writable),
        _ => false,
    }
}

fn appimage_path() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// Whether a file can be created (and deleted) in `dir`.
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".gitwarp-write-test-{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// What applying an update does; data only, see [`execute`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyPlan {
    /// A detached helper that waits for us to exit, runs the installer
    /// silently and optionally starts the app again.
    WindowsHelper {
        program: &'static str,
        args: Vec<String>,
    },
    /// Copy `source` to `new_file` (next to `target`), make it executable and
    /// rename it over `target`.
    LinuxReplace {
        source: PathBuf,
        new_file: PathBuf,
        target: PathBuf,
        relaunch: bool,
    },
    /// This kind of installation cannot apply updates.
    Unsupported,
}

/// The installer switches: silent, no message boxes, no reboot, no start-up
/// delay. (`CloseApplications` is already on in the Inno Setup script.)
const INSTALLER_SWITCHES: [&str; 4] = ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-"];

/// Quotes `text` as a PowerShell single-quoted string. PowerShell also treats
/// the typographic single quotes as quote characters, so those are doubled too.
fn ps_quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('\'');
    for c in text.chars() {
        quoted.push(c);
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            quoted.push(c);
        }
    }
    quoted.push('\'');
    quoted
}

/// The helper's PowerShell script: wait for process `pid` (at most two
/// minutes), run the installer and wait for it, then (when `relaunch`) start
/// `exe`. The app is restarted even when the installer failed.
fn windows_script(pid: u32, installer: &Path, exe: &Path, relaunch: bool) -> String {
    let switches = INSTALLER_SWITCHES
        .iter()
        .map(|switch| ps_quote(switch))
        .collect::<Vec<_>>()
        .join(",");
    let mut script = format!(
        "try {{ Wait-Process -Id {pid} -Timeout 120 -ErrorAction SilentlyContinue }} catch {{}}; \
         try {{ Start-Process -FilePath {} -ArgumentList {switches} -Wait }} catch {{}}",
        ps_quote(&installer.to_string_lossy()),
    );
    if relaunch {
        script.push_str(&format!(
            "; Start-Process -FilePath {}",
            ps_quote(&exe.to_string_lossy())
        ));
    }
    script
}

/// Decides what applying `staged` does. Pure: nothing is touched.
///
/// * `pid`: this process (the helper waits for it to exit).
/// * `exe`: the running executable (restarted by the Windows helper).
/// * `appimage`: the running AppImage file (Linux).
pub fn plan(
    staged: &StagedUpdate,
    relaunch: bool,
    pid: u32,
    exe: &Path,
    appimage: Option<&Path>,
) -> ApplyPlan {
    match staged.kind {
        InstallKind::WindowsInstaller => ApplyPlan::WindowsHelper {
            program: "powershell.exe",
            args: [
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
            ]
            .map(String::from)
            .into_iter()
            .chain([windows_script(pid, &staged.path, exe, relaunch)])
            .collect(),
        },
        InstallKind::LinuxAppImage => match appimage {
            Some(target) => {
                let mut new_file = target.as_os_str().to_owned();
                new_file.push(".new");
                ApplyPlan::LinuxReplace {
                    source: staged.path.clone(),
                    new_file: PathBuf::from(new_file),
                    target: target.to_path_buf(),
                    relaunch,
                }
            }
            None => ApplyPlan::Unsupported,
        },
        _ => ApplyPlan::Unsupported,
    }
}

/// Copies `source` to `new_file`, makes it executable and renames it over
/// `target`. On any failure `target` is left as it was and `new_file` removed.
fn replace_file(source: &Path, new_file: &Path, target: &Path) -> Result<(), UpdateError> {
    let result = (|| {
        fs::copy(source, new_file)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(new_file, fs::Permissions::from_mode(0o755))?;
        }
        fs::rename(new_file, target)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(new_file);
        return Err(io_error(e));
    }
    Ok(())
}

/// Starts `target` as a new, detached process.
fn spawn_detached(target: &Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    Command::new(target)
        .env_remove("APPIMAGE")
        .env_remove("APPDIR")
        .env_remove("ARGV0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(windows)]
fn spawn_helper(program: &str, args: &[String]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(|_| ())
}

#[cfg(not(windows))]
fn spawn_helper(_program: &str, _args: &[String]) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "the Windows helper only runs on Windows",
    ))
}

/// Carries out a plan. The caller quits the app afterwards.
fn execute(plan: &ApplyPlan) -> Result<(), UpdateError> {
    match plan {
        ApplyPlan::WindowsHelper { program, args } => {
            log::info!("update: starting the installer helper");
            spawn_helper(program, args).map_err(io_error)
        }
        ApplyPlan::LinuxReplace {
            source,
            new_file,
            target,
            relaunch,
        } => {
            log::info!("update: replacing {}", target.display());
            replace_file(source, new_file, target)?;
            if *relaunch {
                // The update is in place; failing to restart is not fatal.
                if let Err(e) = spawn_detached(target) {
                    log::warn!("update: restarting {} failed: {e}", target.display());
                }
            }
            Ok(())
        }
        ApplyPlan::Unsupported => Err(UpdateError::NotApplicable),
    }
}

/// Checks the staged file again, against the signed sums stored beside it.
fn reverify(key: &str, staged: &StagedUpdate) -> Result<(), UpdateError> {
    let dir = staged
        .path
        .parent()
        .ok_or_else(|| UpdateError::Io("the staged file has no folder".to_string()))?;
    let name = staged
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| UpdateError::Io("the staged file name is not text".to_string()))?;
    // Only the file name this version and kind are published under.
    if asset_name(staged.kind, &staged.version).as_deref() != Some(name) {
        return Err(UpdateError::NotApplicable);
    }
    let sums = fs::read_to_string(dir.join(SUMS_NAME)).map_err(io_error)?;
    let sig = fs::read_to_string(dir.join(SUMS_SIG_NAME)).map_err(io_error)?;
    verify_with_key(key, &staged.path, name, &sums, &sig)?;
    Ok(())
}

/// Verifies and applies `staged`; see [`apply`].
#[allow(clippy::too_many_arguments)]
fn apply_in(
    root: &Path,
    running: &Version,
    key: &str,
    staged: &StagedUpdate,
    relaunch: bool,
    pid: u32,
    exe: &Path,
    appimage: Option<&Path>,
) -> Result<(), UpdateError> {
    let version = parse_version(&staged.version).ok_or(UpdateError::BadTag)?;
    if version <= *running {
        return Err(UpdateError::NotNewer);
    }
    // Only files from the cache are ever applied.
    let dir = staged.path.parent().ok_or(UpdateError::NotApplicable)?;
    if !dir.starts_with(root) || dir.file_name().and_then(|n| n.to_str()) != Some(&staged.version) {
        return Err(UpdateError::NotApplicable);
    }
    if let Err(e) = reverify(key, staged) {
        log::warn!("update {}: verification failed: {e}", staged.version);
        if !matches!(e, UpdateError::NotApplicable) {
            // Never keep a staged file that does not verify.
            let _ = fs::remove_dir_all(dir);
        }
        return Err(e);
    }
    log::info!("update {} verified, applying", staged.version);
    execute(&plan(staged, relaunch, pid, exe, appimage))
}

/// Re-verifies the staged update and applies it: on Windows a hidden helper is
/// started that installs it once this process has exited; on Linux the AppImage
/// is replaced right away. With `relaunch` the app is started again. The caller
/// quits the app afterwards. Blocking (hashing, copying): run it off the UI
/// thread. Applying twice is a no-op.
pub fn apply(staged: &StagedUpdate, relaunch: bool) -> Result<(), UpdateError> {
    if APPLIED.load(Ordering::SeqCst) {
        return Ok(());
    }
    let running = parse_version(env!("CARGO_PKG_VERSION")).ok_or(UpdateError::BadTag)?;
    let root = cache_root().ok_or(UpdateError::NoCacheDir)?;
    let kind = detect_install_kind();
    if staged.kind != kind || !can_apply(kind) {
        return Err(UpdateError::NotApplicable);
    }
    let exe = std::env::current_exe().map_err(io_error)?;
    apply_in(
        &root,
        &running,
        UPDATE_PUBLIC_KEY,
        staged,
        relaunch,
        std::process::id(),
        &exe,
        appimage_path().as_deref(),
    )?;
    APPLIED.store(true, Ordering::SeqCst);
    Ok(())
}

/// Finds the newest staged update and applies it without relaunching; used
/// when the app quits. `Ok(None)` when there is nothing to apply.
pub fn apply_newest_staged() -> Result<Option<String>, UpdateError> {
    if APPLIED.load(Ordering::SeqCst) || !can_apply(detect_install_kind()) {
        return Ok(None);
    }
    match find_staged() {
        Some(staged) => apply(&staged, false).map(|()| Some(staged.version)),
        None => Ok(None),
    }
}

/// The newest staged update that is newer than the running version and still
/// verifies; deletes cached versions that are not newer. Blocking.
pub fn find_staged() -> Option<StagedUpdate> {
    let running = parse_version(env!("CARGO_PKG_VERSION"))?;
    let root = cache_root()?;
    find_staged_with_key(&root, &running, detect_install_kind(), UPDATE_PUBLIC_KEY)
}

fn find_staged_with_key(
    root: &Path,
    running: &Version,
    kind: InstallKind,
    key: &str,
) -> Option<StagedUpdate> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(version) = parse_version(&name) else {
            continue;
        };
        if !path.is_dir() {
            continue;
        }
        if version <= *running {
            // Left over from an update that was applied (or an older download).
            log::info!("update: removing cached version {name}");
            let _ = fs::remove_dir_all(&path);
        } else {
            candidates.push((version, name, path));
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, name, dir) in candidates {
        let Some(file_name) = asset_name(kind, &name) else {
            continue;
        };
        let staged = StagedUpdate {
            version: name,
            kind,
            path: dir.join(file_name),
        };
        if !staged.path.is_file() {
            continue;
        }
        match reverify(key, &staged) {
            Ok(()) => return Some(staged),
            Err(e) => {
                log::warn!("update: cached {} does not verify: {e}", staged.version);
                let _ = fs::remove_dir_all(&dir);
            }
        }
    }
    None
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

    #[test]
    fn can_apply_table() {
        use InstallKind::*;
        let dir = Path::new("/some/dir");
        let appimage = Path::new("/some/dir/GitWarp.AppImage");
        let table = [
            (WindowsInstaller, true, true),
            (WindowsInstaller, false, false),
            (LinuxAppImage, true, true),
            (LinuxAppImage, false, false),
            (WindowsPortable, true, false),
            (MacApp, true, false),
            (MacHomebrew, true, false),
            (LinuxPackage, true, false),
            (Unknown, true, false),
        ];
        for (kind, writable, expected) in table {
            assert_eq!(
                can_apply_with(kind, Some(dir), Some(appimage), |_| writable),
                expected,
                "{kind:?} writable={writable}"
            );
        }
        // Missing directories never apply.
        assert!(!can_apply_with(WindowsInstaller, None, None, |_| true));
        assert!(!can_apply_with(LinuxAppImage, Some(dir), None, |_| true));
        // The probe gets the directory of the AppImage / the executable.
        assert!(can_apply_with(LinuxAppImage, None, Some(appimage), |p| p == dir));
        assert!(can_apply_with(WindowsInstaller, Some(dir), None, |p| p == dir));
    }

    #[test]
    fn dir_writable_probes_a_real_folder() {
        let dir = temp_dir("writable");
        assert!(dir_writable(&dir));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "probe file removed");
        assert!(!dir_writable(&dir.join("missing")));
        let _ = fs::remove_dir_all(&dir);
    }

    fn staged(kind: InstallKind, path: &str) -> StagedUpdate {
        StagedUpdate {
            version: "1.2.3".to_string(),
            kind,
            path: PathBuf::from(path),
        }
    }

    const SWITCHES: &str = "'/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/SP-'";

    #[test]
    fn windows_plan_runs_a_hidden_helper_with_the_installer_switches() {
        let update = staged(
            InstallKind::WindowsInstaller,
            r"C:\Users\me\updates\1.2.3\gitwarp-1.2.3-windows-x86_64-setup.exe",
        );
        let exe = Path::new(r"C:\Users\me\AppData\Local\Programs\GitWarp\gitwarp.exe");
        let ApplyPlan::WindowsHelper { program, args } = plan(&update, true, 4242, exe, None)
        else {
            panic!("expected a Windows helper");
        };
        assert_eq!(program, "powershell.exe");
        assert_eq!(
            &args[..5],
            [
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command"
            ]
        );
        assert_eq!(args.len(), 6);
        assert_eq!(
            args[5],
            format!(
                "try {{ Wait-Process -Id 4242 -Timeout 120 -ErrorAction SilentlyContinue }} catch {{}}; \
                 try {{ Start-Process -FilePath 'C:\\Users\\me\\updates\\1.2.3\\gitwarp-1.2.3-windows-x86_64-setup.exe' -ArgumentList {SWITCHES} -Wait }} catch {{}}; \
                 Start-Process -FilePath 'C:\\Users\\me\\AppData\\Local\\Programs\\GitWarp\\gitwarp.exe'"
            )
        );
        // Without a relaunch the script ends after the installer.
        let ApplyPlan::WindowsHelper { args, .. } = plan(&update, false, 7, exe, None) else {
            panic!("expected a Windows helper");
        };
        assert!(args[5].ends_with("-Wait } catch {}"));
        assert!(!args[5].contains("gitwarp.exe"));
        assert!(args[5].contains("Wait-Process -Id 7 "));
    }

    #[test]
    fn windows_script_quotes_tricky_paths() {
        let script = windows_script(
            1,
            Path::new(r"C:\Users\O'Brien\Ünï côdé\up dates\setup.exe"),
            Path::new(r"D:\it’s here\x ‘y’\gitwarp.exe"),
            true,
        );
        assert!(script.contains(r"-FilePath 'C:\Users\O''Brien\Ünï côdé\up dates\setup.exe'"));
        assert!(script.contains(r"Start-Process -FilePath 'D:\it’’s here\x ‘‘y’’\gitwarp.exe'"));
        assert_eq!(ps_quote("a'b"), "'a''b'");
        assert_eq!(ps_quote("'; calc; '"), "'''; calc; '''");
        assert_eq!(ps_quote(""), "''");
    }

    #[test]
    fn other_kinds_have_no_plan() {
        let exe = Path::new("/x");
        for kind in [
            InstallKind::WindowsPortable,
            InstallKind::MacApp,
            InstallKind::MacHomebrew,
            InstallKind::LinuxPackage,
            InstallKind::Unknown,
        ] {
            assert_eq!(
                plan(&staged(kind, "/f"), true, 1, exe, Some(exe)),
                ApplyPlan::Unsupported
            );
        }
        // An AppImage without the environment variable cannot be replaced.
        assert_eq!(
            plan(
                &staged(InstallKind::LinuxAppImage, "/f"),
                true,
                1,
                exe,
                None
            ),
            ApplyPlan::Unsupported
        );
    }

    #[test]
    fn linux_plan_copies_next_to_the_appimage() {
        let update = staged(InstallKind::LinuxAppImage, "/cache/1.2.3/x.AppImage");
        let target = Path::new("/home/me/Apps/GitWarp.AppImage");
        assert_eq!(
            plan(
                &update,
                true,
                1,
                Path::new("/tmp/.mount/gitwarp"),
                Some(target)
            ),
            ApplyPlan::LinuxReplace {
                source: PathBuf::from("/cache/1.2.3/x.AppImage"),
                new_file: PathBuf::from("/home/me/Apps/GitWarp.AppImage.new"),
                target: target.to_path_buf(),
                relaunch: true,
            }
        );
    }

    #[test]
    fn replace_file_swaps_the_target_and_cleans_up() {
        let dir = temp_dir("replace-ok");
        let source = dir.join("source");
        let target = dir.join("GitWarp.AppImage");
        let new_file = dir.join("GitWarp.AppImage.new");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        replace_file(&source, &new_file, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!new_file.exists());
        assert!(source.exists(), "the staged file is not consumed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_file_failure_leaves_the_old_file_and_no_leftovers() {
        let dir = temp_dir("replace-fail");
        let target = dir.join("GitWarp.AppImage");
        let new_file = dir.join("GitWarp.AppImage.new");
        fs::write(&target, b"old").unwrap();
        // The source is missing: nothing is copied.
        assert!(replace_file(&dir.join("missing"), &new_file, &target).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(!new_file.exists());
        // The rename fails after the copy (the target is a directory).
        let source = dir.join("source");
        fs::write(&source, b"new").unwrap();
        let blocked = dir.join("blocked.AppImage");
        fs::create_dir_all(&blocked).unwrap();
        let blocked_new = dir.join("blocked.AppImage.new");
        assert!(replace_file(&source, &blocked_new, &blocked).is_err());
        assert!(blocked.is_dir());
        assert!(!blocked_new.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A cache with `<version>/<AppImage>` + signed sums; the signed fixture is
    /// for version 1.2.3 and the payload `payload`.
    fn write_version(root: &Path, version: &str, file: &str, content: &[u8]) -> PathBuf {
        let dir = root.join(version);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), content).unwrap();
        fs::write(dir.join(SUMS_NAME), SUMS).unwrap();
        fs::write(dir.join(SUMS_SIG_NAME), SIG_A).unwrap();
        dir
    }

    #[test]
    fn find_staged_picks_the_newest_verifiable_and_cleans_old_versions() {
        let root = temp_dir("find");
        let kind = InstallKind::LinuxAppImage;
        let running = parse_version("1.0.0").unwrap();
        write_version(&root, "1.0.0", "gitwarp-1.0.0-x86_64.AppImage", b"payload");
        write_version(&root, "0.9.0", "gitwarp-0.9.0-x86_64.AppImage", b"payload");
        write_version(&root, "1.2.3", NAME, b"payload");
        // Newer, but not listed in the signed sums.
        write_version(&root, "1.2.4", "gitwarp-1.2.4-x86_64.AppImage", b"payload");
        // Newest, but without any file.
        fs::create_dir_all(root.join("9.0.0")).unwrap();
        // Not a version: left alone.
        fs::create_dir_all(root.join("tmp")).unwrap();

        let found = find_staged_with_key(&root, &running, kind, KEY_A).unwrap();
        assert_eq!(found.version, "1.2.3");
        assert_eq!(found.kind, kind);
        assert_eq!(found.path, root.join("1.2.3").join(NAME));
        assert!(!root.join("1.0.0").exists());
        assert!(!root.join("0.9.0").exists());
        assert!(!root.join("1.2.4").exists(), "unverifiable entry removed");
        assert!(root.join("tmp").exists());

        // Equal to the running version: nothing to apply, entry removed.
        let running = parse_version("1.2.3").unwrap();
        assert!(find_staged_with_key(&root, &running, kind, KEY_A).is_none());
        assert!(!root.join("1.2.3").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn find_staged_ignores_a_tampered_file_and_a_wrong_key() {
        let root = temp_dir("find-tampered");
        let kind = InstallKind::LinuxAppImage;
        let running = parse_version("1.0.0").unwrap();
        write_version(&root, "1.2.3", NAME, b"tampered");
        assert!(find_staged_with_key(&root, &running, kind, KEY_A).is_none());
        assert!(!root.join("1.2.3").exists());
        // Good file, but the embedded (placeholder) key does not match.
        write_version(&root, "1.2.3", NAME, b"payload");
        assert!(find_staged_with_key(&root, &running, kind, UPDATE_PUBLIC_KEY).is_none());
        // A kind without a release file finds nothing.
        write_version(&root, "1.2.3", NAME, b"payload");
        assert!(find_staged_with_key(&root, &running, InstallKind::MacHomebrew, KEY_A).is_none());
        assert!(find_staged_with_key(&root.join("missing"), &running, kind, KEY_A).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn apply_in_replaces_the_appimage_after_verifying() {
        let root = temp_dir("apply");
        let running = parse_version("1.0.0").unwrap();
        let dir = write_version(&root, "1.2.3", NAME, b"payload");
        let target = root.join("GitWarp.AppImage");
        fs::write(&target, b"old").unwrap();
        let update = StagedUpdate {
            version: "1.2.3".to_string(),
            kind: InstallKind::LinuxAppImage,
            path: dir.join(NAME),
        };
        let exe = Path::new("/unused");
        let apply = |update: &StagedUpdate, running: &Version, key: &str| {
            apply_in(&root, running, key, update, false, 1, exe, Some(&target))
        };

        // Not newer than the running version: refused, nothing touched.
        let same = parse_version("1.2.3").unwrap();
        assert!(matches!(
            apply(&update, &same, KEY_A),
            Err(UpdateError::NotNewer)
        ));
        assert_eq!(fs::read(&target).unwrap(), b"old");

        // A file outside the cache is refused.
        let outside = temp_dir("apply-outside");
        let elsewhere = StagedUpdate {
            path: outside.join(NAME),
            ..update.clone()
        };
        fs::write(&elsewhere.path, b"payload").unwrap();
        assert!(matches!(
            apply(&elsewhere, &running, KEY_A),
            Err(UpdateError::NotApplicable)
        ));
        assert_eq!(fs::read(&target).unwrap(), b"old");
        let _ = fs::remove_dir_all(&outside);

        // A wrong key: verification fails, the cache entry is deleted.
        assert!(matches!(
            apply(&update, &running, UPDATE_PUBLIC_KEY),
            Err(UpdateError::Verify(_))
        ));
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(!dir.exists());

        // Verified: the AppImage is replaced.
        let dir = write_version(&root, "1.2.3", NAME, b"payload");
        assert_eq!(dir, update.path.parent().unwrap());
        apply(&update, &running, KEY_A).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"payload");
        assert!(!root.join("GitWarp.AppImage.new").exists());

        // A file that changed after staging is refused and deleted.
        fs::write(dir.join(NAME), b"changed").unwrap();
        fs::write(&target, b"old").unwrap();
        assert!(matches!(
            apply(&update, &running, KEY_A),
            Err(UpdateError::Verify(VerifyError::HashMismatch))
        ));
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(!dir.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
