use super::engine::{
    OcrCharacterData, OcrRegionData, prepare_ocr_worker_rgba, rebuild_model_ocr_text,
    restore_model_cross_region_spacing, restore_model_region_spacing,
};
use oar_ocr::core::config::OrtSessionConfig;
use oar_ocr::domain::tasks::{TextDetectionConfig, TextRecognitionConfig};
use oar_ocr::oarocr::OAROCRBuilder;
use oar_ocr::processors::LimitType;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use xcap::image::{DynamicImage, Rgba, RgbaImage};

#[cfg(windows)]
use std::os::windows::{io::AsRawHandle, process::CommandExt};
#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};

const OCR_WORKER_ARGUMENT: &str = "--rshot-ocr-worker";
const OCR_SELF_TEST_ARGUMENT: &str = "--rshot-ocr-self-test";
const OCR_WORKER_MAGIC: &[u8; 8] = b"RSHOTOC2";
const OCR_WORKER_MAX_PIXELS: u64 = 8_000_000;
const OCR_WORKER_MAX_OUTPUT: usize = 8 * 1024 * 1024;
const OCR_WORKER_MAX_ERROR_OUTPUT: usize = 64 * 1024;
const OCR_WORKER_TIMEOUT: Duration = Duration::from_secs(20);
const OCR_WORKER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const OCR_RUNTIME_DIRECTORY: &str = env!("RSHOT_ORT_RUNTIME_ID");
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
struct KillOnCloseJob(HANDLE);

#[cfg(windows)]
impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        // SAFETY: 句柄由 CreateJobObjectW 创建且只由此 RAII 对象关闭一次。
        let _ = unsafe { CloseHandle(self.0) };
    }
}

#[cfg(windows)]
fn attach_to_kill_on_close_job(child: &std::process::Child) -> Result<KillOnCloseJob, String> {
    // SAFETY: 不传安全描述符和名称；返回的有效句柄由 KillOnCloseJob 管理。
    let job = unsafe { CreateJobObjectW(None, None) }
        .map_err(|error| format!("无法创建 OCR worker 回收边界：{error}"))?;
    let job = KillOnCloseJob(job);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: limits 在调用期间有效，类型和长度与信息类匹配。
    unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    }
    .map_err(|error| format!("无法设置 OCR worker 自动回收：{error}"))?;
    let process = HANDLE(child.as_raw_handle());
    // SAFETY: child 的进程句柄在 Child 生命周期内有效；job 在本函数及调用方中保持打开。
    unsafe { AssignProcessToJobObject(job.0, process) }
        .map_err(|error| format!("无法关联 OCR worker 自动回收：{error}"))?;
    Ok(job)
}

static DETECTION_MODEL: &[u8] = include_bytes!(env!("RSHOT_OCR_DET_MODEL"));
static RECOGNITION_MODEL: &[u8] = include_bytes!(env!("RSHOT_OCR_REC_MODEL"));
static CHARACTER_DICTIONARY: &str = include_str!(env!("RSHOT_OCR_DICT"));
static ORT_RUNTIME: &[u8] = include_bytes!(env!("RSHOT_ORT_DLL"));
static ORT_PROVIDERS: &[u8] = include_bytes!(env!("RSHOT_ORT_PROVIDERS_DLL"));

#[cfg(test)]
pub(in crate::app) fn embedded_character_count() -> usize {
    CHARACTER_DICTIONARY.lines().count()
}

pub(super) fn is_ocr_worker_invocation() -> bool {
    std::env::args().nth(1).as_deref() == Some(OCR_WORKER_ARGUMENT)
}

pub(super) fn is_ocr_self_test_invocation() -> bool {
    std::env::args().nth(1).as_deref() == Some(OCR_SELF_TEST_ARGUMENT)
}

fn write_request(
    mut writer: impl Write,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<(), String> {
    writer
        .write_all(OCR_WORKER_MAGIC)
        .and_then(|_| writer.write_all(&width.to_le_bytes()))
        .and_then(|_| writer.write_all(&height.to_le_bytes()))
        .and_then(|_| writer.write_all(rgba))
        .and_then(|_| writer.flush())
        .map_err(|error| format!("无法向高精度 OCR 进程发送图片：{error}"))
}

fn read_request(mut reader: impl Read) -> Result<RgbaImage, String> {
    let mut magic = [0_u8; 8];
    let mut width = [0_u8; 4];
    let mut height = [0_u8; 4];
    reader
        .read_exact(&mut magic)
        .and_then(|_| reader.read_exact(&mut width))
        .and_then(|_| reader.read_exact(&mut height))
        .map_err(|error| format!("无法读取 OCR 请求头：{error}"))?;
    if &magic != OCR_WORKER_MAGIC {
        return Err(String::from("OCR 请求版本不匹配"));
    }
    let width = u32::from_le_bytes(width);
    let height = u32::from_le_bytes(height);
    let pixels = width as u64 * height as u64;
    if width == 0 || height == 0 || pixels > OCR_WORKER_MAX_PIXELS {
        return Err(String::from("OCR 请求图片尺寸无效或超过像素上限"));
    }
    let byte_len = pixels
        .checked_mul(4)
        .and_then(|length| usize::try_from(length).ok())
        .ok_or_else(|| String::from("OCR 请求图片尺寸溢出"))?;
    let mut rgba = vec![0_u8; byte_len];
    reader
        .read_exact(&mut rgba)
        .map_err(|error| format!("无法读取 OCR 图片：{error}"))?;
    RgbaImage::from_raw(width, height, rgba).ok_or_else(|| String::from("无法构造 OCR 图片"))
}

fn file_matches(path: &Path, expected: &[u8]) -> bool {
    if !path
        .metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() == expected.len() as u64)
    {
        return false;
    }
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut offset = 0_usize;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let Ok(read) = file.read(&mut buffer) else {
            return false;
        };
        if read == 0 {
            return offset == expected.len();
        }
        if expected.get(offset..offset + read) != Some(&buffer[..read]) {
            return false;
        }
        offset += read;
    }
}

fn install_embedded_file(directory: &Path, name: &str, contents: &[u8]) -> Result<PathBuf, String> {
    let destination = directory.join(name);
    if file_matches(&destination, contents) {
        return Ok(destination);
    }

    let temporary = directory.join(format!(".{name}.{}.tmp", std::process::id()));
    if temporary.exists() {
        fs::remove_file(&temporary)
            .map_err(|error| format!("无法清理 OCR 运行时临时文件：{error}"))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("无法创建 OCR 运行时临时文件：{error}"))?;
    let write_result = file
        .write_all(contents)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all());
    drop(file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("无法写入 OCR 运行时：{error}"));
    }

    if destination.exists() {
        if file_matches(&destination, contents) {
            let _ = fs::remove_file(&temporary);
            return Ok(destination);
        }
        if let Err(error) = fs::remove_file(&destination) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("无法替换损坏的 OCR 运行时：{error}"));
        }
    }
    if let Err(error) = fs::rename(&temporary, &destination) {
        if file_matches(&destination, contents) {
            let _ = fs::remove_file(&temporary);
            return Ok(destination);
        }
        let _ = fs::remove_file(&temporary);
        return Err(format!("无法安装 OCR 运行时：{error}"));
    }
    Ok(destination)
}

fn ensure_embedded_runtime() -> Result<PathBuf, String> {
    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RShot")
        .join(OCR_RUNTIME_DIRECTORY);
    fs::create_dir_all(&root).map_err(|error| format!("无法创建 OCR 运行时目录：{error}"))?;

    // 先放 provider 共享库，最后放主 DLL；主 DLL 可视为完整安装标志。
    install_embedded_file(&root, "onnxruntime_providers_shared.dll", ORT_PROVIDERS)?;
    install_embedded_file(&root, "onnxruntime.dll", ORT_RUNTIME)
}

fn recognize(image: RgbaImage) -> Result<String, String> {
    let runtime = ensure_embedded_runtime()?;
    let environment =
        ort::init_from(&runtime).map_err(|error| format!("无法加载高精度 OCR 运行时：{error}"))?;
    if !environment.commit() {
        return Err(String::from("高精度 OCR 运行时已被提前初始化"));
    }

    let ocr = OAROCRBuilder::new(DETECTION_MODEL, RECOGNITION_MODEL, "")
        .character_dict_content(CHARACTER_DICTIONARY)
        .ort_session(
            OrtSessionConfig::new()
                .with_intra_threads(2)
                .with_inter_threads(1)
                .with_parallel_execution(false)
                .with_memory_pattern(false),
        )
        .text_detection_config(TextDetectionConfig {
            score_threshold: 0.2,
            box_threshold: 0.45,
            unclip_ratio: 1.4,
            // 显式固定 OAR 的通用低内存档位。更大的检测张量会明显抬高 8 GB
            // 设备峰值；高分屏密集小字应由用户缩小选区后再识别。
            limit_side_len: Some(960),
            limit_type: Some(LimitType::Max),
            // 沿用 OAR 的有界默认值。过低的值会在置信度过滤前截断原始轮廓，
            // 可能静默漏掉正文；异常密集图片由 20 秒 worker 熔断负责降级。
            max_candidates: 1000,
            ..Default::default()
        })
        .text_recognition_config(TextRecognitionConfig {
            score_threshold: 0.25,
        })
        .return_word_box(true)
        .image_batch_size(1)
        // 在逐区域串行和 OAR 的四区域 CPU 吞吐档位之间折中，限制 8 GB
        // 设备上的瞬时张量并发，同时减少密集页面撞上总超时的概率。
        .region_batch_size(2)
        .build()
        .map_err(|error| format!("无法加载 PP-OCRv6 small-det/medium-rec：{error}"))?;
    let results = ocr
        .predict(vec![DynamicImage::ImageRgba8(image).into_rgb8()])
        .map_err(|error| format!("PP-OCRv6 small-det/medium-rec 识别失败：{error}"))?;
    let result = results
        .first()
        .ok_or_else(|| String::from("PP-OCRv6 small-det/medium-rec 未返回结果"))?;
    let source_image = result.input_img.as_ref();
    let mut regions = Vec::with_capacity(result.text_regions.len());
    let mut region_characters = Vec::with_capacity(result.text_regions.len());
    for region in &result.text_regions {
        let Some((text, confidence)) = region.text_with_confidence() else {
            continue;
        };
        if text.trim().is_empty() || confidence < 0.25 {
            continue;
        }
        let text_characters: Vec<char> = text.chars().collect();
        let characters = region
            .word_boxes
            .as_ref()
            .filter(|boxes| boxes.len() == text_characters.len())
            .map(|boxes| {
                text_characters
                    .iter()
                    .copied()
                    .zip(boxes)
                    .map(|(ch, bounds)| OcrCharacterData {
                        ch,
                        x: bounds.x_min(),
                        y: bounds.y_min(),
                        width: bounds.x_max() - bounds.x_min(),
                        height: bounds.y_max() - bounds.y_min(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let bounds = &region.bounding_box;
        regions.push(OcrRegionData {
            text: restore_model_region_spacing(text, &characters, source_image),
            x: bounds.x_min(),
            y: bounds.y_min(),
            width: bounds.x_max() - bounds.x_min(),
            height: bounds.y_max() - bounds.y_min(),
            space_before: false,
        });
        region_characters.push(characters);
    }
    restore_model_cross_region_spacing(&mut regions, &region_characters, source_image);
    Ok(rebuild_model_ocr_text(&regions).trim().to_owned())
}

pub(super) fn run_ocr_worker() -> Result<(), String> {
    let image = read_request(std::io::stdin().lock())?;
    let text = recognize(image)?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(text.as_bytes())
        .and_then(|_| stdout.flush())
        .map_err(|error| format!("无法返回 OCR 结果：{error}"))
}

pub(super) fn run_ocr_self_test() -> Result<(), String> {
    let image = RgbaImage::from_pixel(64, 64, Rgba([255, 255, 255, 255]));
    // 走与界面相同的子进程、Job Object、管道、运行时提取和模型推理路径。
    recognize_with_worker(&image, None).map(|_| ())
}

fn read_limited(mut reader: impl Read, limit: usize, label: &str) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut overflowed = false;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("无法读取{label}：{error}"))?;
        if read == 0 {
            break;
        }
        let available = limit.saturating_sub(output.len());
        let retained = available.min(read);
        output.extend_from_slice(&buffer[..retained]);
        overflowed |= retained < read;
    }
    if overflowed {
        Err(format!("{label}超过大小上限"))
    } else {
        Ok(output)
    }
}

fn join_worker_thread<T>(
    thread: thread::JoinHandle<Result<T, String>>,
    label: &str,
) -> Result<T, String> {
    thread.join().map_err(|_| format!("{label}线程异常结束"))?
}

pub(super) fn recognize_with_worker(
    image: &RgbaImage,
    selection: Option<((i32, i32), (i32, i32))>,
) -> Result<String, String> {
    let started = Instant::now();
    let (rgba, width, height) =
        prepare_ocr_worker_rgba(image, selection).ok_or_else(|| String::from("选区尺寸无效"))?;
    let executable =
        std::env::current_exe().map_err(|error| format!("无法定位高精度 OCR 进程：{error}"))?;
    let mut command = Command::new(executable);
    command
        .arg(OCR_WORKER_ARGUMENT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动高精度 OCR 进程：{error}"))?;
    // 请求只会在 Job 关联成功后写入。关联前 worker 阻塞在请求头读取；若父进程
    // 此时异常退出，唯一的 stdin 写端随进程关闭，worker 会因 EOF 自行退出。
    #[cfg(windows)]
    let _worker_job = match attach_to_kill_on_close_job(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let Some(stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(String::from("无法连接高精度 OCR 进程输入"));
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(String::from("无法连接高精度 OCR 进程输出"));
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(String::from("无法连接高精度 OCR 进程错误输出"));
    };

    let input_thread = match thread::Builder::new()
        .name(String::from("rshot-ocr-input"))
        .spawn(move || write_request(stdin, &rgba, width, height))
    {
        Ok(thread) => thread,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("无法创建高精度 OCR 输入线程：{error}"));
        }
    };
    let output_thread = match thread::Builder::new()
        .name(String::from("rshot-ocr-output"))
        .spawn(move || read_limited(stdout, OCR_WORKER_MAX_OUTPUT, "高精度 OCR 输出"))
    {
        Ok(thread) => thread,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = join_worker_thread(input_thread, "OCR 输入");
            return Err(format!("无法创建高精度 OCR 输出线程：{error}"));
        }
    };
    let error_thread = match thread::Builder::new()
        .name(String::from("rshot-ocr-error"))
        .spawn(move || read_limited(stderr, OCR_WORKER_MAX_ERROR_OUTPUT, "高精度 OCR 错误输出"))
    {
        Ok(thread) => thread,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = join_worker_thread(input_thread, "OCR 输入");
            let _ = join_worker_thread(output_thread, "OCR 输出");
            return Err(format!("无法创建高精度 OCR 错误输出线程：{error}"));
        }
    };

    let deadline = started + OCR_WORKER_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(OCR_WORKER_POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_worker_thread(input_thread, "OCR 输入");
                let _ = join_worker_thread(output_thread, "OCR 输出");
                let _ = join_worker_thread(error_thread, "OCR 错误输出");
                return Err(String::from("高精度 OCR 超过 20 秒，已终止并回退系统 OCR"));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_worker_thread(input_thread, "OCR 输入");
                let _ = join_worker_thread(output_thread, "OCR 输出");
                let _ = join_worker_thread(error_thread, "OCR 错误输出");
                return Err(format!("等待高精度 OCR 进程失败：{error}"));
            }
        }
    };

    let input_result = join_worker_thread(input_thread, "OCR 输入");
    let output = join_worker_thread(output_thread, "OCR 输出");
    let error_output = join_worker_thread(error_thread, "OCR 错误输出");
    if !status.success() {
        let detail = error_output
            .as_deref()
            .map(String::from_utf8_lossy)
            .map(|detail| detail.trim().to_owned())
            .unwrap_or_else(|error| error.clone());
        return Err(if detail.is_empty() {
            String::from("高精度 OCR 进程异常结束")
        } else {
            format!("高精度 OCR 进程异常结束：{detail}")
        });
    }
    input_result?;
    let output = output?;
    error_output?;
    String::from_utf8(output).map_err(|error| format!("OCR 结果不是有效 UTF-8：{error}"))
}

#[cfg(test)]
pub(in crate::app) fn worker_protocol_round_trip(
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<RgbaImage, String> {
    let mut request = Vec::new();
    write_request(&mut request, rgba, width, height)?;
    read_request(request.as_slice())
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_dictionary_matches_the_recognition_model() {
        assert_eq!(super::embedded_character_count(), 18_708);
    }
}
