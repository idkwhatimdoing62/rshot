use super::state::CaptureFailureStage;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) const CAPTURE_DIAGNOSTIC_LOG_NAME: &str = "capture-errors.log";
pub(super) const CAPTURE_DIAGNOSTIC_LOG_MAX_BYTES: u64 = 64 * 1024;

pub(super) fn capture_diagnostic_log_path() -> io::Result<PathBuf> {
    let config_path = confy::get_configuration_file_path("RShot", None)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let parent = config_path
        .parent()
        .ok_or_else(|| io::Error::other("配置文件没有父目录"))?;
    Ok(parent.join(CAPTURE_DIAGNOSTIC_LOG_NAME))
}

pub(super) fn capture_failure_log_line(stage: CaptureFailureStage, now: SystemTime) -> String {
    let unix_seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    format!(
        "unix_seconds={unix_seconds} event=capture_failed code={}\n",
        stage.code()
    )
}

pub(super) fn record_capture_failure(stage: CaptureFailureStage) -> io::Result<bool> {
    let path = capture_diagnostic_log_path()?;
    record_capture_failure_in(
        &path,
        stage,
        SystemTime::now(),
        CAPTURE_DIAGNOSTIC_LOG_MAX_BYTES,
    )
}

/// 仅追加固定字段。达到上限后停止写入，用户删除日志后会从空文件重新开始。
pub(super) fn record_capture_failure_in(
    path: &Path,
    stage: CaptureFailureStage,
    now: SystemTime,
    max_bytes: u64,
) -> io::Result<bool> {
    let line = capture_failure_log_line(stage, now);
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
    // 独占打开让多个 rshot 进程不能同时越过大小检查；抢不到锁时本次诊断直接失败。
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
