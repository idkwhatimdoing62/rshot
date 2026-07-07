use arboard::{Clipboard, ImageData};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use serde::{Deserialize, Serialize};
use std::error::Error;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW,
    GetCursorPos,
    GetMessageW,
    MSG, // ← MSG 加进来
};
use xcap::Monitor;
#[derive(Serialize, Deserialize)]
struct Config {
    hotkey: String,
    quit: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hotkey: "Alt+A".into(),
            quit: "Alt+D".into(),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let manager = GlobalHotKeyManager::new()?;
    let cfg: Config = confy::load("rshot", None)?;
    let hotkey: HotKey = cfg.hotkey.parse()?; // "Alt+KeyQ" → HotKey，一行
    let quit: HotKey = cfg.quit.parse()?; // "Alt+KeyQ" → HotKey，一行
    manager.register(hotkey)?; // 订阅热键监听
    manager.register(quit)?;

    println!(
        "配置文件: {}",
        confy::get_configuration_file_path("rshot", None)?.display()
    );

    let receiver = GlobalHotKeyEvent::receiver(); // 拿到信箱
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // 取出一条消息
            DispatchMessageW(&msg); // 派发消息给信箱
            while let Ok(event) = receiver.try_recv() {
                if event.state != HotKeyState::Pressed {
                    continue;
                } // 不是按下→跳过，减一层
                if event.id == quit.id {
                    return Ok(());
                } // 退出→直接结束整个程序
                if event.id == hotkey.id {
                    capture_to_clipboard()?;
                }
            }
        }
    }
    Ok(())
}

fn capture_to_clipboard() -> Result<(), Box<dyn Error>> {
    let mut p = POINT::default();
    unsafe {
        GetCursorPos(&mut p)?;
    };
    let monitor = Monitor::from_point(p.x, p.y)?; // 返回鼠标所在的显示屏
    let image = monitor.capture_image()?; // 先解包
    let w = image.width() as usize; // 真实宽
    let h = image.height() as usize; // 真实高（into_raw 前取，否则 image 被消耗）

    let mut clipboard = Clipboard::new()?;
    clipboard.set_image(ImageData {
        width: w,
        height: h,
        bytes: image.into_raw().into(), // Vec<u8> → Cow<[u8]>
    })?;
    Ok(())
}
