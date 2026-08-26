//! The single-line text input the application needs: GPUI ships none. Focus
//! handle, cursor, backspace and character entry, submit on Enter, no
//! selection.

use gpui::{
    App, InteractiveElement, KeyDownEvent, ParentElement, SharedString, Styled, div, prelude::*,
    px, rgb,
};

const MUTED: u32 = 0x8b8b91;

/// What one keystroke did to a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    /// Enter: the parent takes `value`.
    Submitted,
    /// The text or cursor changed; re-render.
    Edited,
    /// Escape: the parent should move focus away.
    Blur,
    /// Not a text key (command chords, unknown keys); nothing changed.
    Ignored,
}

/// One editable single-line field. `cursor` counts characters, not bytes.
pub struct TextField {
    pub focus: gpui::FocusHandle,
    pub value: String,
    pub cursor: usize,
    pub placeholder: &'static str,
}

impl TextField {
    pub fn new(cx: &mut App, placeholder: &'static str) -> Self {
        Self {
            focus: cx.focus_handle(),
            value: String::new(),
            cursor: 0,
            placeholder,
        }
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    fn char_count(&self) -> usize {
        self.value.chars().count()
    }

    /// Byte offset of the `chars`-th character (or the end).
    fn byte_offset(&self, chars: usize) -> usize {
        self.value
            .char_indices()
            .nth(chars)
            .map_or(self.value.len(), |(offset, _)| offset)
    }

    fn insert(&mut self, text: &str) {
        let at = self.byte_offset(self.cursor);
        self.value.insert_str(at, text);
        self.cursor += text.chars().count();
    }

    /// Remove the character before the cursor.
    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_offset(self.cursor - 1);
        let end = self.byte_offset(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    /// Remove the character under the cursor.
    fn delete(&mut self) {
        let start = self.byte_offset(self.cursor);
        let end = self.byte_offset(self.cursor + 1);
        self.value.replace_range(start..end, "");
    }

    /// Handle one keystroke while focused.
    pub fn key(&mut self, event: &KeyDownEvent) -> KeyOutcome {
        let stroke = &event.keystroke;
        // Command chords never edit text.
        if stroke.modifiers.platform
            || stroke.modifiers.control
            || stroke.modifiers.alt
            || stroke.modifiers.function
        {
            return KeyOutcome::Ignored;
        }
        match stroke.key.as_str() {
            "enter" => return KeyOutcome::Submitted,
            "escape" => return KeyOutcome::Blur,
            "backspace" => self.backspace(),
            "delete" => self.delete(),
            "left" => self.cursor = self.cursor.saturating_sub(1),
            "right" => self.cursor = (self.cursor + 1).min(self.char_count()),
            "home" => self.cursor = 0,
            "end" => self.cursor = self.char_count(),
            // Character entry rides on key_char ("s" → "s", option-s → "ß");
            // control characters (tab, newline) are not text.
            _ => match stroke.key_char.as_deref() {
                Some(text) if !text.is_empty() && !text.chars().any(char::is_control) => {
                    self.insert(text)
                }
                _ => return KeyOutcome::Ignored,
            },
        }
        KeyOutcome::Edited
    }

    /// Render the field with a visible cursor while focused.
    pub fn render(&self, id: &'static str, window: &gpui::Window) -> impl IntoElement + use<> {
        let focused = self.focus.is_focused(window);
        let display: SharedString = if focused {
            let at = self.byte_offset(self.cursor);
            format!("{}|{}", &self.value[..at], &self.value[at..]).into()
        } else if self.value.is_empty() {
            self.placeholder.into()
        } else {
            self.value.clone().into()
        };

        div()
            .id(id)
            .track_focus(&self.focus)
            .h(px(34.))
            .px(px(10.))
            .rounded(px(7.))
            .border_1()
            .border_color(crate::border())
            .bg(crate::tone(0x111114, 0.50))
            .flex()
            .items_center()
            .text_size(px(13.))
            .text_color(if focused { rgb(0xf4f4f5) } else { rgb(MUTED) })
            .child(display)
    }
}
