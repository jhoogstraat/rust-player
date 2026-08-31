//! Embedded icon assets + the gpui [`AssetSource`] that serves them.
//!
//! The four glyphs come from the Solar Icons set (Linear weight) by 480
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
    (MAGNIFIER, "magnifier"),
    (CLOCK_CIRCLE, "clock-circle"),
    (LIST, "list"),
    (SETTINGS_MINIMALISTIC, "settings-minimalistic"),
];

/// An icon element for an embedded asset path. Size and colour are set by
/// the caller.
pub fn icon(path: &'static str) -> Svg {
    svg().path(path).flex_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_icon_loads_and_parses() {
        let assets = Assets;
        for path in assets.list("icons/").unwrap() {
            let bytes = assets
                .load(&path)
                .unwrap()
                .unwrap_or_else(|| panic!("missing asset {path}"));
            let text = std::str::from_utf8(&bytes).expect("icon svg is utf-8");
            assert!(text.contains("<svg"), "{path} is not an svg");
            assert!(text.contains("viewBox"), "{path} lacks a viewBox");
        }
    }
}
