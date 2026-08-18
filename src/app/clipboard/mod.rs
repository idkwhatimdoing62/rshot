use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, POINT};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::{DROPFILES, DragQueryFileW, HDROP};
use windows::core::BOOL;
use xcap::image::RgbaImage;

use super::temp_artifact::ManagedTempArtifact;

const OPEN_ATTEMPTS: usize = 3;
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(37);
static CLIPBOARD_LOCK: Mutex<()> = Mutex::new(());

fn lock_clipboard() -> MutexGuard<'static, ()> {
    CLIPBOARD_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Copy)]
pub(super) struct ClipboardOwner(HWND);

impl ClipboardOwner {
    pub(super) const fn from_window(window: HWND) -> Self {
        Self(window)
    }
}

pub(super) enum ClipboardContent<'a> {
    Text(&'a str),
    Image(&'a RgbaImage),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum PublishedFormat {
    UnicodeText,
    DibImage,
    FileImage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PublishStage {
    Prepare,
    Open,
    Empty,
    SetFormat,
    Close,
    Complete,
}

impl PublishStage {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::Prepare => "RSH-CLP-001",
            Self::Open => "RSH-CLP-002",
            Self::Empty => "RSH-CLP-003",
            Self::SetFormat => "RSH-CLP-004",
            Self::Close => "RSH-CLP-005",
            Self::Complete => "RSH-CLP-006",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TransactionCertainty {
    Unchanged,
    Certain,
    Uncertain,
}

#[derive(Debug)]
pub(super) struct PublishOutcome {
    stage: PublishStage,
    formats: BTreeSet<PublishedFormat>,
    certainty: TransactionCertainty,
    diagnostics: Vec<String>,
}

impl PublishOutcome {
    pub(super) fn succeeded(&self) -> bool {
        self.stage == PublishStage::Complete && !self.formats.is_empty()
    }

    pub(super) fn formats(&self) -> &BTreeSet<PublishedFormat> {
        &self.formats
    }

    pub(super) const fn stage(&self) -> PublishStage {
        self.stage
    }

    pub(super) const fn certainty(&self) -> TransactionCertainty {
        self.certainty
    }

    pub(super) fn format_description(&self) -> &'static str {
        match (
            self.formats.contains(&PublishedFormat::DibImage),
            self.formats.contains(&PublishedFormat::FileImage),
            self.formats.contains(&PublishedFormat::UnicodeText),
        ) {
            (true, true, _) => "位图（CF_DIB）和 PNG 文件（CF_HDROP）",
            (true, false, _) => "位图（CF_DIB）",
            (false, true, _) => "PNG 文件（CF_HDROP）",
            (_, _, true) => "Unicode 文本（CF_UNICODETEXT）",
            _ => "无",
        }
    }

    pub(super) fn diagnostic_message(&self) -> String {
        if self.diagnostics.is_empty() {
            format!("剪贴板发布失败，阶段：{:?}", self.stage)
        } else {
            self.diagnostics.join("\n")
        }
    }

    pub(super) fn has_diagnostics(&self) -> bool {
        !self.diagnostics.is_empty()
    }
}

#[derive(Default)]
pub(super) struct ClipboardPublisher;

impl ClipboardPublisher {
    pub(super) fn publish(
        &self,
        content: ClipboardContent<'_>,
        owner: ClipboardOwner,
    ) -> PublishOutcome {
        let prepared = match content {
            ClipboardContent::Text(text) => PreparedPublish::text(text),
            ClipboardContent::Image(image) => PreparedPublish::image(image),
        };
        match prepared {
            Ok(prepared) => publish_prepared(prepared, owner),
            Err(detail) => PublishOutcome {
                stage: PublishStage::Prepare,
                formats: BTreeSet::new(),
                certainty: TransactionCertainty::Unchanged,
                diagnostics: vec![detail],
            },
        }
    }
}

pub(super) fn is_clipboard_self_test_invocation() -> bool {
    std::env::args_os().any(|argument| argument == "--rshot-clipboard-self-test")
}

pub(super) fn run_clipboard_self_test() -> Result<(), String> {
    let publisher = ClipboardPublisher;
    let image = RgbaImage::from_pixel(2, 2, xcap::image::Rgba([18, 52, 86, 255]));
    let outcome = publisher.publish(
        ClipboardContent::Image(&image),
        ClipboardOwner::from_window(HWND::default()),
    );
    if !outcome.succeeded()
        || !outcome.formats().contains(&PublishedFormat::DibImage)
        || !outcome.formats().contains(&PublishedFormat::FileImage)
    {
        return Err(format!(
            "clipboard self-test publish failed at {:?}: {}",
            outcome.stage(),
            outcome.diagnostic_message()
        ));
    }
    let paths = current_clipboard_file_paths().map_err(|error| error.to_string())?;
    if paths.len() != 1 || !paths[0].is_file() {
        return Err(String::from(
            "clipboard self-test could not consume the published file format",
        ));
    }
    unsafe {
        if IsClipboardFormatAvailable(CF_DIB.0 as u32).is_err() {
            return Err(String::from(
                "clipboard self-test could not consume the published DIB format",
            ));
        }
    }
    Ok(())
}

struct PreparedFormat {
    kind: PublishedFormat,
    clipboard_format: u32,
    memory: Option<HGLOBAL>,
}

impl PreparedFormat {
    fn new(kind: PublishedFormat, clipboard_format: u32, bytes: &[u8]) -> Result<Self, String> {
        let memory = unsafe { global_from_bytes(bytes) }
            .ok_or_else(|| format!("为 {kind:?} 分配剪贴板内存失败"))?;
        Ok(Self {
            kind,
            clipboard_format,
            memory: Some(memory),
        })
    }

    unsafe fn transfer(&mut self) -> Result<(), String> {
        let memory = self.memory.expect("prepared memory must exist");
        unsafe { SetClipboardData(self.clipboard_format, Some(HANDLE(memory.0))) }
            .map_err(|error| format!("写入 {:?} 失败：{error}", self.kind))?;
        self.memory = None;
        Ok(())
    }
}

impl Drop for PreparedFormat {
    fn drop(&mut self) {
        if let Some(memory) = self.memory.take() {
            unsafe {
                let _ = GlobalFree(Some(memory));
            }
        }
    }
}

struct PreparedPublish {
    formats: Vec<PreparedFormat>,
    artifact: Option<ManagedTempArtifact>,
    diagnostics: Vec<String>,
}

impl PreparedPublish {
    fn text(text: &str) -> Result<Self, String> {
        let bytes = unicode_text_bytes(text);
        Ok(Self {
            formats: vec![PreparedFormat::new(
                PublishedFormat::UnicodeText,
                CF_UNICODETEXT.0 as u32,
                &bytes,
            )?],
            artifact: None,
            diagnostics: Vec::new(),
        })
    }

    fn image(image: &RgbaImage) -> Result<Self, String> {
        let mut formats = Vec::new();
        let mut diagnostics = Vec::new();
        let dib = build_dib(image);
        match PreparedFormat::new(PublishedFormat::DibImage, CF_DIB.0 as u32, &dib) {
            Ok(format) => formats.push(format),
            Err(error) => diagnostics.push(error),
        }
        let mut artifact = match ManagedTempArtifact::create_png(image) {
            Ok(artifact) => Some(artifact),
            Err(error) => {
                diagnostics.push(format!("创建临时 PNG 失败：{error}"));
                None
            }
        };
        if let Some(file) = artifact.as_ref() {
            let bytes = build_hdrop(file.path());
            match PreparedFormat::new(PublishedFormat::FileImage, CF_HDROP.0 as u32, &bytes) {
                Ok(format) => formats.push(format),
                Err(error) => diagnostics.push(error),
            }
        }
        if formats.is_empty() {
            return Err(diagnostics.join("\n"));
        }
        Ok(Self {
            formats,
            artifact: artifact.take(),
            diagnostics,
        })
    }
}

fn publish_prepared(mut prepared: PreparedPublish, owner: ClipboardOwner) -> PublishOutcome {
    let _guard = lock_clipboard();
    execute_transaction(&mut WindowsTransaction, &mut prepared, owner)
}

trait TransactionBackend {
    fn open(&mut self, owner: ClipboardOwner) -> Result<(), String>;
    fn empty(&mut self) -> Result<(), String>;
    fn set(&mut self, format: &mut PreparedFormat) -> Result<(), String>;
    fn close(&mut self) -> Result<(), String>;
    fn wait_before_retry(&mut self);
}

struct WindowsTransaction;

impl TransactionBackend for WindowsTransaction {
    fn open(&mut self, owner: ClipboardOwner) -> Result<(), String> {
        unsafe { OpenClipboard(Some(owner.0)) }.map_err(|error| error.to_string())
    }

    fn empty(&mut self) -> Result<(), String> {
        unsafe { EmptyClipboard() }.map_err(|error| error.to_string())
    }

    fn set(&mut self, format: &mut PreparedFormat) -> Result<(), String> {
        unsafe { format.transfer() }
    }

    fn close(&mut self) -> Result<(), String> {
        unsafe { CloseClipboard() }.map_err(|error| error.to_string())
    }

    fn wait_before_retry(&mut self) {
        thread::sleep(OPEN_RETRY_DELAY);
    }
}

fn execute_transaction(
    backend: &mut impl TransactionBackend,
    prepared: &mut PreparedPublish,
    owner: ClipboardOwner,
) -> PublishOutcome {
    let mut diagnostics = std::mem::take(&mut prepared.diagnostics);
    let mut formats = BTreeSet::new();
    let mut opened = false;
    for attempt in 0..OPEN_ATTEMPTS {
        match backend.open(owner) {
            Ok(()) => {
                opened = true;
                break;
            }
            Err(error) if attempt + 1 < OPEN_ATTEMPTS => {
                diagnostics.push(format!("打开剪贴板第 {} 次失败：{error}", attempt + 1));
                backend.wait_before_retry();
            }
            Err(error) => diagnostics.push(format!("打开剪贴板失败：{error}")),
        }
    }
    if !opened {
        return PublishOutcome {
            stage: PublishStage::Open,
            formats,
            certainty: TransactionCertainty::Unchanged,
            diagnostics,
        };
    }
    if let Err(error) = backend.empty() {
        diagnostics.push(format!("清空剪贴板失败：{error}"));
        let close_ok = backend.close().is_ok();
        return PublishOutcome {
            stage: PublishStage::Empty,
            formats,
            certainty: if close_ok {
                TransactionCertainty::Certain
            } else {
                TransactionCertainty::Uncertain
            },
            diagnostics,
        };
    }
    for format in &mut prepared.formats {
        match backend.set(format) {
            Ok(()) => {
                formats.insert(format.kind);
                if format.kind == PublishedFormat::FileImage
                    && let Some(artifact) = prepared.artifact.as_mut()
                {
                    artifact.retain();
                }
            }
            Err(error) => diagnostics.push(error),
        }
    }
    if let Err(error) = backend.close() {
        diagnostics.push(format!("关闭剪贴板失败：{error}"));
        return PublishOutcome {
            stage: PublishStage::Close,
            formats,
            certainty: TransactionCertainty::Uncertain,
            diagnostics,
        };
    }
    PublishOutcome {
        stage: if formats.is_empty() {
            PublishStage::SetFormat
        } else {
            PublishStage::Complete
        },
        formats,
        certainty: TransactionCertainty::Certain,
        diagnostics,
    }
}

pub(super) fn current_clipboard_file_paths() -> io::Result<Vec<PathBuf>> {
    let _guard = lock_clipboard();
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
                paths.push(PathBuf::from(OsString::from_wide(
                    &buffer[..written as usize],
                )));
            }
            Ok(paths)
        })();
        let close = CloseClipboard();
        if let Err(error) = close {
            return Err(io::Error::other(error.to_string()));
        }
        result
    }
}

pub(super) fn unicode_text_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

pub(super) fn build_dib(image: &RgbaImage) -> Vec<u8> {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let stride = (width * 3 + 3) & !3;
    let mut output = Vec::with_capacity(40 + stride * height);
    output.extend_from_slice(&40u32.to_le_bytes());
    output.extend_from_slice(&(width as i32).to_le_bytes());
    output.extend_from_slice(&(height as i32).to_le_bytes());
    output.extend_from_slice(&1u16.to_le_bytes());
    output.extend_from_slice(&24u16.to_le_bytes());
    output.extend_from_slice(&0u32.to_le_bytes());
    output.extend_from_slice(&((stride * height) as u32).to_le_bytes());
    output.extend_from_slice(&0i32.to_le_bytes());
    output.extend_from_slice(&0i32.to_le_bytes());
    output.extend_from_slice(&0u32.to_le_bytes());
    output.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..height).rev() {
        let row_start = output.len();
        for x in 0..width {
            let pixel = image.get_pixel(x as u32, y as u32).0;
            output.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
        while output.len() - row_start < stride {
            output.push(0);
        }
    }
    output
}

fn build_hdrop(path: &Path) -> Vec<u8> {
    let header = DROPFILES {
        pFiles: std::mem::size_of::<DROPFILES>() as u32,
        pt: POINT { x: 0, y: 0 },
        fNC: BOOL(0),
        fWide: BOOL(1),
    };
    let header = unsafe {
        std::slice::from_raw_parts(
            (&header as *const DROPFILES).cast::<u8>(),
            std::mem::size_of::<DROPFILES>(),
        )
    };
    let mut output = Vec::from(header);
    for unit in path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .chain(std::iter::once(0))
    {
        output.extend_from_slice(&unit.to_le_bytes());
    }
    output
}

unsafe fn global_from_bytes(data: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let memory = GlobalAlloc(GHND, data.len()).ok()?;
        let target = GlobalLock(memory);
        if target.is_null() {
            let _ = GlobalFree(Some(memory));
            return None;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), target.cast::<u8>(), data.len());
        let _ = GlobalUnlock(memory);
        Some(memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeTransaction {
        open_failures: usize,
        open_calls: usize,
        waits: usize,
        empty_ok: bool,
        failed_format: Option<PublishedFormat>,
        close_ok: bool,
    }

    impl TransactionBackend for FakeTransaction {
        fn open(&mut self, _owner: ClipboardOwner) -> Result<(), String> {
            self.open_calls += 1;
            if self.open_calls <= self.open_failures {
                Err(String::from("busy"))
            } else {
                Ok(())
            }
        }

        fn empty(&mut self) -> Result<(), String> {
            self.empty_ok
                .then_some(())
                .ok_or_else(|| String::from("empty"))
        }

        fn set(&mut self, format: &mut PreparedFormat) -> Result<(), String> {
            if self.failed_format == Some(format.kind) {
                Err(String::from("set"))
            } else {
                format.memory = None;
                Ok(())
            }
        }

        fn close(&mut self) -> Result<(), String> {
            self.close_ok
                .then_some(())
                .ok_or_else(|| String::from("close"))
        }

        fn wait_before_retry(&mut self) {
            self.waits += 1;
        }
    }

    fn prepared(kinds: &[PublishedFormat]) -> PreparedPublish {
        PreparedPublish {
            formats: kinds
                .iter()
                .map(|kind| PreparedFormat {
                    kind: *kind,
                    clipboard_format: 0,
                    memory: None,
                })
                .collect(),
            artifact: None,
            diagnostics: Vec::new(),
        }
    }

    fn fake() -> FakeTransaction {
        FakeTransaction {
            open_failures: 0,
            open_calls: 0,
            waits: 0,
            empty_ok: true,
            failed_format: None,
            close_ok: true,
        }
    }

    #[test]
    fn open_is_retried_at_most_three_times_without_real_sleep() {
        let mut backend = fake();
        backend.open_failures = 3;
        let mut data = prepared(&[PublishedFormat::UnicodeText]);
        let outcome = execute_transaction(&mut backend, &mut data, ClipboardOwner(HWND::default()));
        assert_eq!(backend.open_calls, 3);
        assert_eq!(backend.waits, 2);
        assert_eq!(outcome.stage(), PublishStage::Open);
        assert_eq!(outcome.certainty(), TransactionCertainty::Unchanged);
    }

    #[test]
    fn partial_image_publish_is_success_after_a_certain_close() {
        let mut backend = fake();
        backend.failed_format = Some(PublishedFormat::FileImage);
        let mut data = prepared(&[PublishedFormat::DibImage, PublishedFormat::FileImage]);
        let outcome = execute_transaction(&mut backend, &mut data, ClipboardOwner(HWND::default()));
        assert!(outcome.succeeded());
        assert_eq!(
            outcome.formats(),
            &BTreeSet::from([PublishedFormat::DibImage])
        );
    }

    #[test]
    fn close_failure_reports_uncertain_failure_even_after_set() {
        let mut backend = fake();
        backend.close_ok = false;
        let mut data = prepared(&[PublishedFormat::DibImage]);
        let outcome = execute_transaction(&mut backend, &mut data, ClipboardOwner(HWND::default()));
        assert!(!outcome.succeeded());
        assert_eq!(outcome.stage(), PublishStage::Close);
        assert_eq!(outcome.certainty(), TransactionCertainty::Uncertain);
        assert!(outcome.formats().contains(&PublishedFormat::DibImage));
    }

    #[test]
    fn unicode_text_is_null_terminated_utf16() {
        let bytes = unicode_text_bytes("中文😄\nabc");
        assert_eq!(&bytes[bytes.len() - 2..], &[0, 0]);
        let units = bytes[..bytes.len() - 2]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        assert_eq!(String::from_utf16(&units).unwrap(), "中文😄\nabc");
    }

    #[test]
    fn dib_has_expected_header_and_bgr_pixels() {
        let image = RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
        let dib = build_dib(&image);
        assert_eq!(dib.len(), 48);
        assert_eq!(&dib[0..4], &40u32.to_le_bytes());
        assert_eq!(&dib[40..46], &[0, 0, 255, 0, 255, 0]);
    }
}
