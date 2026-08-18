use super::*;

impl ApplicationHandler for App {
    // 本程序不在启动时建窗口，遮罩是热键触发后临时建的，这里留空
    fn can_create_surfaces(&mut self, _event_loop: &dyn ActiveEventLoop) {}

    // 每轮空闲：轮询 global-hotkey 的事件通道
    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        // 启动后立即检查一次，之后每 12 小时检查一次。
        self.temp_artifacts.tick(Instant::now());
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            if ev.id == self.quit_id {
                event_loop.exit(); // 退出整个程序
            } else if ev.id == self.shot_id && self.capture_operation.is_none() {
                // 只限制活动截图会话；已有贴图不会阻止下一次截图。
                self.open_overlay(event_loop);
            }
        }
        // 托盘双击只显示程序与当前快捷键信息，不触发截图。
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = ev {
                show_message(&self.about_message, false);
            }
        }
        let now = Instant::now();
        let capture_wakeup = self
            .capture_operation
            .as_mut()
            .and_then(|operation| operation.tick(now));
        // ponytail: 120ms 轮询一次热键。想零延迟得用 EventLoopProxy 唤醒，暂不需要
        let hotkey_wakeup = now + Duration::from_millis(120);
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            capture_wakeup
                .map(|capture| capture.min(hotkey_wakeup))
                .unwrap_or(hotkey_wakeup),
        ));
    }

    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let event = match self.pins.handle_window_event(id, event) {
            PinEventOutcome::Handled => return,
            PinEventOutcome::Failed(failure) => {
                if self.diagnostics_enabled {
                    let _ = record_diagnostic(DiagnosticEvent::Pin(failure.stage()));
                }
                show_message(
                    &format!("一张置顶贴图发生错误，已单独关闭。\n\n{failure}"),
                    true,
                );
                return;
            }
            PinEventOutcome::NotOwned(event) => event,
        };
        let Some(operation) = &mut self.capture_operation else {
            return;
        };
        let command = operation.handle_window_event(id, event);
        match command {
            CaptureCommand::None => {}
            CaptureCommand::Close => self.close_overlay(),
            CaptureCommand::Copy => self.confirm(),
            CaptureCommand::Ocr => self.copy_ocr_text(),
            CaptureCommand::Pin => self.pin(event_loop),
            CaptureCommand::Failed(failure) => self.handle_session_failure(failure),
        }
    }
}
