mod engine;
mod operation;
mod worker;

use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use xcap::image::RgbaImage;

pub(super) use engine::{OcrBackend, OcrFallbackReason, OcrRecognition};
pub(super) use operation::{OcrEvent, OcrOperation, OcrSessionId};

const WINDOWS_OCR_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) struct OcrRequest<'a> {
    pub(super) frozen_image: &'a RgbaImage,
    pub(super) selection: Option<((i32, i32), (i32, i32))>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OcrFailureStage {
    InvalidInput,
    ModelWorkerUnavailable,
    ModelRecognitionFailed,
    WindowsOcrUnavailable,
    WindowsRecognitionFailed,
    ReadResult,
    WindowsTimeout,
    OperationTimeout,
}

impl OcrFailureStage {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "RSH-OCR-001",
            Self::ModelWorkerUnavailable => "RSH-OCR-002",
            Self::ModelRecognitionFailed => "RSH-OCR-003",
            Self::WindowsOcrUnavailable => "RSH-OCR-004",
            Self::WindowsRecognitionFailed => "RSH-OCR-005",
            Self::ReadResult => "RSH-OCR-006",
            Self::WindowsTimeout => "RSH-OCR-007",
            Self::OperationTimeout => "RSH-OCR-008",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct OcrFailure {
    stage: OcrFailureStage,
    model_stage: OcrFailureStage,
    model_failure: Option<String>,
    windows_failure: String,
}

impl OcrFailure {
    pub(super) fn stage(&self) -> OcrFailureStage {
        self.stage
    }

    pub(super) fn model_stage(&self) -> OcrFailureStage {
        self.model_stage
    }
}

impl fmt::Display for OcrFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(model) = &self.model_failure {
            writeln!(f, "高精度 OCR：{model}")?;
        }
        write!(f, "系统 OCR 回退：{}", self.windows_failure)
    }
}

trait RecognitionAdapter {
    fn recognize(&self, request: &OcrRequest<'_>) -> Result<String, String>;
}

struct WorkerAdapter;
struct WindowsAdapter;

impl RecognitionAdapter for WorkerAdapter {
    fn recognize(&self, request: &OcrRequest<'_>) -> Result<String, String> {
        worker::recognize_with_worker(request.frozen_image, request.selection)
    }
}

impl RecognitionAdapter for WindowsAdapter {
    fn recognize(&self, request: &OcrRequest<'_>) -> Result<String, String> {
        let image = request.frozen_image.clone();
        let selection = request.selection;
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name(String::from("rshot-windows-ocr"))
            .spawn(move || {
                let result = super::windows_adapter::WinRtApartment::initialize()
                    .map_err(|error| format!("无法初始化系统 OCR 线程：{error}"))
                    .and_then(|_apartment| engine::recognize_image_text_windows(&image, selection));
                let _ = sender.send(result);
            })
            .map_err(|error| format!("无法启动系统 OCR 线程：{error}"))?;
        receiver
            .recv_timeout(WINDOWS_OCR_TIMEOUT)
            .map_err(|_| String::from("系统 OCR 超时"))?
    }
}

fn valid_request(request: &OcrRequest<'_>) -> bool {
    if request.frozen_image.width() == 0 || request.frozen_image.height() == 0 {
        return false;
    }
    request.selection.is_none_or(|(start, end)| {
        start.0 != end.0
            && start.1 != end.1
            && start.0.max(end.0) > 0
            && start.1.max(end.1) > 0
            && start.0.min(end.0) < request.frozen_image.width() as i32
            && start.1.min(end.1) < request.frozen_image.height() as i32
    })
}

fn classify_model_failure(detail: &str) -> OcrFailureStage {
    if detail.contains("识别失败") || detail.contains("推理") {
        OcrFailureStage::ModelRecognitionFailed
    } else {
        OcrFailureStage::ModelWorkerUnavailable
    }
}

fn classify_windows_failure(detail: &str) -> OcrFailureStage {
    if detail.contains("系统 OCR 超时") {
        OcrFailureStage::WindowsTimeout
    } else if detail.contains("读取 OCR") || detail.contains("读取当前 OCR") {
        OcrFailureStage::ReadResult
    } else if detail.contains("创建系统 OCR") || detail.contains("语言包") {
        OcrFailureStage::WindowsOcrUnavailable
    } else {
        OcrFailureStage::WindowsRecognitionFailed
    }
}

fn recognize_with(
    request: OcrRequest<'_>,
    model: &dyn RecognitionAdapter,
    windows: &dyn RecognitionAdapter,
) -> Result<OcrRecognition, OcrFailure> {
    if !valid_request(&request) {
        return Err(OcrFailure {
            stage: OcrFailureStage::InvalidInput,
            model_stage: OcrFailureStage::ModelWorkerUnavailable,
            model_failure: None,
            windows_failure: String::from("选区尺寸无效"),
        });
    }
    let (model_failure, model_stage, fallback_reason) = match model.recognize(&request) {
        Ok(text) if !text.trim().is_empty() => {
            return Ok(OcrRecognition {
                text: text.trim().to_owned(),
                backend: OcrBackend::PpOcrV6,
                fallback_reason: None,
            });
        }
        Ok(_) => (
            String::from("高精度 OCR 未识别到文字"),
            OcrFailureStage::ModelRecognitionFailed,
            OcrFallbackReason::ModelReturnedNoText,
        ),
        Err(error) => {
            let stage = classify_model_failure(&error);
            (error, stage, OcrFallbackReason::ModelUnavailable)
        }
    };
    windows
        .recognize(&request)
        .map(|text| OcrRecognition {
            text,
            backend: OcrBackend::Windows,
            fallback_reason: Some(fallback_reason),
        })
        .map_err(|windows_failure| {
            let stage = classify_windows_failure(&windows_failure);
            OcrFailure {
                stage,
                model_stage,
                model_failure: Some(model_failure),
                windows_failure,
            }
        })
}

pub(super) fn recognize(request: OcrRequest<'_>) -> Result<OcrRecognition, OcrFailure> {
    recognize_with(request, &WorkerAdapter, &WindowsAdapter)
}

pub(super) fn try_run_process_role() -> Result<bool, String> {
    if let Some(result) = try_run_corpus_invocation() {
        result?;
        return Ok(true);
    }
    if worker::is_ocr_self_test_invocation() {
        worker::run_ocr_self_test()?;
        return Ok(true);
    }
    if worker::is_ocr_worker_invocation() {
        worker::run_ocr_worker()?;
        return Ok(true);
    }
    Ok(false)
}

fn try_run_corpus_invocation() -> Option<Result<PathBuf, String>> {
    const ARGUMENT: &str = "--rshot-ocr-corpus-self-test";
    let mut arguments = std::env::args_os();
    while let Some(argument) = arguments.next() {
        if argument == ARGUMENT {
            let Some(manifest) = arguments.next() else {
                return Some(Err(format!("{ARGUMENT} requires a manifest path")));
            };
            let Some(report) = arguments.next() else {
                return Some(Err(format!("{ARGUMENT} requires a report path")));
            };
            return Some(run_corpus(Path::new(&manifest), Path::new(&report)));
        }
    }
    None
}

fn run_corpus(manifest: &Path, report: &Path) -> Result<PathBuf, String> {
    let source = fs::read_to_string(manifest)
        .map_err(|error| format!("could not read OCR corpus manifest: {error}"))?;
    let directory = manifest
        .parent()
        .ok_or_else(|| String::from("OCR corpus manifest has no parent directory"))?;
    let mut passed = 0_u32;
    for (index, line) in source.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (filename, expected) = line.split_once('\t').ok_or_else(|| {
            format!(
                "OCR corpus manifest line {} is not tab-separated",
                index + 1
            )
        })?;
        let relative = Path::new(filename);
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(format!("OCR corpus filename is not local: {filename}"));
        }
        let image = xcap::image::open(directory.join(relative))
            .map_err(|error| format!("could not load OCR corpus image {filename}: {error}"))?
            .to_rgba8();
        let actual = worker::recognize_with_worker(&image, None)?;
        let normalized = actual.trim().replace("\r\n", "\n");
        if normalized != expected {
            return Err(format!(
                "OCR corpus mismatch for {filename}: expected {expected:?}, got {normalized:?}"
            ));
        }
        passed += 1;
    }
    if passed == 0 {
        return Err(String::from("OCR corpus manifest contains no samples"));
    }
    if let Some(parent) = report
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        report,
        format!("{{\n  \"schema\": \"rshot_ocr_corpus_v1\",\n  \"passed\": {passed}\n}}\n"),
    )
    .map_err(|error| error.to_string())?;
    Ok(report.to_owned())
}

pub(super) fn run_artifact_self_test() -> Result<(), String> {
    worker::run_ocr_self_test()
}

#[cfg(test)]
pub(super) use engine::{
    OcrCharacterData, OcrLineData, OcrRegionData, OcrWordData, is_cjk_language_tag, ocr_region,
    prepare_ocr_rgba, prepare_ocr_rgba_for_recognition, prepare_ocr_worker_rgba,
    rebuild_model_ocr_text, rebuild_ocr_text, regroup_ocr_lines,
    restore_model_cross_region_spacing, restore_model_region_spacing,
};
#[cfg(test)]
pub(super) use worker::worker_protocol_round_trip;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct RecordingAdapter<'a> {
        calls: &'a Cell<usize>,
        result: Result<&'static str, &'static str>,
    }

    impl RecognitionAdapter for RecordingAdapter<'_> {
        fn recognize(&self, _request: &OcrRequest<'_>) -> Result<String, String> {
            self.calls.set(self.calls.get() + 1);
            self.result.map(str::to_owned).map_err(str::to_owned)
        }
    }

    fn request(image: &RgbaImage) -> OcrRequest<'_> {
        OcrRequest {
            frozen_image: image,
            selection: None,
        }
    }

    #[test]
    fn model_success_does_not_call_windows_adapter() {
        let image = RgbaImage::new(2, 2);
        let model_calls = Cell::new(0);
        let windows_calls = Cell::new(0);
        let result = recognize_with(
            request(&image),
            &RecordingAdapter {
                calls: &model_calls,
                result: Ok(" model text "),
            },
            &RecordingAdapter {
                calls: &windows_calls,
                result: Ok("windows text"),
            },
        )
        .unwrap();

        assert_eq!(result.text, "model text");
        assert_eq!(result.backend, OcrBackend::PpOcrV6);
        assert_eq!(model_calls.get(), 1);
        assert_eq!(windows_calls.get(), 0);
    }

    #[test]
    fn empty_model_result_falls_back_once() {
        let image = RgbaImage::new(2, 2);
        let model_calls = Cell::new(0);
        let windows_calls = Cell::new(0);
        let result = recognize_with(
            request(&image),
            &RecordingAdapter {
                calls: &model_calls,
                result: Ok(""),
            },
            &RecordingAdapter {
                calls: &windows_calls,
                result: Ok("fallback"),
            },
        )
        .unwrap();

        assert_eq!(result.backend, OcrBackend::Windows);
        assert_eq!(
            result.fallback_reason,
            Some(OcrFallbackReason::ModelReturnedNoText)
        );
        assert_eq!((model_calls.get(), windows_calls.get()), (1, 1));
    }

    #[test]
    fn model_failure_falls_back_once() {
        let image = RgbaImage::new(2, 2);
        let calls = Cell::new(0);
        let result = recognize_with(
            request(&image),
            &RecordingAdapter {
                calls: &calls,
                result: Err("worker timeout"),
            },
            &RecordingAdapter {
                calls: &calls,
                result: Ok("fallback"),
            },
        )
        .unwrap();

        assert_eq!(
            result.fallback_reason,
            Some(OcrFallbackReason::ModelUnavailable)
        );
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn windows_empty_text_is_a_successful_result() {
        let image = RgbaImage::new(2, 2);
        let calls = Cell::new(0);
        let result = recognize_with(
            request(&image),
            &RecordingAdapter {
                calls: &calls,
                result: Ok(""),
            },
            &RecordingAdapter {
                calls: &calls,
                result: Ok(""),
            },
        )
        .unwrap();

        assert!(result.text.is_empty());
        assert_eq!(result.backend, OcrBackend::Windows);
    }

    #[test]
    fn both_backend_failures_are_preserved() {
        let image = RgbaImage::new(2, 2);
        let calls = Cell::new(0);
        let failure = recognize_with(
            request(&image),
            &RecordingAdapter {
                calls: &calls,
                result: Err("worker timeout"),
            },
            &RecordingAdapter {
                calls: &calls,
                result: Err("系统 OCR 识别失败"),
            },
        )
        .unwrap_err();

        assert_eq!(
            failure.model_stage(),
            OcrFailureStage::ModelWorkerUnavailable
        );
        assert_eq!(failure.stage(), OcrFailureStage::WindowsRecognitionFailed);
        assert!(failure.to_string().contains("worker timeout"));
        assert!(failure.to_string().contains("系统 OCR 识别失败"));
    }

    #[test]
    fn invalid_selection_calls_neither_adapter() {
        let image = RgbaImage::new(2, 2);
        let calls = Cell::new(0);
        let failure = recognize_with(
            OcrRequest {
                frozen_image: &image,
                selection: Some(((1, 1), (1, 2))),
            },
            &RecordingAdapter {
                calls: &calls,
                result: Ok("model"),
            },
            &RecordingAdapter {
                calls: &calls,
                result: Ok("windows"),
            },
        )
        .unwrap_err();

        assert_eq!(failure.stage(), OcrFailureStage::InvalidInput);
        assert_eq!(calls.get(), 0);
    }
}
