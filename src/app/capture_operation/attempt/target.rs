use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;

pub(super) struct CaptureTarget {
    pub(super) overlay_monitor: MonitorHandle,
    pub(super) origin: (i32, i32),
}

pub(super) fn match_overlay_monitor(
    event_loop: &dyn ActiveEventLoop,
    cursor: (i32, i32),
) -> Option<CaptureTarget> {
    let (cx, cy) = cursor;
    event_loop.available_monitors().find_map(|monitor| {
        let (Some(position), Some(mode)) = (monitor.position(), monitor.current_video_mode())
        else {
            return None;
        };
        let size = mode.size();
        (cx >= position.x
            && cy >= position.y
            && cx < position.x + size.width as i32
            && cy < position.y + size.height as i32)
            .then_some(CaptureTarget {
                overlay_monitor: monitor,
                origin: (position.x, position.y),
            })
    })
}
