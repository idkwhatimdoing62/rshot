use super::{Interaction, InteractionOutcome, Viewport};
use crate::app::*;

macro_rules! editor {
    ($interaction:expr) => {
        match $interaction.editor_mut() {
            Some(editor) => editor,
            None => return CaptureCommand::None,
        }
    };
}

impl Interaction {
    pub(in crate::app::capture_operation) fn handle_event(
        &mut self,
        event: WindowEvent,
        viewport: Viewport,
    ) -> InteractionOutcome {
        self.viewport = viewport;
        self.redraw_requested = false;
        self.ime_requested = None;
        let command = self.handle_active_window_event(event);
        let terminal = !matches!(command, CaptureCommand::None);
        InteractionOutcome {
            command: terminal.then_some(command),
            redraw: !terminal && self.redraw_requested,
            ime: (!terminal).then_some(self.ime_requested).flatten(),
        }
    }

    fn handle_active_window_event(&mut self, event: WindowEvent) -> CaptureCommand {
        match event {
            WindowEvent::CloseRequested => return CaptureCommand::Close,
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    // 文字输入中：字符进缓冲区，退格删字，回车提交，Esc 取消
                    if self.editor().is_some_and(|editor| editor.text_editing) {
                        // 输入法组合中：按键交给 IME（拼音会走 Preedit/Commit），别自己处理，避免重复进缓冲
                        if !editor!(self).ime_preedit.is_empty() {
                            return CaptureCommand::None;
                        }
                        if event.physical_key == PhysicalKey::Code(KeyCode::Backspace) {
                            if let Some(last) = editor!(self).annotations.last_mut()
                                && let Shape::Text(_, text) = &mut last.shape
                            {
                                text.pop();
                                self.bump_revision();
                            }
                            self.update_ime_area();
                            self.request_redraw();
                            return CaptureCommand::None;
                        }
                        if event.physical_key == PhysicalKey::Code(KeyCode::Enter) {
                            self.commit_text();
                            return CaptureCommand::None;
                        }
                        if let Key::Named(NamedKey::Escape) = event.logical_key {
                            self.cancel_text();
                            return CaptureCommand::None;
                        }
                        if let Some(text) = event.text
                            && text.chars().all(|c| !c.is_control())
                        {
                            if let Some(last) = editor!(self).annotations.last_mut()
                                && let Shape::Text(_, buf) = &mut last.shape
                            {
                                buf.push_str(text.as_str());
                                self.bump_revision();
                            }
                            self.update_ime_area();
                            self.request_redraw();
                        }
                        return CaptureCommand::None;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyP) {
                        return CaptureCommand::Pin;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyZ)
                        && self.modifiers.control_key()
                        && self.is_editing()
                    {
                        editor!(self).annotations.pop();
                        self.bump_revision();
                        self.request_redraw();
                        return CaptureCommand::None;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyB) && self.is_editing() {
                        editor!(self).tool = Tool::Pen;
                        self.request_redraw();
                        return CaptureCommand::None;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyN) && self.is_editing() {
                        editor!(self).tool = Tool::Line;
                        self.request_redraw();
                        return CaptureCommand::None;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyM) && self.is_editing() {
                        editor!(self).tool = Tool::Rect;
                        self.request_redraw();
                        return CaptureCommand::None;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyT) && self.is_editing() {
                        editor!(self).tool = Tool::Text;
                        self.request_redraw();
                        return CaptureCommand::None;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyR) && self.is_editing() {
                        self.reselect();
                        return CaptureCommand::None;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyO) && self.is_editing() {
                        return CaptureCommand::Ocr;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyC) {
                        return CaptureCommand::Copy;
                    }
                    if let Key::Named(NamedKey::Escape) = event.logical_key {
                        if editor!(self).palette_open {
                            self.close_palette();
                            self.request_redraw();
                            return CaptureCommand::None;
                        }
                        return CaptureCommand::Close;
                    }
                }
            }
            WindowEvent::Ime(ime) => {
                // 输入法组合：只处理正在编辑文字时
                if self.editor().is_some_and(|editor| editor.text_editing) {
                    match ime {
                        Ime::Preedit(text, _cursor) => {
                            // 首次进入组合：若键盘事件已把同样的拼音塞进草稿尾部，先去掉避免重复
                            if editor!(self).ime_preedit.is_empty()
                                && !text.is_empty()
                                && let Some(last) = editor!(self).annotations.last_mut()
                                && let Shape::Text(_, buf) = &mut last.shape
                            {
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
                                    self.bump_revision();
                                }
                            }
                            editor!(self).ime_preedit = text;
                            self.update_ime_area();
                            self.request_redraw();
                        }
                        Ime::Commit(text) => {
                            editor!(self).ime_preedit.clear();
                            if let Some(last) = editor!(self).annotations.last_mut()
                                && let Shape::Text(_, buf) = &mut last.shape
                            {
                                buf.push_str(&text);
                                self.bump_revision();
                            }
                            self.update_ime_area();
                            self.request_redraw();
                        }
                        Ime::Enabled => {
                            self.update_ime_area();
                        }
                        Ime::Disabled | Ime::DeleteSurrounding { .. } => {
                            editor!(self).ime_preedit.clear();
                            self.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::PointerMoved { position, .. } => {
                self.cursor = (position.x as i32, position.y as i32);
                if self.is_editing() {
                    let hover = toolbar_hit(
                        self.cursor,
                        self.viewport.width as i32,
                        self.viewport.height as i32,
                        self.selection(),
                    );
                    let hover_index = hover.map(toolbar_item_slot);
                    let hover_changed = hover_index != editor!(self).toolbar_hover;
                    editor!(self).toolbar_hover = hover_index;
                    let palette_hover = if editor!(self).palette_open {
                        palette_hit(
                            self.cursor,
                            self.viewport.width as i32,
                            self.viewport.height as i32,
                            self.selection(),
                        )
                    } else {
                        None
                    };
                    let palette_changed = palette_hover != editor!(self).palette_hover;
                    editor!(self).palette_hover = palette_hover;
                    if editor!(self).drawing {
                        self.update_draft(self.cursor);
                        self.request_redraw();
                    } else if hover_changed || palette_changed {
                        self.request_redraw();
                    }
                    return CaptureCommand::None;
                }
                let before = self.selection();
                match self.start() {
                    // 按住中：移动超过 4 像素才算拖框，否则保持（留给单击截窗）
                    Some(anchor) => {
                        if (self.cursor.0 - anchor.0).abs() > 4
                            || (self.cursor.1 - anchor.1).abs() > 4
                        {
                            self.set_dragged(true);
                            self.set_selection(Some((anchor, self.cursor)));
                        }
                    }
                    // 没按住：悬停锁定光标下的窗口。但已手动拖过框就别再冲掉它
                    None => {
                        if !self.manual() {
                            self.set_selection(self.window_under_cursor());
                        }
                    }
                }
                // 选框变了才重画，省得原地不动也刷屏
                if self.selection() != before {
                    self.request_redraw();
                }
            }
            WindowEvent::PointerButton { state, button, .. } => {
                let mb = button.mouse_button();
                if self.is_editing() {
                    let toolbar_item = toolbar_hit(
                        self.cursor,
                        self.viewport.width as i32,
                        self.viewport.height as i32,
                        self.selection(),
                    );
                    let palette_swatch = if editor!(self).palette_open {
                        palette_hit(
                            self.cursor,
                            self.viewport.width as i32,
                            self.viewport.height as i32,
                            self.selection(),
                        )
                    } else {
                        None
                    };
                    if mb == Some(MouseButton::Left) {
                        match state {
                            ElementState::Pressed => {
                                // 优先级：色板色块 > 工具栏按钮 > 画布
                                if let Some(i) = palette_swatch {
                                    editor!(self).palette_pressed = Some(i);
                                    return CaptureCommand::None;
                                }
                                if toolbar_item.is_some() {
                                    editor!(self).toolbar_pressed =
                                        toolbar_item.map(toolbar_item_slot);
                                    return CaptureCommand::None;
                                }
                                // 色板开着时点画布只关菜单，不画标注
                                if editor!(self).palette_open {
                                    self.close_palette();
                                    return CaptureCommand::None;
                                }
                                if point_in_selection(self.cursor, self.selection()) {
                                    if editor!(self).tool == Tool::Text {
                                        self.start_text(self.cursor);
                                    } else {
                                        editor!(self).drawing = true;
                                        self.start_shape(self.cursor);
                                    }
                                }
                            }
                            ElementState::Released => {
                                if let Some(pressed) = editor!(self).palette_pressed.take() {
                                    if palette_swatch == Some(pressed) {
                                        self.set_color(pressed);
                                        self.close_palette();
                                        self.request_redraw();
                                    }
                                    return CaptureCommand::None;
                                }
                                if let Some(item) = toolbar_item {
                                    let pressed = editor!(self).toolbar_pressed.take();
                                    let slot = toolbar_item_slot(item);
                                    if pressed == Some(slot) {
                                        return self.apply_toolbar_item(item);
                                    }
                                    return CaptureCommand::None;
                                }
                                if editor!(self).drawing {
                                    self.commit_draft();
                                    self.request_redraw();
                                    editor!(self).drawing = false;
                                }
                            }
                        }
                        return CaptureCommand::None;
                    }
                    if state == ElementState::Released {
                        editor!(self).toolbar_pressed = None;
                        editor!(self).palette_pressed = None;
                    }
                }
                // 右键抬起 = 确认（有手动框裁框，否则全屏）。
                // 必须等抬起：若按下就关遮罩，抬起那半下会漏给下面窗口，触发系统右键菜单
                if mb == Some(MouseButton::Right) && state == ElementState::Released {
                    return CaptureCommand::Copy;
                } else if mb == Some(MouseButton::Left) {
                    if self.is_editing() {
                        match state {
                            ElementState::Pressed => {
                                if point_in_selection(self.cursor, self.selection()) {
                                    editor!(self).drawing = true;
                                    self.start_shape(self.cursor);
                                }
                            }
                            ElementState::Released => {
                                if editor!(self).drawing {
                                    self.commit_draft();
                                    self.request_redraw();
                                }
                                editor!(self).drawing = false;
                            }
                        }
                        return CaptureCommand::None;
                    }
                    match state {
                        ElementState::Pressed => {
                            // 按下先记锚点；sel 保持（可能是悬停锁定的窗口），供单击截取
                            self.set_start(Some(self.cursor));
                            self.set_dragged(false);
                            self.set_manual(false); // 重新开框，解除上次的手动锁定
                        }
                        ElementState::Released => {
                            let was_drag = self.dragged();
                            self.set_start(None);
                            self.set_dragged(false);
                            // 单击锁定窗口，或有效拖框后进入编辑态；零面积拖框继续选择。
                            self.finish_pointer_selection(was_drag);
                            self.request_redraw();
                        }
                    }
                }
            }
            _ => (),
        }
        CaptureCommand::None
    }
}
