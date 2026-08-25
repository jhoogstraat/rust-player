//! The single-line text input the application needs: GPUI ships none. Focus
//! handle, cursor, backspace and character entry, submit on Enter, no
//! selection.

use gpui::{
    App, InteractiveElement, KeyDownEvent, ParentElement, SharedString, Styled, div, prelude::*,
    px, rgb,
};

const BORDER: u32 = 0x2d2d32;
const MUTED: u32 = 0x8b8b91;

/// One editable single-line field.
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

    fn insert(&mut self, ch: &str) {
        let before: String = self.value.chars().take(self.cursor).collect();
        let after: String = self.value.chars().skip(self.cursor).collect();
        self.value = format!("{before}{ch}{after}");
        self.cursor += ch.chars().count();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let before: String = self.value.chars().take(self.cursor - 1).collect();
        let after: String = self.value.chars().skip(self.cursor).collect();
        self.value = format!("{before}{after}");
        self.cursor -= 1;
    }

    /// Handle one keystroke while focused. `Some` means the field was
    /// submitted (Enter); `false` returns mean the key was not consumed so a
    /// parent handler can act on it.
    pub fn key(&mut self, event: &KeyDownEvent) -> Option<bool> {
        let stroke = &event.keystroke;
        // Modifier-only or command chords never edit text.
        if stroke.modifiers.platform
            || stroke.modifiers.control
            || stroke.modifiers.alt
            || stroke.modifiers.function
        {
            return Some(false);
        }
        match stroke.key.as_str() {
            "enter" => return Some(true),
            "escape" => return None, // let the parent blur
            "backspace" => self.backspace(),
            "left" => self.cursor = self.cursor.saturating_sub(1),
            "right" => self.cursor = (self.cursor + 1).min(self.char_count()),
            "home" => self.cursor = 0,
            "end" => self.cursor = self.char_count(),
            _ => {
                // Character entry rides on key_char ("s" → "s", option-s → "ß").
                if let Some(text) = &stroke.key_char {
                    for ch in text.chars() {
                        let before = self.char_count();
                        self.insert(&ch.to_string());
                        if self.char_count() == before && !text.is_empty() {
                            break;
                        }
                    }
                } else {
                    return Some(false);
                }
            }
        }
        Some(false)
    }

    /// Render the field with a visible cursor while focused.
    pub fn render(&self, id: &'static str, window: &gpui::Window) -> impl IntoElement + use<> {
        let focused = self.focus.is_focused(window);
        let display: SharedString = if self.value.is_empty() && !focused {
            self.placeholder.into()
        } else if focused {
            let chars: Vec<char> = self.value.chars().collect();
            let at = self.cursor.min(chars.len());
            let head: String = chars[..at].iter().collect();
            let tail: String = chars[at..].iter().collect();
            format!("{head}|{tail}").into()
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
            .border_color(rgb(BORDER))
            .bg(rgb(0x111114))
            .flex()
            .items_center()
            .text_size(px(13.))
            .text_color(if focused { rgb(0xf4f4f5) } else { rgb(MUTED) })
            .child(display)
    }
}
