use super::*;

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
            } else if ev.id == self.shot_id && self.window.is_none() {
                // 没在框选时才响应，避免叠窗
                self.open_overlay(event_loop);
            }
        }
        // 托盘双击只显示程序与当前快捷键信息，不触发截图。
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = ev {
                show_message(&self.about_message, false);
            }
        }
        // 文字输入光标闪烁：每 ~530ms 翻转一次可见性
        if self.text_editing && self.mode == Mode::Editing {
            let now = Instant::now();
            match self.last_blink {
                Some(last) if now.duration_since(last) >= Duration::from_millis(530) => {
                    self.cursor_visible = !self.cursor_visible;
                    self.last_blink = Some(now);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
                None => {
                    self.last_blink = Some(now);
                    self.cursor_visible = true;
                }
                _ => {}
            }
        } else {
            self.last_blink = None;
            self.cursor_visible = true;
        }
        // ponytail: 120ms 轮询一次热键。想零延迟得用 EventLoopProxy 唤醒，暂不需要
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(120),
        ));
    }

    fn window_event(
        &mut self,
        _event_loop: &dyn ActiveEventLoop,
        id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if window.id() != id {
            return;
        }
        match event {
            WindowEvent::CloseRequested => self.close_overlay(),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    // 文字输入中：字符进缓冲区，退格删字，回车提交，Esc 取消
                    if self.text_editing && self.mode == Mode::Editing {
                        // 输入法组合中：按键交给 IME（拼音会走 Preedit/Commit），别自己处理，避免重复进缓冲
                        if !self.ime_preedit.is_empty() {
                            return;
                        }
                        if event.physical_key == PhysicalKey::Code(KeyCode::Backspace) {
                            if let Some(last) = self.annotations.last_mut() {
                                if let Shape::Text(_, text) = &mut last.shape {
                                    text.pop();
                                }
                            }
                            self.update_ime_area();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        if event.physical_key == PhysicalKey::Code(KeyCode::Enter) {
                            self.commit_text();
                            return;
                        }
                        if let Key::Named(NamedKey::Escape) = event.logical_key {
                            self.cancel_text();
                            return;
                        }
                        if let Some(text) = event.text {
                            if text.chars().all(|c| !c.is_control()) {
                                if let Some(last) = self.annotations.last_mut() {
                                    if let Shape::Text(_, buf) = &mut last.shape {
                                        buf.push_str(text.as_str());
                                    }
                                }
                                self.update_ime_area();
                                if let Some(w) = &self.window {
                                    w.request_redraw();
                                }
                            }
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyP)
                        && self.mode != Mode::Pinned
                    {
                        self.pin();
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyZ)
                        && self.modifiers.control_key()
                        && self.mode == Mode::Editing
                    {
                        self.annotations.pop();
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyB)
                        && self.mode == Mode::Editing
                    {
                        self.tool = Tool::Pen;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyN)
                        && self.mode == Mode::Editing
                    {
                        self.tool = Tool::Line;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyM)
                        && self.mode == Mode::Editing
                    {
                        self.tool = Tool::Rect;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyT)
                        && self.mode == Mode::Editing
                    {
                        self.tool = Tool::Text;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyR)
                        && self.mode == Mode::Editing
                    {
                        self.reselect();
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyO)
                        && self.mode == Mode::Editing
                    {
                        self.copy_ocr_text();
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyC)
                        && self.mode != Mode::Pinned
                    {
                        self.confirm();
                        return;
                    }
                    if let Key::Named(NamedKey::Escape) = event.logical_key {
                        if self.palette_open {
                            self.close_palette();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        self.close_overlay();
                    }
                }
            }
            WindowEvent::Ime(ime) => {
                // 输入法组合：只处理正在编辑文字时
                if self.text_editing && self.mode == Mode::Editing {
                    match ime {
                        Ime::Preedit(text, _cursor) => {
                            // 首次进入组合：若键盘事件已把同样的拼音塞进草稿尾部，先去掉避免重复
                            if self.ime_preedit.is_empty() && !text.is_empty() {
                                if let Some(last) = self.annotations.last_mut() {
                                    if let Shape::Text(_, buf) = &mut last.shape {
                                        let n = text.chars().count();
                                        let tail: String = buf
                                            .chars()
                                            .rev()
                                            .take(n)
                                            .collect::<Vec<_>>()
                                            .into_iter()
                                            .rev()
                                            .collect();
                                        if tail == text {
                                            for _ in 0..n {
                                                buf.pop();
                                            }
                                        }
                                    }
                                }
                            }
                            self.ime_preedit = text;
                            self.update_ime_area();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                        Ime::Commit(text) => {
                            self.ime_preedit.clear();
                            if let Some(last) = self.annotations.last_mut() {
                                if let Shape::Text(_, buf) = &mut last.shape {
                                    buf.push_str(&text);
                                }
                            }
                            self.update_ime_area();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                        Ime::Enabled => {
                            self.update_ime_area();
                        }
                        Ime::Disabled | Ime::DeleteSurrounding { .. } => {
                            self.ime_preedit.clear();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::PointerMoved { position, .. } => {
                self.cur = (position.x as i32, position.y as i32);
                if self.mode == Mode::Editing {
                    let hover = self
                        .window
                        .as_ref()
                        .map(|w| w.surface_size())
                        .and_then(|size| {
                            toolbar_hit(self.cur, size.width as i32, size.height as i32, self.sel)
                        });
                    let hover_index = hover.map(toolbar_item_slot);
                    let hover_changed = hover_index != self.toolbar_hover;
                    self.toolbar_hover = hover_index;
                    let palette_hover = if self.palette_open {
                        self.window.as_ref().and_then(|w| {
                            let size = w.surface_size();
                            palette_hit(self.cur, size.width as i32, size.height as i32, self.sel)
                        })
                    } else {
                        None
                    };
                    let palette_changed = palette_hover != self.palette_hover;
                    self.palette_hover = palette_hover;
                    if self.drawing {
                        self.update_draft(self.cur);
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    } else if hover_changed || palette_changed {
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                    return;
                }
                if self.mode == Mode::Pinned {
                    if let Some((cursor_start, window_start)) = self.pin_drag {
                        if let Some(cursor) = cursor_position() {
                            if let Some(w) = &self.window {
                                let x = window_start.0 + cursor.0 - cursor_start.0;
                                let y = window_start.1 + cursor.1 - cursor_start.1;
                                w.set_outer_position(PhysicalPosition::new(x, y).into());
                            }
                        }
                    }
                    return;
                }
                let before = self.sel;
                match self.start {
                    // 按住中：移动超过 4 像素才算拖框，否则保持（留给单击截窗）
                    Some(anchor) => {
                        if (self.cur.0 - anchor.0).abs() > 4 || (self.cur.1 - anchor.1).abs() > 4 {
                            self.dragged = true;
                            self.sel = Some((anchor, self.cur));
                        }
                    }
                    // 没按住：悬停锁定光标下的窗口。但已手动拖过框就别再冲掉它
                    None => {
                        if !self.manual {
                            self.sel = self.window_under_cursor();
                        }
                    }
                }
                // 选框变了才重画，省得原地不动也刷屏
                if self.sel != before {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::PointerButton { state, button, .. } => {
                let mb = button.mouse_button();
                if self.mode == Mode::Editing {
                    let toolbar_item =
                        self.window
                            .as_ref()
                            .map(|w| w.surface_size())
                            .and_then(|size| {
                                toolbar_hit(
                                    self.cur,
                                    size.width as i32,
                                    size.height as i32,
                                    self.sel,
                                )
                            });
                    let palette_swatch = if self.palette_open {
                        self.window.as_ref().and_then(|w| {
                            let size = w.surface_size();
                            palette_hit(self.cur, size.width as i32, size.height as i32, self.sel)
                        })
                    } else {
                        None
                    };
                    if mb == Some(MouseButton::Left) {
                        match state {
                            ElementState::Pressed => {
                                // 优先级：色板色块 > 工具栏按钮 > 画布
                                if let Some(i) = palette_swatch {
                                    self.palette_pressed = Some(i);
                                    return;
                                }
                                if toolbar_item.is_some() {
                                    self.toolbar_pressed = toolbar_item.map(toolbar_item_slot);
                                    return;
                                }
                                // 色板开着时点画布只关菜单，不画标注
                                if self.palette_open {
                                    self.close_palette();
                                    return;
                                }
                                if point_in_selection(self.cur, self.sel) {
                                    if self.tool == Tool::Text {
                                        self.start_text(self.cur);
                                    } else {
                                        self.drawing = true;
                                        self.start_shape(self.cur);
                                    }
                                }
                            }
                            ElementState::Released => {
                                if let Some(pressed) = self.palette_pressed.take() {
                                    if palette_swatch == Some(pressed) {
                                        self.set_color(pressed);
                                        self.close_palette();
                                        if let Some(w) = &self.window {
                                            w.request_redraw();
                                        }
                                    }
                                    return;
                                }
                                if let Some(item) = toolbar_item {
                                    let pressed = self.toolbar_pressed.take();
                                    let slot = toolbar_item_slot(item);
                                    if pressed == Some(slot) {
                                        self.apply_toolbar_item(item);
                                    }
                                    return;
                                }
                                if self.drawing {
                                    self.commit_draft();
                                    if let Some(w) = &self.window {
                                        w.request_redraw();
                                    }
                                    self.drawing = false;
                                }
                            }
                        }
                        return;
                    }
                    if state == ElementState::Released {
                        self.toolbar_pressed = None;
                        self.palette_pressed = None;
                    }
                }
                if self.mode == Mode::Pinned {
                    if mb == Some(MouseButton::Right) && state == ElementState::Released {
                        self.close_overlay();
                    } else if mb == Some(MouseButton::Left) {
                        let close_hit = self
                            .window
                            .as_ref()
                            .map(|w| {
                                let size = w.surface_size();
                                let r = pin_close_rect(size.width as i32, size.height as i32);
                                self.cur.0 >= r.0
                                    && self.cur.0 < r.2
                                    && self.cur.1 >= r.1
                                    && self.cur.1 < r.3
                            })
                            .unwrap_or(false);
                        match state {
                            ElementState::Pressed => {
                                if close_hit {
                                    self.close_overlay();
                                    return;
                                }
                                if let Some(cursor) = cursor_position() {
                                    if let Some(pos) =
                                        self.window.as_ref().and_then(|w| w.outer_position().ok())
                                    {
                                        self.pin_drag = Some((cursor, (pos.x, pos.y)));
                                    }
                                }
                            }
                            ElementState::Released => {
                                self.pin_drag = None;
                            }
                        }
                    }
                    return;
                }
                // 右键抬起 = 确认（有手动框裁框，否则全屏）。
                // 必须等抬起：若按下就关遮罩，抬起那半下会漏给下面窗口，触发系统右键菜单
                if mb == Some(MouseButton::Right) && state == ElementState::Released {
                    self.confirm();
                } else if mb == Some(MouseButton::Left) {
                    if self.mode == Mode::Editing {
                        match state {
                            ElementState::Pressed => {
                                if point_in_selection(self.cur, self.sel) {
                                    self.drawing = true;
                                    self.start_shape(self.cur);
                                }
                            }
                            ElementState::Released => {
                                if self.drawing {
                                    self.commit_draft();
                                    if let Some(w) = &self.window {
                                        w.request_redraw();
                                    }
                                }
                                self.drawing = false;
                            }
                        }
                        return;
                    }
                    match state {
                        ElementState::Pressed => {
                            // 按下先记锚点；sel 保持（可能是悬停锁定的窗口），供单击截取
                            self.start = Some(self.cur);
                            self.dragged = false;
                            self.manual = false; // 重新开框，解除上次的手动锁定
                        }
                        ElementState::Released => {
                            let was_drag = self.dragged;
                            self.start = None;
                            self.dragged = false;
                            if !was_drag {
                                // 单击锁定窗口后进入编辑态，避免误触直接结束。
                                if self.sel.is_some() {
                                    self.manual = true;
                                    self.mode = Mode::Editing;
                                    self.toolbar_hover = None;
                                    self.toolbar_pressed = None;
                                }
                            }
                            // 拖框后进入编辑态：工具栏选工具/颜色，左键画标注。
                            else {
                                self.manual = true;
                                self.mode = Mode::Editing;
                                self.toolbar_hover = None;
                                self.toolbar_pressed = None;
                            }
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(failure) = self.redraw_session(id) {
                    self.handle_session_failure(failure);
                }
            }
            _ => (),
        }
    }
}
