//! The one place the app talks to AppKit directly.

use objc::runtime::{BOOL, Object, YES};
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// `NSVisualEffectMaterialHUDWindow`.
const HUD_WINDOW: isize = 13;

/// Re-point GPUI's blur view at a material that still blurs. GPUI asks for
/// the `Selection` material, which macOS 27 draws as a flat layer with no
/// backdrop blur, leaving the window merely see-through.
// ponytail: reaches past GPUI into AppKit; delete this file once the pinned
// GPUI picks a material that blurs on macOS 27.
pub(crate) fn restore(window: &gpui::Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: the handle is this window's live NSView and we are on the main
    // thread; messages to nil are no-ops.
    unsafe {
        let view = handle.ns_view.as_ptr() as *mut Object;
        let ns_window: *mut Object = msg_send![view, window];
        let content: *mut Object = msg_send![ns_window, contentView];
        let subviews: *mut Object = msg_send![content, subviews];
        let count: usize = msg_send![subviews, count];
        for index in 0..count {
            let subview: *mut Object = msg_send![subviews, objectAtIndex: index];
            let is_blur: BOOL = msg_send![subview, isKindOfClass: class!(NSVisualEffectView)];
            if is_blur == YES {
                let _: () = msg_send![subview, setMaterial: HUD_WINDOW];
            }
        }
    }
}
