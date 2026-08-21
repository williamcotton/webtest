//! Managed Chrome for Testing downloads, cache ownership, and executable resolution.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

pub const TESTED_CHROME_VERSION: &str = "152.0.7977.54";
const LOCK_WAIT: Duration = Duration::from_secs(30);
const LOCK_STALE_AFTER: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Platform {
    Linux64,
    MacArm64,
    MacX64,
    Win32,
    Win64,
}

impl Platform {
    pub fn current() -> Result<Self, BrowserManagerError> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Ok(Self::Linux64),
            ("macos", "aarch64") => Ok(Self::MacArm64),
            ("macos", "x86_64") => Ok(Self::MacX64),
            ("windows", "x86") => Ok(Self::Win32),
            ("windows", "x86_64") => Ok(Self::Win64),
            (os, arch) => Err(BrowserManagerError::UnsupportedPlatform {
                os: os.into(),
                arch: arch.into(),
            }),
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Linux64 => "linux64",
            Self::MacArm64 => "mac-arm64",
            Self::MacX64 => "mac-x64",
            Self::Win32 => "win32",
            Self::Win64 => "win64",
        }
    }

    fn executable_relative(self) -> PathBuf {
        match self {
            Self::Linux64 => PathBuf::from("chrome-linux64/chrome"),
            Self::MacArm64 => PathBuf::from(
                "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
            ),
            Self::MacX64 => PathBuf::from(
                "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
            ),
            Self::Win32 => PathBuf::from("chrome-win32/chrome.exe"),
            Self::Win64 => PathBuf::from("chrome-win64/chrome.exe"),
        }
    }
}

#[derive(Clone, Debug)]
struct Release {
    version: String,
    platform: Platform,
    url: String,
    md5: String,
}

impl Release {
    fn tested(platform: Platform) -> Self {
        // Hex-encoded values from each artifact's official Google Cloud
        // Storage `x-goog-hash: md5=...` metadata.
        let (url, md5) = match platform {
            Platform::Linux64 => (
                "https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/linux64/chrome-linux64.zip",
                "282ca8171761f4b47fa718c51dd16a92",
            ),
            Platform::MacArm64 => (
                "https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/mac-arm64/chrome-mac-arm64.zip",
                "5ad78a348307d70e87dad3662686b993",
            ),
            Platform::MacX64 => (
                "https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/mac-x64/chrome-mac-x64.zip",
                "8c3fc396155df61696d3c3736117c8bf",
            ),
            Platform::Win32 => (
                "https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/win32/chrome-win32.zip",
                "9e01af5adae986a5eaef2a1b29c59bad",
            ),
            Platform::Win64 => (
                "https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/win64/chrome-win64.zip",
                "3dfb0532a3bc7bccedd9701b7299f9b1",
            ),
        };
        Self {
            version: TESTED_CHROME_VERSION.into(),
            platform,
            url: url.into(),
            md5: md5.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct InstallRecord {
    managed_by: String,
    version: String,
    platform: Platform,
    checksum_algorithm: String,
    checksum: String,
    source_url: String,
    installed_unix_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledBrowser {
    pub version: String,
    pub platform: Platform,
    pub executable: PathBuf,
    pub checksum: String,
}

#[derive(Clone)]
pub struct BrowserManager {
    root: PathBuf,
    downloader: Arc<dyn Downloader>,
}

trait Downloader: Send + Sync {
    fn open(&self, url: &str) -> Result<Box<dyn Read>, BrowserManagerError>;
}

struct HttpDownloader;

impl Downloader for HttpDownloader {
    fn open(&self, url: &str) -> Result<Box<dyn Read>, BrowserManagerError> {
        let response = ureq::get(url)
            .call()
            .map_err(|error| BrowserManagerError::Download {
                url: url.into(),
                message: error.to_string(),
            })?;
        Ok(Box::new(response.into_reader()))
    }
}

#[derive(Debug, Error)]
pub enum BrowserManagerError {
    #[error("managed Chrome is not available for {os}/{arch}")]
    UnsupportedPlatform { os: String, arch: String },
    #[error(
        "Chrome for Testing version `{requested}` is not supported; this WebTest release uses `{tested}`"
    )]
    UnsupportedVersion { requested: String, tested: String },
    #[error("could not determine the WebTest cache directory")]
    CacheDirectory,
    #[error("could not create `{path}`: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("timed out waiting for browser installation lock `{0}`")]
    LockTimeout(PathBuf),
    #[error("could not acquire browser installation lock `{path}`: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not download `{url}`: {message}")]
    Download { url: String, message: String },
    #[error("could not write browser archive `{path}`: {source}")]
    WriteArchive {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("browser checksum mismatch: expected {expected}, got {actual}")]
    Checksum { expected: String, actual: String },
    #[error("could not read Chrome archive: {0}")]
    Archive(#[from] zip::result::ZipError),
    #[error("Chrome archive contained unsafe path `{0}`")]
    UnsafeArchivePath(String),
    #[error("could not extract `{path}`: {source}")]
    Extract {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Chrome archive did not contain expected executable `{0}`")]
    MissingExecutable(PathBuf),
    #[error("could not write installation metadata `{path}`: {source}")]
    WriteMetadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not activate managed Chrome at `{path}`: {source}")]
    Activate {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("refusing to clean unowned or invalid browser directory `{0}`")]
    Unowned(PathBuf),
    #[error("could not remove browser directory `{path}`: {source}")]
    Remove {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl BrowserManager {
    pub fn new() -> Result<Self, BrowserManagerError> {
        let root = cache_root()?;
        Ok(Self {
            root,
            downloader: Arc::new(HttpDownloader),
        })
    }

    pub fn with_cache_root(root: PathBuf) -> Self {
        Self {
            root,
            downloader: Arc::new(HttpDownloader),
        }
    }

    pub fn cache_root(&self) -> &Path {
        &self.root
    }

    pub fn install(&self, version: Option<&str>) -> Result<InstalledBrowser, BrowserManagerError> {
        let platform = Platform::current()?;
        let requested = version.unwrap_or(TESTED_CHROME_VERSION);
        if requested != TESTED_CHROME_VERSION {
            return Err(BrowserManagerError::UnsupportedVersion {
                requested: requested.into(),
                tested: TESTED_CHROME_VERSION.into(),
            });
        }
        self.install_release(&Release::tested(platform))
    }

    pub fn current(&self) -> Result<Option<InstalledBrowser>, BrowserManagerError> {
        let platform = Platform::current()?;
        self.read_install(&Release::tested(platform))
    }

    pub fn list(&self) -> Result<Vec<InstalledBrowser>, BrowserManagerError> {
        let base = self.root.join("browsers/chrome");
        if !base.is_dir() {
            return Ok(Vec::new());
        }
        let mut installed = Vec::new();
        let versions = fs::read_dir(&base).map_err(|source| BrowserManagerError::Extract {
            path: base.clone(),
            source,
        })?;
        for version in versions.flatten() {
            if version
                .file_type()
                .map(|kind| !kind.is_dir() || kind.is_symlink())
                .unwrap_or(true)
            {
                continue;
            }
            let platforms = match fs::read_dir(version.path()) {
                Ok(platforms) => platforms,
                Err(_) => continue,
            };
            for platform in platforms.flatten() {
                if platform
                    .file_type()
                    .map(|kind| !kind.is_dir() || kind.is_symlink())
                    .unwrap_or(true)
                {
                    continue;
                }
                let marker = platform.path().join("install.json");
                let Ok(record) = read_record(&marker) else {
                    continue;
                };
                if record.managed_by != "webtest" {
                    continue;
                }
                if version.file_name().to_string_lossy() != record.version
                    || platform.file_name().to_string_lossy() != record.platform.name()
                {
                    continue;
                }
                let executable = platform.path().join(record.platform.executable_relative());
                if executable.is_file() {
                    installed.push(InstalledBrowser {
                        version: record.version,
                        platform: record.platform,
                        executable,
                        checksum: record.checksum,
                    });
                }
            }
        }
        installed.sort_by(|left, right| {
            (&left.version, left.platform.name()).cmp(&(&right.version, right.platform.name()))
        });
        Ok(installed)
    }

    pub fn clean(&self, version: Option<&str>) -> Result<usize, BrowserManagerError> {
        let installs = self.list()?;
        let mut removed = 0;
        for install in installs {
            if version.is_some_and(|version| version != install.version) {
                continue;
            }
            let directory = self.install_directory(&install.version, install.platform);
            validate_owned_directory(&self.root, &directory, &install.version, install.platform)?;
            fs::remove_dir_all(&directory).map_err(|source| BrowserManagerError::Remove {
                path: directory,
                source,
            })?;
            removed += 1;
        }
        Ok(removed)
    }

    fn install_release(&self, release: &Release) -> Result<InstalledBrowser, BrowserManagerError> {
        if let Some(installed) = self.read_install(release)? {
            return Ok(installed);
        }
        let base = self.root.join("browsers/chrome");
        fs::create_dir_all(&base).map_err(|source| BrowserManagerError::CreateDirectory {
            path: base.clone(),
            source,
        })?;
        let _lock = InstallLock::acquire(&base.join("install.lock"))?;
        if let Some(installed) = self.read_install(release)? {
            return Ok(installed);
        }

        let mut archive =
            NamedTempFile::new_in(&base).map_err(|source| BrowserManagerError::WriteArchive {
                path: base.clone(),
                source,
            })?;
        let mut reader = self.downloader.open(&release.url)?;
        let mut hasher = Md5::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count =
                reader
                    .read(&mut buffer)
                    .map_err(|source| BrowserManagerError::Download {
                        url: release.url.clone(),
                        message: source.to_string(),
                    })?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
            archive.write_all(&buffer[..count]).map_err(|source| {
                BrowserManagerError::WriteArchive {
                    path: archive.path().to_path_buf(),
                    source,
                }
            })?;
        }
        archive
            .flush()
            .map_err(|source| BrowserManagerError::WriteArchive {
                path: archive.path().to_path_buf(),
                source,
            })?;
        let actual = format!("{:x}", hasher.finalize());
        if actual != release.md5 {
            return Err(BrowserManagerError::Checksum {
                expected: release.md5.clone(),
                actual,
            });
        }

        let version_parent = base.join(&release.version);
        fs::create_dir_all(&version_parent).map_err(|source| {
            BrowserManagerError::CreateDirectory {
                path: version_parent.clone(),
                source,
            }
        })?;
        let staging = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(&version_parent)
            .map_err(|source| BrowserManagerError::CreateDirectory {
                path: version_parent.clone(),
                source,
            })?;
        extract_zip(archive.path(), staging.path())?;
        let executable = staging.path().join(release.platform.executable_relative());
        if !executable.is_file() {
            return Err(BrowserManagerError::MissingExecutable(executable));
        }
        make_executable(&executable)?;
        let record = InstallRecord {
            managed_by: "webtest".into(),
            version: release.version.clone(),
            platform: release.platform,
            checksum_algorithm: "md5".into(),
            checksum: release.md5.clone(),
            source_url: release.url.clone(),
            installed_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        let metadata = staging.path().join("install.json");
        let encoded = serde_json::to_vec_pretty(&record).map_err(|source| {
            BrowserManagerError::WriteMetadata {
                path: metadata.clone(),
                source: io::Error::other(source),
            }
        })?;
        fs::write(&metadata, encoded).map_err(|source| BrowserManagerError::WriteMetadata {
            path: metadata,
            source,
        })?;

        let target = self.install_directory(&release.version, release.platform);
        fs::rename(staging.path(), &target).map_err(|source| BrowserManagerError::Activate {
            path: target.clone(),
            source,
        })?;
        self.read_install(release)?
            .ok_or(BrowserManagerError::MissingExecutable(
                target.join(release.platform.executable_relative()),
            ))
    }

    fn read_install(
        &self,
        release: &Release,
    ) -> Result<Option<InstalledBrowser>, BrowserManagerError> {
        let directory = self.install_directory(&release.version, release.platform);
        let marker = directory.join("install.json");
        if !marker.is_file() {
            return Ok(None);
        }
        let Ok(record) = read_record(&marker) else {
            return Ok(None);
        };
        if record.managed_by != "webtest"
            || record.version != release.version
            || record.platform != release.platform
            || record.checksum_algorithm != "md5"
            || record.checksum != release.md5
        {
            return Ok(None);
        }
        let executable = directory.join(release.platform.executable_relative());
        if !executable.is_file() {
            return Ok(None);
        }
        Ok(Some(InstalledBrowser {
            version: record.version,
            platform: record.platform,
            executable,
            checksum: record.checksum,
        }))
    }

    fn install_directory(&self, version: &str, platform: Platform) -> PathBuf {
        self.root
            .join("browsers/chrome")
            .join(version)
            .join(platform.name())
    }
}

fn cache_root() -> Result<PathBuf, BrowserManagerError> {
    if let Some(path) = std::env::var_os("WEBTEST_CACHE_DIR") {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            Ok(path)
        } else {
            std::env::current_dir()
                .map(|directory| directory.join(path))
                .map_err(|_| BrowserManagerError::CacheDirectory)
        };
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Caches/webtest"))
            .ok_or(BrowserManagerError::CacheDirectory);
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
            return Ok(PathBuf::from(path).join("webtest"));
        }
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".cache/webtest"))
            .ok_or(BrowserManagerError::CacheDirectory);
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|home| home.join("webtest/cache"))
            .ok_or(BrowserManagerError::CacheDirectory);
    }
    #[allow(unreachable_code)]
    Err(BrowserManagerError::CacheDirectory)
}

fn extract_zip(archive: &Path, destination: &Path) -> Result<(), BrowserManagerError> {
    let file = File::open(archive).map_err(|source| BrowserManagerError::Extract {
        path: archive.to_path_buf(),
        source,
    })?;
    let mut archive = zip::ZipArchive::new(file)?;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        if entry.enclosed_name().is_none() {
            return Err(BrowserManagerError::UnsafeArchivePath(entry.name().into()));
        }
    }
    archive.extract(destination)?;
    Ok(())
}

fn make_executable(path: &Path) -> Result<(), BrowserManagerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|source| BrowserManagerError::Extract {
                path: path.to_path_buf(),
                source,
            })?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o700);
        fs::set_permissions(path, permissions).map_err(|source| BrowserManagerError::Extract {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn read_record(path: &Path) -> Result<InstallRecord, ()> {
    let bytes = fs::read(path).map_err(|_| ())?;
    serde_json::from_slice(&bytes).map_err(|_| ())
}

fn validate_owned_directory(
    root: &Path,
    directory: &Path,
    version: &str,
    platform: Platform,
) -> Result<(), BrowserManagerError> {
    if fs::symlink_metadata(directory)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(true)
    {
        return Err(BrowserManagerError::Unowned(directory.to_path_buf()));
    }
    let chrome_root = root.join("browsers/chrome");
    let version_directory = chrome_root.join(version);
    if fs::symlink_metadata(&version_directory)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(true)
    {
        return Err(BrowserManagerError::Unowned(directory.to_path_buf()));
    }
    let expected_parent = root
        .join("browsers/chrome")
        .join(version)
        .join(platform.name());
    if directory != expected_parent {
        return Err(BrowserManagerError::Unowned(directory.to_path_buf()));
    }
    let canonical_root = fs::canonicalize(&chrome_root)
        .map_err(|_| BrowserManagerError::Unowned(directory.to_path_buf()))?;
    let canonical_directory = fs::canonicalize(directory)
        .map_err(|_| BrowserManagerError::Unowned(directory.to_path_buf()))?;
    if !canonical_directory.starts_with(canonical_root) {
        return Err(BrowserManagerError::Unowned(directory.to_path_buf()));
    }
    let record = read_record(&directory.join("install.json"))
        .map_err(|_| BrowserManagerError::Unowned(directory.to_path_buf()))?;
    if record.managed_by != "webtest" || record.version != version || record.platform != platform {
        return Err(BrowserManagerError::Unowned(directory.to_path_buf()));
    }
    Ok(())
}

struct InstallLock {
    path: PathBuf,
}

impl InstallLock {
    fn acquire(path: &Path) -> Result<Self, BrowserManagerError> {
        let started = Instant::now();
        loop {
            match OpenOptions::new().create_new(true).write(true).open(path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(path) {
                        let _ = fs::remove_file(path);
                        continue;
                    }
                    if started.elapsed() >= LOCK_WAIT {
                        return Err(BrowserManagerError::LockTimeout(path.to_path_buf()));
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Err(source) => {
                    return Err(BrowserManagerError::Lock {
                        path: path.to_path_buf(),
                        source,
                    });
                }
            }
        }
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn lock_is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age >= LOCK_STALE_AFTER)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Cursor, Write as _},
        net::TcpListener,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use zip::write::SimpleFileOptions;

    use super::*;

    struct BytesDownloader {
        bytes: Vec<u8>,
        calls: AtomicUsize,
        fail: Mutex<bool>,
    }

    impl Downloader for BytesDownloader {
        fn open(&self, _url: &str) -> Result<Box<dyn Read>, BrowserManagerError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if *self.fail.lock().expect("fail lock") {
                return Err(BrowserManagerError::Download {
                    url: "fixture".into(),
                    message: "interrupted".into(),
                });
            }
            Ok(Box::new(Cursor::new(self.bytes.clone())))
        }
    }

    fn fixture(platform: Platform) -> (Vec<u8>, String) {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            writer
                .start_file(
                    platform.executable_relative().to_string_lossy(),
                    SimpleFileOptions::default().unix_permissions(0o755),
                )
                .expect("start executable");
            writer.write_all(b"fake chrome").expect("write executable");
            writer.finish().expect("finish archive");
        }
        let bytes = bytes.into_inner();
        let checksum = format!("{:x}", Md5::digest(&bytes));
        (bytes, checksum)
    }

    fn manager(root: &Path, platform: Platform) -> (BrowserManager, Release, Arc<BytesDownloader>) {
        let (bytes, md5) = fixture(platform);
        let downloader = Arc::new(BytesDownloader {
            bytes,
            calls: AtomicUsize::new(0),
            fail: Mutex::new(false),
        });
        let manager = BrowserManager {
            root: root.to_path_buf(),
            downloader: downloader.clone(),
        };
        let release = Release {
            version: "1.2.3.4".into(),
            platform,
            url: "https://example.invalid/chrome.zip".into(),
            md5,
        };
        (manager, release, downloader)
    }

    #[test]
    fn installs_lists_resolves_and_cleans_verified_archive() {
        let directory = tempfile::tempdir().expect("temp directory");
        let (manager, release, downloader) = manager(directory.path(), Platform::MacArm64);
        let installed = manager.install_release(&release).expect("install");
        assert!(installed.executable.is_file());
        assert_eq!(downloader.calls.load(Ordering::Relaxed), 1);
        let again = manager.install_release(&release).expect("reuse");
        assert_eq!(again.executable, installed.executable);
        assert_eq!(downloader.calls.load(Ordering::Relaxed), 1);
        assert_eq!(manager.list().expect("list").len(), 1);
        assert_eq!(manager.clean(Some("1.2.3.4")).expect("clean"), 1);
        assert!(!installed.executable.exists());
    }

    #[test]
    fn checksum_mismatch_never_activates_staging_directory() {
        let directory = tempfile::tempdir().expect("temp directory");
        let (manager, mut release, _) = manager(directory.path(), Platform::Linux64);
        release.md5 = "00000000000000000000000000000000".into();
        let error = manager.install_release(&release).expect_err("checksum");
        assert!(matches!(error, BrowserManagerError::Checksum { .. }));
        assert!(
            !manager
                .install_directory(&release.version, release.platform)
                .exists()
        );
    }

    #[test]
    fn interrupted_download_leaves_no_installation() {
        let directory = tempfile::tempdir().expect("temp directory");
        let (manager, release, downloader) = manager(directory.path(), Platform::Linux64);
        *downloader.fail.lock().expect("fail lock") = true;
        assert!(manager.install_release(&release).is_err());
        assert!(
            !manager
                .install_directory(&release.version, release.platform)
                .exists()
        );
    }

    #[test]
    fn concurrent_installers_download_once() {
        let directory = tempfile::tempdir().expect("temp directory");
        let (manager, release, downloader) = manager(directory.path(), Platform::Linux64);
        let first_manager = manager.clone();
        let first_release = release.clone();
        let first = thread::spawn(move || first_manager.install_release(&first_release));
        let second_manager = manager.clone();
        let second_release = release.clone();
        let second = thread::spawn(move || second_manager.install_release(&second_release));
        first.join().expect("first thread").expect("first install");
        second
            .join()
            .expect("second thread")
            .expect("second install");
        assert_eq!(downloader.calls.load(Ordering::Relaxed), 1);
    }

    #[cfg(unix)]
    #[test]
    fn clean_refuses_symlinked_install_directory() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temp directory");
        let outside = tempfile::tempdir().expect("outside");
        let manager = BrowserManager::with_cache_root(directory.path().to_path_buf());
        let target = manager.install_directory("1", Platform::Linux64);
        fs::create_dir_all(target.parent().expect("parent")).expect("create parent");
        symlink(outside.path(), &target).expect("symlink");
        let error = validate_owned_directory(directory.path(), &target, "1", Platform::Linux64)
            .expect_err("refuse symlink");
        assert!(matches!(error, BrowserManagerError::Unowned(_)));
        assert_eq!(manager.clean(None).expect("safe clean"), 0);
        assert!(outside.path().exists());
    }

    #[test]
    fn tested_release_metadata_is_complete() {
        for platform in [
            Platform::Linux64,
            Platform::MacArm64,
            Platform::MacX64,
            Platform::Win32,
            Platform::Win64,
        ] {
            let release = Release::tested(platform);
            assert_eq!(release.version, TESTED_CHROME_VERSION);
            assert!(release.url.starts_with("https://storage.googleapis.com/"));
            assert_eq!(release.md5.len(), 32);
            assert_eq!(release.platform, platform);
        }
    }

    #[test]
    fn installs_from_local_http_artifact_server() {
        let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
            return;
        };
        let platform = Platform::current().unwrap_or(Platform::Linux64);
        let (bytes, md5) = fixture(platform);
        let address = listener.local_addr().expect("artifact address");
        let response_bytes = bytes.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("artifact request");
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_bytes.len()
            )
            .expect("artifact headers");
            stream.write_all(&response_bytes).expect("artifact body");
        });
        let directory = tempfile::tempdir().expect("temp directory");
        let manager = BrowserManager::with_cache_root(directory.path().to_path_buf());
        let release = Release {
            version: "http-fixture".into(),
            platform,
            url: format!("http://{address}/chrome.zip"),
            md5,
        };
        let installed = manager.install_release(&release).expect("HTTP install");
        assert!(installed.executable.is_file());
        server.join().expect("artifact server");
    }
}
