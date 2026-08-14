use sha2::{Digest, Sha256};
use std::env;
use std::fmt::Write as FmtWrite;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

struct EmbeddedFile {
    name: &'static str,
    size: u64,
    sha256: &'static str,
    rustc_env: &'static str,
}

struct DownloadFile {
    file: EmbeddedFile,
    url: &'static str,
}

const MODELS: &[DownloadFile] = &[
    DownloadFile {
        file: EmbeddedFile {
            name: "pp-ocrv6_small_det.onnx",
            size: 9_880_512,
            sha256: "d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e",
            rustc_env: "RSHOT_OCR_DET_MODEL",
        },
        url: "https://github.com/GreatV/oar-ocr/releases/download/v0.7.0/pp-ocrv6_small_det.onnx",
    },
    DownloadFile {
        file: EmbeddedFile {
            name: "pp-ocrv6_medium_rec.onnx",
            size: 76_554_979,
            sha256: "9c09abf0957f7968c7586464b7397b84ad2387a0497a351af40e9acc71b673ba",
            rustc_env: "RSHOT_OCR_REC_MODEL",
        },
        url: "https://github.com/GreatV/oar-ocr/releases/download/v0.7.0/pp-ocrv6_medium_rec.onnx",
    },
    DownloadFile {
        file: EmbeddedFile {
            name: "ppocrv6_dict.txt",
            size: 74_947,
            sha256: "b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d",
            rustc_env: "RSHOT_OCR_DICT",
        },
        url: "https://github.com/GreatV/oar-ocr/releases/download/v0.7.0/ppocrv6_dict.txt",
    },
];

const ORT_ARCHIVE_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-win-x64-1.28.0.zip";
const ORT_ARCHIVE_NAME: &str = "onnxruntime-win-x64-1.28.0.zip";
const ORT_ARCHIVE_SIZE: u64 = 78_796_801;
const ORT_ARCHIVE_SHA256: &str = "abef733dacbe2f571547a7150b479b5cb9cc0df22f96c24983a42cadb1b4f8bc";
const ORT_CUSTOM_MANIFEST: &str = "rshot-ocr-runtime.sha256";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const ORT_FILES: &[EmbeddedFile] = &[
    EmbeddedFile {
        name: "onnxruntime.dll",
        size: 15_809_848,
        sha256: "18370c375f07357fa5874344a9d9ac17e6b6fe1eb18b1dd209d79483b4470257",
        rustc_env: "RSHOT_ORT_DLL",
    },
    EmbeddedFile {
        name: "onnxruntime_providers_shared.dll",
        size: 21_856,
        sha256: "599629fa643707defe9156140ae5edd73531f221aa97b7585b1c9bb0a93586f8",
        rustc_env: "RSHOT_ORT_PROVIDERS_DLL",
    },
];

fn sha256(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut output = String::with_capacity(64);
    for byte in digest {
        let _ = write!(&mut output, "{byte:02x}");
    }
    Ok(output)
}

fn runtime_content_id(directory: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    for runtime in ORT_FILES {
        hasher.update(runtime.name.as_bytes());
        let mut file = File::open(directory.join(runtime.name))?;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
    }
    let digest = hasher.finalize();
    let mut output = String::with_capacity(64);
    for byte in digest {
        let _ = write!(&mut output, "{byte:02x}");
    }
    Ok(output)
}

fn is_valid(path: &Path, size: u64, expected_sha256: &str) -> bool {
    path.metadata().is_ok_and(|metadata| metadata.len() == size)
        && sha256(path).is_ok_and(|digest| digest == expected_sha256)
}

fn embedded_files_are_complete(directory: &Path, files: &[EmbeddedFile]) -> bool {
    files
        .iter()
        .all(|file| is_valid(&directory.join(file.name), file.size, file.sha256))
}

fn custom_runtime_is_complete(directory: &Path) -> bool {
    let manifest_path = directory.join(ORT_CUSTOM_MANIFEST);
    let Ok(manifest) = fs::read_to_string(&manifest_path) else {
        return false;
    };
    ORT_FILES.iter().all(|runtime| {
        let expected = manifest.lines().find_map(|line| {
            let mut fields = line.split_whitespace();
            let digest = fields.next()?;
            let name = fields.next()?.trim_start_matches('*');
            (name.eq_ignore_ascii_case(runtime.name)
                && digest.len() == 64
                && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| digest.to_ascii_lowercase())
        });
        expected.is_some_and(|digest| {
            let path = directory.join(runtime.name);
            path.metadata().is_ok_and(|metadata| metadata.is_file())
                && sha256(&path).is_ok_and(|actual| actual == digest)
        })
    })
}

fn model_directory_is_complete(directory: &Path) -> bool {
    MODELS.iter().all(|model| {
        is_valid(
            &directory.join(model.file.name),
            model.file.size,
            model.file.sha256,
        )
    })
}

fn download_verified(
    url: &str,
    label: &str,
    size: u64,
    expected_sha256: &str,
    destination: &Path,
) -> Result<(), String> {
    let temporary = destination.with_extension("download");
    let _ = fs::remove_file(&temporary);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .timeout_connect(Some(DOWNLOAD_CONNECT_TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .header("User-Agent", "rshot-build")
        .call()
        .map_err(|error| format!("下载 {label} 失败：{error}"))?;
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(size.saturating_add(1))
        .reader();
    let mut file = File::create(&temporary)
        .map_err(|error| format!("创建 {} 失败：{error}", temporary.display()))?;
    io::copy(&mut reader, &mut file)
        .map_err(|error| format!("写入 {} 失败：{error}", temporary.display()))?;
    file.flush()
        .map_err(|error| format!("刷新 {} 失败：{error}", temporary.display()))?;
    drop(file);
    if !is_valid(&temporary, size, expected_sha256) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("{label} 的大小或 SHA-256 校验失败"));
    }
    if destination.exists() {
        fs::remove_file(destination)
            .map_err(|error| format!("替换 {} 失败：{error}", destination.display()))?;
    }
    fs::rename(&temporary, destination)
        .map_err(|error| format!("保存 {} 失败：{error}", destination.display()))?;
    Ok(())
}

fn target_directory() -> Result<PathBuf, String> {
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR 不存在")?);
    Ok(env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("target")))
}

fn model_directory() -> Result<PathBuf, String> {
    println!("cargo:rerun-if-env-changed=RSHOT_OCR_MODEL_DIR");
    println!("cargo:rerun-if-env-changed=OAR_HOME");

    if let Some(explicit) = env::var_os("RSHOT_OCR_MODEL_DIR") {
        let directory = PathBuf::from(explicit);
        if model_directory_is_complete(&directory) {
            return Ok(directory);
        }
        return Err(format!(
            "RSHOT_OCR_MODEL_DIR={} 不包含校验通过的 PP-OCRv6 small-det/medium-rec 模型",
            directory.display()
        ));
    }

    let mut candidates = Vec::new();
    if let Some(directory) = env::var_os("OAR_HOME") {
        candidates.push(PathBuf::from(directory));
    }
    if let Some(profile) = env::var_os("USERPROFILE") {
        candidates.push(PathBuf::from(profile).join(".oar"));
    }
    if let Some(directory) = candidates
        .into_iter()
        .find(|directory| model_directory_is_complete(directory))
    {
        return Ok(directory);
    }

    let directory = target_directory()?.join("rshot-ocr-models");
    fs::create_dir_all(&directory).map_err(|error| format!("创建模型缓存目录失败：{error}"))?;
    for model in MODELS {
        let path = directory.join(model.file.name);
        if !is_valid(&path, model.file.size, model.file.sha256) {
            println!("cargo:warning=首次构建正在下载并校验 {}", model.file.name);
            download_verified(
                model.url,
                model.file.name,
                model.file.size,
                model.file.sha256,
                &path,
            )?;
        }
    }
    Ok(directory)
}

fn extract_runtime(archive: &Path, directory: &Path) -> Result<(), String> {
    let file =
        File::open(archive).map_err(|error| format!("打开 {} 失败：{error}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| format!("读取 {} 失败：{error}", archive.display()))?;
    for runtime in ORT_FILES {
        let archive_path = format!("onnxruntime-win-x64-1.28.0/lib/{}", runtime.name);
        let mut source = zip
            .by_name(&archive_path)
            .map_err(|error| format!("压缩包缺少 {archive_path}：{error}"))?;
        let destination = directory.join(runtime.name);
        let temporary = destination.with_extension("extracting");
        let _ = fs::remove_file(&temporary);
        let mut output = File::create(&temporary)
            .map_err(|error| format!("创建 {} 失败：{error}", temporary.display()))?;
        io::copy(&mut source, &mut output)
            .map_err(|error| format!("解压 {} 失败：{error}", runtime.name))?;
        output
            .flush()
            .map_err(|error| format!("刷新 {} 失败：{error}", temporary.display()))?;
        drop(output);
        if !is_valid(&temporary, runtime.size, runtime.sha256) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("{} 的大小或 SHA-256 校验失败", runtime.name));
        }
        if destination.exists() {
            fs::remove_file(&destination)
                .map_err(|error| format!("替换 {} 失败：{error}", destination.display()))?;
        }
        fs::rename(&temporary, &destination)
            .map_err(|error| format!("保存 {} 失败：{error}", destination.display()))?;
    }
    Ok(())
}

fn runtime_directory() -> Result<PathBuf, String> {
    println!("cargo:rerun-if-env-changed=RSHOT_OCR_RUNTIME_DIR");
    if let Some(explicit) = env::var_os("RSHOT_OCR_RUNTIME_DIR") {
        let directory = PathBuf::from(explicit);
        println!(
            "cargo:rerun-if-changed={}",
            directory.join(ORT_CUSTOM_MANIFEST).display()
        );
        if embedded_files_are_complete(&directory, ORT_FILES)
            || custom_runtime_is_complete(&directory)
        {
            return Ok(directory);
        }
        return Err(format!(
            "RSHOT_OCR_RUNTIME_DIR={} 不包含官方校验值匹配的文件，也没有有效的 {}",
            directory.display(),
            ORT_CUSTOM_MANIFEST,
        ));
    }

    let directory = target_directory()?.join("rshot-ocr-runtime");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("创建推理运行时缓存目录失败：{error}"))?;
    if embedded_files_are_complete(&directory, ORT_FILES) {
        return Ok(directory);
    }

    let archive = directory.join(ORT_ARCHIVE_NAME);
    if !is_valid(&archive, ORT_ARCHIVE_SIZE, ORT_ARCHIVE_SHA256) {
        println!("cargo:warning=首次构建正在下载并校验 ONNX Runtime 1.28.0 CPU");
        download_verified(
            ORT_ARCHIVE_URL,
            ORT_ARCHIVE_NAME,
            ORT_ARCHIVE_SIZE,
            ORT_ARCHIVE_SHA256,
            &archive,
        )?;
    }
    extract_runtime(&archive, &directory)?;
    Ok(directory)
}

fn expose_embedded_files(directory: &Path, files: &[EmbeddedFile]) {
    for file in files {
        let path = directory.join(file.name);
        println!("cargo:rerun-if-changed={}", path.display());
        println!("cargo:rustc-env={}={}", file.rustc_env, path.display());
    }
}

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_os != "windows" || target_arch != "x86_64" {
        panic!("内置 OCR 运行时目前只支持 Windows x86_64，当前目标为 {target_os}/{target_arch}");
    }

    let models = model_directory().unwrap_or_else(|error| {
        panic!(
            "无法准备离线 OCR 模型：{error}\n可以联网重试，或将模型放入目录后设置 RSHOT_OCR_MODEL_DIR。"
        )
    });
    let runtime = runtime_directory().unwrap_or_else(|error| {
        panic!(
            "无法准备离线 OCR 推理运行时：{error}\n可以联网重试，或将运行时文件放入目录后设置 RSHOT_OCR_RUNTIME_DIR。"
        )
    });
    let model_files: Vec<EmbeddedFile> = MODELS
        .iter()
        .map(|model| EmbeddedFile {
            name: model.file.name,
            size: model.file.size,
            sha256: model.file.sha256,
            rustc_env: model.file.rustc_env,
        })
        .collect();
    expose_embedded_files(&models, &model_files);
    expose_embedded_files(&runtime, ORT_FILES);
    let runtime_digest = runtime_content_id(&runtime)
        .unwrap_or_else(|error| panic!("无法计算 ONNX Runtime 内容标识：{error}"));
    println!(
        "cargo:rustc-env=RSHOT_ORT_RUNTIME_ID=ocr-runtime-win-x64-1.28.0-{}",
        &runtime_digest[..12]
    );
}
