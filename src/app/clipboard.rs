use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, POINT};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::{DROPFILES, DragQueryFileW, HDROP};
use windows::core::BOOL;
use xcap::image::{ExtendedColorType, ImageEncoder, RgbaImage, codecs::png::PngEncoder};

pub(super) const TEMP_PNG_DIR_NAME: &str = "rshot-clipboard";
pub(super) const TEMP_PNG_PREFIX: &str = "rshot-";
pub(super) const TEMP_PNG_MAX_AGE: Duration = Duration::from_secs(12 * 60 * 60);
pub(super) const TEMP_PNG_CLEANUP_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);
pub(super) const FILE_ATTRIBUTE_REPARSE_POINT_MASK: u32 = 0x400;
static TEMP_PNG_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CLIPBOARD_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn lock_clipboard() -> MutexGuard<'static, ()> {
    CLIPBOARD_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) fn unicode_text_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

pub(super) fn text_to_clipboard(text: &str, owner: HWND) -> bool {
    let bytes = unicode_text_bytes(text);
    let _clipboard_guard = lock_clipboard();
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            return false;
        }
        if EmptyClipboard().is_err() {
            let _ = CloseClipboard();
            return false;
        }
        let success = global_from_bytes(&bytes).is_some_and(|h| {
            if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0))).is_ok() {
                true
            } else {
                let _ = GlobalFree(Some(h));
                false
            }
        });
        let clipboard_closed = CloseClipboard().is_ok();
        success && clipboard_closed
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PublishedImageFormats {
    dib: bool,
    file: bool,
}

impl PublishedImageFormats {
    pub(super) const fn new(dib: bool, file: bool) -> Self {
        Self { dib, file }
    }

    pub(super) const fn any(self) -> bool {
        self.dib || self.file
    }

    pub(super) const fn dib(self) -> bool {
        self.dib
    }

    pub(super) const fn file(self) -> bool {
        self.file
    }

    pub(super) const fn description(self) -> &'static str {
        match (self.dib(), self.file()) {
            (true, true) => "位图（CF_DIB）和 PNG 文件（CF_HDROP）",
            (true, false) => "位图（CF_DIB）",
            (false, true) => "PNG 文件（CF_HDROP）",
            (false, false) => "无",
        }
    }
}

#[derive(Debug)]
pub(super) struct ImageClipboardOutcome {
    published: PublishedImageFormats,
    clipboard_closed: bool,
    failures: Vec<String>,
}

impl ImageClipboardOutcome {
    pub(super) const fn succeeded(&self) -> bool {
        image_clipboard_publish_succeeded(self.published, self.clipboard_closed)
    }

    pub(super) const fn published(&self) -> PublishedImageFormats {
        self.published
    }

    pub(super) fn failure_message(&self) -> String {
        if self.failures.is_empty() {
            "未能写入任何剪贴板格式。".to_owned()
        } else {
            self.failures.join("\n")
        }
    }
}

pub(super) const fn image_clipboard_publish_succeeded(
    published: PublishedImageFormats,
    clipboard_closed: bool,
) -> bool {
    clipboard_closed && published.any()
}

pub(super) fn temp_cleanup_due(last_attempt: Option<Instant>, now: Instant) -> bool {
    last_attempt.is_none_or(|last| now.saturating_duration_since(last) >= TEMP_PNG_CLEANUP_INTERVAL)
}

pub(super) fn claim_temp_cleanup_slot(last_attempt: &mut Option<Instant>, now: Instant) -> bool {
    if !temp_cleanup_due(*last_attempt, now) {
        return false;
    }
    // 先记录本次尝试，避免权限或线程错误时每 120ms 重试并持续占用 CPU。
    *last_attempt = Some(now);
    true
}

pub(super) fn temp_png_dir() -> PathBuf {
    std::env::temp_dir().join(TEMP_PNG_DIR_NAME)
}

pub(super) fn verify_temp_png_dir(dir: &Path, create: bool) -> io::Result<()> {
    if create {
        match fs::create_dir(dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    let metadata = fs::symlink_metadata(dir)?;
    if !metadata.file_type().is_dir()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT_MASK != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "rshot 临时图片目录不是安全的普通目录",
        ));
    }
    Ok(())
}

pub(super) fn allocate_unique_temp_png_in(
    dir: &Path,
    now: SystemTime,
) -> io::Result<(PathBuf, File)> {
    verify_temp_png_dir(dir, true)?;
    let unix_nanos = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for _ in 0..64 {
        let sequence = TEMP_PNG_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "{TEMP_PNG_PREFIX}{}-{unix_nanos}-{sequence}.png",
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

pub(super) fn write_unique_temp_png_in(
    img: &RgbaImage,
    dir: &Path,
    now: SystemTime,
) -> io::Result<PathBuf> {
    let (path, file) = allocate_unique_temp_png_in(dir, now)?;
    let result = (|| -> io::Result<()> {
        let mut writer = BufWriter::new(file);
        PngEncoder::new(&mut writer)
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                ExtendedColorType::Rgba8,
            )
            .map_err(io::Error::other)?;
        writer.flush()
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

pub(super) fn write_unique_temp_png(img: &RgbaImage) -> io::Result<PathBuf> {
    write_unique_temp_png_in(img, &temp_png_dir(), SystemTime::now())
}

pub(super) fn is_managed_temp_png(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(parts) = name
        .strip_prefix(TEMP_PNG_PREFIX)
        .and_then(|name| name.strip_suffix(".png"))
    else {
        return false;
    };
    let mut parts = parts.split('-');
    let valid_number = |part: Option<&str>| {
        part.is_some_and(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    };
    valid_number(parts.next())
        && valid_number(parts.next())
        && valid_number(parts.next())
        && parts.next().is_none()
}

pub(super) fn is_safe_regular_file(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_file()
        && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT_MASK == 0
}

pub(super) fn is_expired(modified: SystemTime, now: SystemTime, max_age: Duration) -> bool {
    now.duration_since(modified).is_ok_and(|age| age >= max_age)
}

pub(super) fn current_clipboard_hdrop_paths() -> io::Result<Vec<PathBuf>> {
    let _clipboard_guard = lock_clipboard();
    unsafe {
        OpenClipboard(None).map_err(|error| io::Error::other(error.to_string()))?;
        let result = (|| -> io::Result<Vec<PathBuf>> {
            if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_err() {
                return Ok(Vec::new());
            }
            let handle = GetClipboardData(CF_HDROP.0 as u32)
                .map_err(|error| io::Error::other(error.to_string()))?;
            let hdrop = HDROP(handle.0);
            let count = DragQueryFileW(hdrop, u32::MAX, None);
            let mut paths = Vec::with_capacity(count as usize);
            for index in 0..count {
                let len = DragQueryFileW(hdrop, index, None);
                if len == 0 {
                    return Err(io::Error::other("无法读取剪贴板文件路径长度"));
                }
                let mut buffer = vec![0u16; len as usize + 1];
                let written = DragQueryFileW(hdrop, index, Some(&mut buffer));
                if written == 0 {
                    return Err(io::Error::other("无法读取剪贴板文件路径"));
                }
                paths.push(PathBuf::from(std::ffi::OsString::from_wide(
                    &buffer[..written as usize],
                )));
            }
            Ok(paths)
        })();
        let _ = CloseClipboard();
        result
    }
}

pub(super) fn cleanup_expired_temp_pngs_in(
    dir: &Path,
    now: SystemTime,
    max_age: Duration,
    protected_paths: &[PathBuf],
) -> io::Result<usize> {
    match verify_temp_png_dir(dir, false) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    }
    let mut removed = 0;
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if !is_managed_temp_png(&path) || protected_paths.iter().any(|item| item == &path) {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !is_safe_regular_file(&metadata) {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if !is_expired(modified, now, max_age) {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => eprintln!("无法删除过期临时图片 {}：{error}", path.display()),
        }
    }
    Ok(removed)
}

pub(super) fn cleanup_expired_temp_pngs(now: SystemTime) -> io::Result<usize> {
    // 无法读取当前剪贴板时本轮不删除，避免误删仍被 CF_HDROP 引用的文件。
    let protected_paths = current_clipboard_hdrop_paths()?;
    let mut removed =
        cleanup_expired_temp_pngs_in(&temp_png_dir(), now, TEMP_PNG_MAX_AGE, &protected_paths)?;

    // 兼容旧版本固定写入的文件；不扫描系统临时目录，只检查这个精确路径。
    let legacy = std::env::temp_dir().join("rshot.png");
    if !protected_paths.iter().any(|item| item == &legacy) {
        if let Ok(metadata) = fs::symlink_metadata(&legacy) {
            if is_safe_regular_file(&metadata)
                && metadata
                    .modified()
                    .is_ok_and(|modified| is_expired(modified, now, TEMP_PNG_MAX_AGE))
            {
                match fs::remove_file(&legacy) {
                    Ok(()) => removed += 1,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => {
                        eprintln!("无法删除旧版临时图片 {}：{error}", legacy.display())
                    }
                }
            }
        }
    }
    Ok(removed)
}

/// 把截图放进剪贴板，同时挂两种格式：
/// - CF_DIB 位图：微信/Word/画图 等能贴图的程序直接粘。
/// - CF_HDROP 文件：每次使用唯一临时 PNG；未成功发布的文件立即删除。
///
/// 返回实际发布成功的格式；没有可用格式或剪贴板未正常关闭时，调用方保留会话并提示重试。
pub(super) fn image_to_clipboard(img: &RgbaImage, owner: HWND) -> ImageClipboardOutcome {
    let (png_path, mut failures) = match write_unique_temp_png(img) {
        Ok(path) => (Some(path), Vec::new()),
        Err(error) => (None, vec![format!("创建临时 PNG 失败：{error}")]),
    };
    let hdrop = png_path.as_deref().map(build_hdrop);
    // PNG 编码完成后再构造 DIB，避免两份大块临时数据同时参与编码峰值。
    let dib = build_dib(img);

    let _clipboard_guard = lock_clipboard();
    let mut published = PublishedImageFormats::new(false, false);
    let mut clipboard_closed = false;
    unsafe {
        match OpenClipboard(Some(owner)) {
            Ok(()) => {
                match EmptyClipboard() {
                    Ok(()) => {
                        match publish_clipboard_bytes(CF_DIB.0 as u32, &dib) {
                            Ok(()) => published.dib = true,
                            Err(error) => failures.push(format!("写入位图格式失败：{error}")),
                        }
                        if let Some(bytes) = hdrop.as_deref() {
                            match publish_clipboard_bytes(CF_HDROP.0 as u32, bytes) {
                                Ok(()) => published.file = true,
                                Err(error) => {
                                    failures.push(format!("写入 PNG 文件格式失败：{error}"))
                                }
                            }
                        }
                    }
                    Err(error) => failures.push(format!("清空剪贴板失败：{error}")),
                }
                match CloseClipboard() {
                    Ok(()) => clipboard_closed = true,
                    Err(error) => failures.push(format!("关闭剪贴板失败：{error}")),
                }
            }
            Err(error) => failures.push(format!("打开剪贴板失败：{error}")),
        }
    }
    if !published.file()
        && let Some(path) = png_path
    {
        match fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                eprintln!("无法删除未发布的临时 PNG {}：{error}", path.display());
                failures.push(format!("删除未发布的临时 PNG 失败：{error}"));
            }
            _ => {}
        }
    }
    ImageClipboardOutcome {
        published,
        clipboard_closed,
        failures,
    }
}

/// SetClipboardData 成功后 HGLOBAL 所有权转给系统；失败时仍由本进程释放。
unsafe fn publish_clipboard_bytes(format: u32, data: &[u8]) -> Result<(), String> {
    let Some(memory) = (unsafe { global_from_bytes(data) }) else {
        return Err("分配剪贴板内存失败".to_owned());
    };
    match unsafe { SetClipboardData(format, Some(HANDLE(memory.0))) } {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = unsafe { GlobalFree(Some(memory)) };
            Err(error.to_string())
        }
    }
}

/// 组一个 24 位 BI_RGB 的 DIB：40 字节 BITMAPINFOHEADER + 自底向上、每行补齐 4 字节的 BGR 像素。
pub(super) fn build_dib(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let stride = (w * 3 + 3) & !3; // 每行补齐到 4 字节边界（DIB 要求）
    let mut out = Vec::with_capacity(40 + stride * h);
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&(w as i32).to_le_bytes()); // biWidth
    out.extend_from_slice(&(h as i32).to_le_bytes()); // biHeight 正=自底向上
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&24u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    out.extend_from_slice(&((stride * h) as u32).to_le_bytes()); // biSizeImage
    out.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    out.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant
    for y in (0..h).rev() {
        let mut row = 0usize;
        for x in 0..w {
            let px = img.get_pixel(x as u32, y as u32).0;
            out.push(px[2]); // B
            out.push(px[1]); // G
            out.push(px[0]); // R
            row += 3;
        }
        while row < stride {
            out.push(0); // 行尾补齐
            row += 1;
        }
    }
    out
}

/// 组一个 CF_HDROP 数据块：DROPFILES 头 + 宽字符路径 + 双 null 结尾。
pub(super) fn build_hdrop(path: &std::path::Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    let df = DROPFILES {
        pFiles: std::mem::size_of::<DROPFILES>() as u32, // 路径列表相对本头的偏移
        pt: POINT { x: 0, y: 0 },
        fNC: BOOL(0),
        fWide: BOOL(1), // 宽字符路径
    };
    let head = unsafe {
        std::slice::from_raw_parts(
            (&df as *const DROPFILES) as *const u8,
            std::mem::size_of::<DROPFILES>(),
        )
    };
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0); // 路径结尾
    wide.push(0); // 列表结尾（双 null）
    let mut out = Vec::with_capacity(head.len() + wide.len() * 2);
    out.extend_from_slice(head);
    for u in wide {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

/// 把字节拷进一块可移动全局内存，交给剪贴板（SetClipboardData 成功后由系统接管，不能再释放）。
pub(super) unsafe fn global_from_bytes(data: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GHND, data.len()).ok()?;
        let p = GlobalLock(h);
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), p as *mut u8, data.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }
}
