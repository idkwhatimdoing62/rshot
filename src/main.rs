// release 构建切到 windows 子系统 = 双击不弹黑色控制台窗口。
// debug（cargo run）保留控制台，方便看 println!/panic。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

fn main() {
    app::entry();
}
