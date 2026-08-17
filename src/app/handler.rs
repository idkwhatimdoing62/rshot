use super::*;

impl App {
    fn handle_pinned_window_event(&mut self, id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.close_pin(id);
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && matches!(event.logical_key, Key::Named(NamedKey::Escape)) =>
            {
                self.close_pin(id);
            }
            WindowEvent::PointerMoved { position, .. } => {
                if let Some(pin) = self.pins.get_mut(&id) {
                    pin.set_cursor((position.x as i32, position.y as i32));
                    if let Some(cursor) = cursor_position() {
                        pin.drag_to(cursor);
                    }
                }
            }
            WindowEvent::PointerButton { state, button, .. } => {
                let mouse_button = button.mouse_button();
                if mouse_button == Some(MouseButton::Right) && state == ElementState::Released {
                    self.close_pin(id);
                    return;
                }
                if mouse_button != Some(MouseButton::Left) {
                    return;
                }
                match state {
                    ElementState::Pressed => {
                        let close_hit = self
                            .pins
                            .get(&id)
                            .is_some_and(PinnedWindow::close_button_hit);
                        if close_hit {
                            self.close_pin(id);
                            return;
                        }
                        if let (Some(pin), Some(cursor)) =
                            (self.pins.get_mut(&id), cursor_position())
                        {
                            pin.begin_drag(cursor);
                        }
                    }
                    ElementState::Released => {
                        if let Some(pin) = self.pins.get_mut(&id) {
                            pin.end_drag();
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let failure = self.pins.get_mut(&id).and_then(|pin| pin.redraw().err());
                if let Some(failure) = failure {
                    self.handle_pin_failure(id, failure);
                }
            }
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    // 本程序不在启动时建窗口，遮罩是热键触发后临时建的，这里留空
    fn can_create_surfaces(&mut self, _event_loop: &dyn ActiveEventLoop) {}

    // 每轮空闲：轮询 global-hotkey 的事件通道
    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        // 启动后立即检查一次，之后每 12 小时检查一次。
        self.cleanup_temp_files_if_due(Instant::now());
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
        if self.pins.contains_key(&id) {
            self.handle_pinned_window_event(id, event);
            return;
        }
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
