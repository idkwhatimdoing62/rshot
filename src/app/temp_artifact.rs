use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use xcap::image::{ExtendedColorType, ImageEncoder, RgbaImage, codecs::png::PngEncoder};

pub(super) const TEMP_PNG_DIR_NAME: &str = "rshot-clipboard";
pub(super) const TEMP_PNG_PREFIX: &str = "rshot-";
pub(super) const TEMP_PNG_MAX_AGE: Duration = Duration::from_secs(12 * 60 * 60);
pub(super) const TEMP_PNG_CLEANUP_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
static TEMP_PNG_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) struct ManagedTempArtifact {
    path: PathBuf,
    retained: bool,
}

impl ManagedTempArtifact {
    pub(super) fn create_png(image: &RgbaImage) -> io::Result<Self> {
        Self::create_png_in(image, &temp_png_dir(), SystemTime::now())
    }

    pub(super) fn create_png_in(
        image: &RgbaImage,
        directory: &Path,
        now: SystemTime,
    ) -> io::Result<Self> {
        let (path, file) = allocate_unique_file(directory, now)?;
        let result = (|| -> io::Result<()> {
            let mut writer = BufWriter::new(file);
            PngEncoder::new(&mut writer)
                .write_image(
                    image.as_raw(),
                    image.width(),
                    image.height(),
                    ExtendedColorType::Rgba8,
                )
                .map_err(io::Error::other)?;
            writer.flush()
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        Ok(Self {
            path,
            retained: false,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn retain(&mut self) {
        self.retained = true;
    }
}

impl Drop for ManagedTempArtifact {
    fn drop(&mut self) {
        if self.retained {
            return;
        }
        if let Err(error) = fs::remove_file(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "event=temp_artifact_reclaim_failed path={} detail={error}",
                self.path.display()
            );
        }
    }
}

pub(super) struct TempArtifactLifecycle {
    last_attempt: Option<Instant>,
    running: Arc<AtomicBool>,
}

impl Default for TempArtifactLifecycle {
    fn default() -> Self {
        Self {
            last_attempt: None,
            running: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl TempArtifactLifecycle {
    pub(super) fn tick(&mut self, now: Instant) {
        if !cleanup_due(self.last_attempt, now) || self.running.swap(true, Ordering::AcqRel) {
            return;
        }
        self.last_attempt = Some(now);
        let running = Arc::clone(&self.running);
        if let Err(error) = std::thread::Builder::new()
            .name(String::from("rshot-temp-cleanup"))
            .spawn(move || {
                let _reset = RunningReset(running);
                match cleanup_expired(SystemTime::now()) {
                    Ok(report) if report.failures > 0 => eprintln!(
                        "event=temp_artifact_cleanup_partial deleted={} failures={}",
                        report.deleted, report.failures
                    ),
                    Ok(_) => {}
                    Err(error) => eprintln!("event=temp_artifact_cleanup_failed detail={error}"),
                }
            })
        {
            self.running.store(false, Ordering::Release);
            eprintln!("event=temp_artifact_cleanup_spawn_failed detail={error}");
        }
    }
}

struct RunningReset(Arc<AtomicBool>);

impl Drop for RunningReset {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CleanupReport {
    pub(super) deleted: usize,
    pub(super) failures: usize,
}

fn cleanup_due(last_attempt: Option<Instant>, now: Instant) -> bool {
    last_attempt.is_none_or(|last| now.saturating_duration_since(last) >= TEMP_PNG_CLEANUP_INTERVAL)
}

fn temp_png_dir() -> PathBuf {
    std::env::temp_dir().join(TEMP_PNG_DIR_NAME)
}

fn verify_directory(directory: &Path, create: bool) -> io::Result<()> {
    if create {
        match fs::create_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.file_type().is_dir()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "rshot 临时图片目录不是安全的普通目录",
        ));
    }
    Ok(())
}

fn allocate_unique_file(directory: &Path, now: SystemTime) -> io::Result<(PathBuf, File)> {
    verify_directory(directory, true)?;
    let nanos = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for _ in 0..64 {
        let sequence = TEMP_PNG_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(
            "{TEMP_PNG_PREFIX}{}-{nanos}-{sequence}.png",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "无法分配唯一的 rshot 临时图片文件名",
    ))
}

fn is_managed_png(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(body) = name
        .strip_prefix(TEMP_PNG_PREFIX)
        .and_then(|name| name.strip_suffix(".png"))
    else {
        return false;
    };
    let mut parts = body.split('-');
    let number = |part: Option<&str>| {
        part.is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    number(parts.next()) && number(parts.next()) && number(parts.next()) && parts.next().is_none()
}

fn safe_regular_file(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

fn expired(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified)
        .is_ok_and(|age| age >= TEMP_PNG_MAX_AGE)
}

pub(super) fn cleanup_expired_in(
    directory: &Path,
    now: SystemTime,
    protected: &[PathBuf],
) -> io::Result<CleanupReport> {
    match verify_directory(directory, false) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(CleanupReport::default());
        }
        Err(error) => return Err(error),
    }
    let mut report = CleanupReport::default();
    for entry in fs::read_dir(directory)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                report.failures += 1;
                continue;
            }
        };
        let path = entry.path();
        if !is_managed_png(&path) || protected.iter().any(|item| item == &path) {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            report.failures += 1;
            continue;
        };
        if !safe_regular_file(&metadata)
            || !metadata
                .modified()
                .is_ok_and(|modified| expired(modified, now))
        {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => report.deleted += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => report.failures += 1,
        }
    }
    Ok(report)
}

fn cleanup_expired(now: SystemTime) -> io::Result<CleanupReport> {
    let protected = super::clipboard::current_clipboard_file_paths()?;
    let mut report = cleanup_expired_in(&temp_png_dir(), now, &protected)?;
    let legacy = std::env::temp_dir().join("rshot.png");
    if !protected.iter().any(|item| item == &legacy)
        && let Ok(metadata) = fs::symlink_metadata(&legacy)
        && safe_regular_file(&metadata)
        && metadata
            .modified()
            .is_ok_and(|modified| expired(modified, now))
    {
        match fs::remove_file(legacy) {
            Ok(()) => report.deleted += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => report.failures += 1,
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::FileTimes;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "rshot-artifact-test-{}-{}",
                std::process::id(),
                TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn create_file(path: &Path, modified: SystemTime) {
        fs::write(path, b"test").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(modified))
            .unwrap();
    }

    #[test]
    fn artifact_is_reclaimed_unless_retained() {
        let directory = TestDirectory::new();
        let image = RgbaImage::new(2, 2);
        let artifact =
            ManagedTempArtifact::create_png_in(&image, &directory.0, SystemTime::now()).unwrap();
        let path = artifact.path().to_owned();
        drop(artifact);
        assert!(!path.exists());

        let mut artifact =
            ManagedTempArtifact::create_png_in(&image, &directory.0, SystemTime::now()).unwrap();
        let path = artifact.path().to_owned();
        artifact.retain();
        drop(artifact);
        assert!(path.exists());
    }

    #[test]
    fn cleanup_only_removes_expired_unprotected_managed_files() {
        let directory = TestDirectory::new();
        let now = SystemTime::now();
        let old = now - Duration::from_secs(13 * 60 * 60);
        let fresh = now - Duration::from_secs(60 * 60);
        let expired = directory.0.join("rshot-1-2-3.png");
        let protected = directory.0.join("rshot-1-2-4.png");
        let recent = directory.0.join("rshot-1-2-5.png");
        let foreign = directory.0.join("other.png");
        create_file(&expired, old);
        create_file(&protected, old);
        create_file(&recent, fresh);
        create_file(&foreign, old);

        let report =
            cleanup_expired_in(&directory.0, now, std::slice::from_ref(&protected)).unwrap();
        assert_eq!(
            report,
            CleanupReport {
                deleted: 1,
                failures: 0
            }
        );
        assert!(!expired.exists());
        assert!(protected.exists());
        assert!(recent.exists());
        assert!(foreign.exists());
    }

    #[test]
    fn cleanup_schedule_starts_immediately_then_waits_twelve_hours() {
        let start = Instant::now();
        assert!(cleanup_due(None, start));
        assert!(!cleanup_due(Some(start), start));
        assert!(!cleanup_due(
            Some(start),
            start + TEMP_PNG_CLEANUP_INTERVAL - Duration::from_nanos(1)
        ));
        assert!(cleanup_due(Some(start), start + TEMP_PNG_CLEANUP_INTERVAL));
    }
}
