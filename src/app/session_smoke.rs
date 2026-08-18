use super::App;
use super::capture_operation::CaptureOperation;
use super::ocr;
use super::pinned::run_pin_coexistence_self_test;
use std::fs;
use std::path::{Path, PathBuf};
use xcap::image::RgbaImage;

const ARGUMENT: &str = "--rshot-session-self-test";

pub(super) fn try_run_invocation() -> Option<Result<PathBuf, String>> {
    let mut arguments = std::env::args_os();
    while let Some(argument) = arguments.next() {
        if argument == ARGUMENT {
            let Some(path) = arguments.next() else {
                return Some(Err(format!("{ARGUMENT} requires an output path")));
            };
            return Some(run(Path::new(&path)));
        }
    }
    None
}

fn install_capture(app: &mut App, width: u32, height: u32) {
    app.capture_operation = Some(CaptureOperation::ready_without_window(RgbaImage::new(
        width, height,
    )));
}

fn run(path: &Path) -> Result<PathBuf, String> {
    let mut app = App::default();

    install_capture(&mut app, 8, 6);
    if app.capture_operation.is_none() {
        return Err(String::from("first capture did not create a session"));
    }
    app.close_overlay();

    for (width, height) in [(8, 6), (12, 9)] {
        install_capture(&mut app, width, height);
        if app.capture_operation.is_none() {
            return Err(String::from("consecutive capture did not create a session"));
        }
        app.close_overlay();
    }

    run_pin_coexistence_self_test(ocr::run_artifact_self_test)?;

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let report = concat!(
        "{\n",
        "  \"schema\": \"rshot_session_smoke_v1\",\n",
        "  \"scenarios\": [\"first_capture\", \"consecutive_capture\", \"pin_coexistence\", \"ocr_with_pin\"]\n",
        "}\n"
    );
    fs::write(path, report).map_err(|error| error.to_string())?;
    Ok(path.to_owned())
}
