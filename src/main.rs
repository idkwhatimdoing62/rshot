use arboard::{Clipboard, ImageData};
use mouse_position::mouse_position::Mouse;
use std::error::Error;
use xcap::Monitor;

fn main() -> Result<(), Box<dyn Error>> {
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
