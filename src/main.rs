use arboard::{Clipboard, ImageData};
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager,
    hotkey::{Code, HotKey, Modifiers},
};
use mouse_position::mouse_position::Mouse;
use std::error::Error;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW,
    GetMessageW,
    MSG, // ← MSG 加进来
};
use xcap::Monitor;
fn main() -> Result<(), Box<dyn Error>> {
    let manager = GlobalHotKeyManager::new()?;
    let hotkey = HotKey::new(Some(Modifiers::ALT), Code::KeyA);
    manager.register(hotkey)?; // 订阅热键监听

    let receiver = GlobalHotKeyEvent::receiver(); // 拿到信箱
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() { // 取出一条消息
            DispatchMessageW(&msg); // 派发消息给信箱
            if let Ok(_) = receiver.try_recv() { //取出消息
                capture_to_clipboard()?;
            }
        }
    }
    Ok(())
}

fn capture_to_clipboard() -> Result<(), Box<dyn Error>> {
    let position = Mouse::get_mouse_position();
    let Mouse::Position { x, y } = position else {
        return Err("拿不到鼠标坐标".into()); // 不匹配就早退
    };
    let monitor = Monitor::from_point(x, y)?; // 返回鼠标所在的显示屏
    let mut clipboard = Clipboard::new()?;
    let image = monitor.capture_image()?; // 先解包
    let w = image.width() as usize; // 真实宽
    let h = image.height() as usize; // 真实高（into_raw 前取，否则 image 被消耗）
    clipboard.set_image(ImageData {
        width: w,
        height: h,
        bytes: image.into_raw().into(), // Vec<u8> → Cow<[u8]>
    })?;
    Ok(())
}
