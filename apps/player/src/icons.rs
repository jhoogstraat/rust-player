//! Embedded icon assets + the gpui [`AssetSource`] that serves them.
//!
//! The glyphs come from the Solar Icons set (Linear weight) by 480
//! Design, the same files the Comet shell embeds (CC BY 4.0; attribution:
//! "Solar Icons by 480 Design"). Icons render via [`icon`]:
//! `icon(icons::STAR).size(px(16.)).text_color(…)` — gpui tints SVGs with
//! the text color.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString, Styled as _, Svg, svg};

macro_rules! icon_assets {
    ($(($const_name:ident, $path:literal)),+ $(,)?) => {
        $(pub const $const_name: &str = concat!("icons/", $path, ".svg");)+

        /// Serves the embedded icons to gpui's SVG renderer.
        pub struct Assets;

        impl AssetSource for Assets {
            fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
                Ok(match path {
                    $(concat!("icons/", $path, ".svg") => Some(Cow::Borrowed(
                        include_bytes!(concat!("../assets/icons/", $path, ".svg")).as_slice(),
                    )),)+
                    _ => None,
                })
            }

            fn list(&self, path: &str) -> Result<Vec<SharedString>> {
                let all = [$(concat!("icons/", $path, ".svg")),+];
                Ok(all
                    .iter()
                    .filter(|p| p.starts_with(path))
                    .map(|p| SharedString::from(*p))
                    .collect())
            }
        }
    };
}

icon_assets![
    // Liked songs (also the favorites affordance at large).
    (STAR, "star"),
    (PLAY, "play"),
    (PAUSE, "pause"),
    (MAGNIFIER, "magnifier"),
    (CLOCK_CIRCLE, "clock-circle"),
    (LIST, "list"),
];

/// An icon element for an embedded asset path. Size and colour are set by
/// the caller.
pub fn icon(path: &'static str) -> Svg {
    svg().path(path).flex_none()
}
