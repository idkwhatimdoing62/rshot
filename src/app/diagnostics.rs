use super::clipboard::PublishStage;
use super::ocr::OcrFailureStage;
use super::pinned::PinFailureStage;
use super::state::{CaptureFailureStage, SessionFailureStage};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) const DIAGNOSTIC_LOG_NAME: &str = "rshot-diagnostics.log";
pub(super) const DIAGNOSTIC_LOG_MAX_BYTES: u64 = 64 * 1024;

pub(super) enum DiagnosticEvent {
    Capture(CaptureFailureStage),
    Clipboard(PublishStage),
    Ocr(OcrFailureStage),
    Pin(PinFailureStage),
    Render(SessionFailureStage),
}

impl DiagnosticEvent {
    fn fields(&self) -> (&'static str, &'static str) {
        match self {
            Self::Capture(stage) => ("capture_failed", stage.code()),
            Self::Clipboard(stage) => ("clipboard_failed", stage.code()),
            Self::Ocr(stage) => ("ocr_failed", stage.code()),
            Self::Pin(stage) => ("pin_failed", stage.code()),
            Self::Render(stage) => ("render_failed", stage.code()),
        }
    }
}

pub(super) fn diagnostic_log_path() -> io::Result<PathBuf> {
    let config_path = confy::get_configuration_file_path("RShot", None)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let parent = config_path
        .parent()
        .ok_or_else(|| io::Error::other("配置文件没有父目录"))?;
    Ok(parent.join(DIAGNOSTIC_LOG_NAME))
}

fn diagnostic_log_line(event: &str, code: &str, now: SystemTime) -> String {
    let unix_seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    format!(
        "unix_seconds={unix_seconds} version={} event={event} code={code}\n",
        env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
pub(super) fn capture_failure_log_line(stage: CaptureFailureStage, now: SystemTime) -> String {
    diagnostic_log_line("capture_failed", stage.code(), now)
}

pub(super) fn record_capture_failure(stage: CaptureFailureStage) -> io::Result<bool> {
    record_diagnostic(DiagnosticEvent::Capture(stage))
}

pub(super) fn record_diagnostic(event: DiagnosticEvent) -> io::Result<bool> {
    let (event, code) = event.fields();
    let path = diagnostic_log_path()?;
    record_diagnostic_in(
        &path,
        event,
        code,
        SystemTime::now(),
        DIAGNOSTIC_LOG_MAX_BYTES,
    )
}

#[cfg(test)]
pub(super) fn record_capture_failure_in(
    path: &Path,
    stage: CaptureFailureStage,
    now: SystemTime,
    max_bytes: u64,
) -> io::Result<bool> {
    record_line_in(path, &capture_failure_log_line(stage, now), max_bytes)
}

fn record_diagnostic_in(
    path: &Path,
    event: &str,
    code: &str,
    now: SystemTime,
    max_bytes: u64,
) -> io::Result<bool> {
    record_line_in(path, &diagnostic_log_line(event, code, now), max_bytes)
}

fn record_line_in(path: &Path, line: &str, max_bytes: u64) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "诊断日志路径不是普通文件",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .share_mode(0)
        .open(path)?;
    let current_len = file.metadata()?.len();
    if current_len.saturating_add(line.len() as u64) > max_bytes {
        return Ok(false);
    }
    file.write_all(line.as_bytes())?;
    Ok(true)
}

pub(super) fn try_export_diagnostics_invocation() -> Option<Result<PathBuf, String>> {
    let mut arguments = std::env::args_os();
    while let Some(argument) = arguments.next() {
        if argument == "--export-diagnostics" {
            let Some(path) = arguments.next() else {
                return Some(Err(String::from(
                    "--export-diagnostics requires an output path",
                )));
            };
            return Some(
                export_diagnostics_to(Path::new(&path)).map_err(|error| error.to_string()),
            );
        }
    }
    None
}

pub(super) fn export_diagnostics_to(path: &Path) -> io::Result<PathBuf> {
    export_diagnostics_from(diagnostic_log_path().ok().as_deref(), path)
}

fn export_diagnostics_from(log_path: Option<&Path>, path: &Path) -> io::Result<PathBuf> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let log = log_path
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|log| sanitize_log(&log))
        .unwrap_or_default();
    let report = format!(
        "rshot_diagnostics_v1\nversion={}\nos={}\narch={}\nlog_begin\n{}log_end\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        log
    );
    fs::write(path, report)?;
    Ok(path.to_owned())
}

fn sanitize_log(log: &str) -> String {
    log.lines()
        .filter(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 4 {
                return false;
            }
            let valid_time = fields[0]
                .strip_prefix("unix_seconds=")
                .is_some_and(|value| value.bytes().all(|byte| byte.is_ascii_digit()));
            let valid_version = fields[1].strip_prefix("version=").is_some_and(|value| {
                !value.is_empty()
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b".-+".contains(&byte))
            });
            let valid_event = matches!(
                fields[2],
                "event=capture_failed"
                    | "event=clipboard_failed"
                    | "event=ocr_failed"
                    | "event=pin_failed"
                    | "event=render_failed"
            );
            let valid_code = fields[3]
                .strip_prefix("code=RSH-")
                .and_then(|value| value.split_once('-'))
                .is_some_and(|(domain, number)| {
                    matches!(domain, "CAP" | "CLP" | "OCR" | "PIN" | "RND")
                        && number.len() == 3
                        && number.bytes().all(|byte| byte.is_ascii_digit())
                });
            valid_time && valid_version && valid_event && valid_code
        })
        .map(|line| format!("{line}\n"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rshot-diagnostic-test-{}-{}-{name}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn exported_report_contains_only_fixed_metadata_and_sanitized_log_fields() {
        let log_path = test_path("log");
        record_diagnostic_in(
            &log_path,
            "clipboard_failed",
            "RSH-CLP-002",
            UNIX_EPOCH + std::time::Duration::from_secs(12),
            4096,
        )
        .unwrap();
        let export_path = test_path("export");
        export_diagnostics_from(Some(&log_path), &export_path).unwrap();
        let exported = fs::read_to_string(&export_path).unwrap();
        assert!(exported.contains("RSH-CLP-002"));
        assert!(!exported.contains(std::env::temp_dir().to_string_lossy().as_ref()));
        let _ = fs::remove_file(log_path);
        let _ = fs::remove_file(export_path);
    }

    #[test]
    fn export_discards_unknown_or_privacy_unsafe_log_lines() {
        let log_path = test_path("unsafe-log");
        fs::write(
            &log_path,
            "user_path=C:\\secret\\shot.png\nunix_seconds=12 version=0.3.0 event=ocr_failed code=RSH-OCR-002 detail=private\nunix_seconds=13 version=0.3.0 event=render_failed code=RSH-RND-004\n",
        )
        .unwrap();
        let export_path = test_path("safe-export");
        export_diagnostics_from(Some(&log_path), &export_path).unwrap();
        let exported = fs::read_to_string(&export_path).unwrap();
        assert!(!exported.contains("secret"));
        assert!(!exported.contains("private"));
        assert!(exported.contains("RSH-RND-004"));
        let _ = fs::remove_file(log_path);
        let _ = fs::remove_file(export_path);
    }
}
