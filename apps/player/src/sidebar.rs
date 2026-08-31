//! The fixed left-edge navigation sidebar, following the Comet shell's
//! column pattern: a fixed-width column of nav rows with the bottom item
//! pinned, its own slightly lighter surface, and a right hairline. Row
//! geometry is Comet's settings-nav recipe (8px-rounded row, 16px icon,
//! 13px label; selection = white wash + medium weight, hover brightens).

use gpui::{
    Context, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, prelude::*, px,
};
use player_core::{Command, LibrarySection};

use crate::{MUTED, rgb};
use crate::{PANEL, PlayerApp, TEXT, border, icons, tone, wash};

/// Fixed sidebar width (Comet's default).
pub(crate) const SIDEBAR_WIDTH: f32 = 256.0;

/// Where the app is navigated. Library sections browse the second column;
/// Settings swaps the main area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavSection {
    Search,
    LikedSongs,
    RecentlyPlayed,
    Playlists,
    Settings,
}

impl NavSection {
    /// The library section this nav destination browses, if any.
    pub(crate) fn library(&self) -> Option<LibrarySection> {
        match self {
            NavSection::Search => None,
            NavSection::LikedSongs => Some(LibrarySection::LikedSongs),
            NavSection::RecentlyPlayed => Some(LibrarySection::RecentlyPlayed),
            NavSection::Playlists => Some(LibrarySection::Playlists),
            NavSection::Settings => None,
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            NavSection::Search => icons::MAGNIFIER,
            NavSection::LikedSongs => icons::STAR,
            NavSection::RecentlyPlayed => icons::CLOCK_CIRCLE,
            NavSection::Playlists => icons::LIST,
            NavSection::Settings => icons::SETTINGS_MINIMALISTIC,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            NavSection::Search => "Search",
            NavSection::LikedSongs => "Liked songs",
            NavSection::RecentlyPlayed => "Recently played",
            NavSection::Playlists => "Playlists",
            NavSection::Settings => "Settings",
        }
    }
}

const NAV_ITEMS: [NavSection; 4] = [
    NavSection::LikedSongs,
    NavSection::Search,
    NavSection::RecentlyPlayed,
    NavSection::Playlists,
];

/// The sidebar column: nav rows on top, Settings pinned to the bottom.
pub(crate) fn render_sidebar(app: &PlayerApp, cx: &mut Context<PlayerApp>) -> impl IntoElement {
    div()
        .w(px(SIDEBAR_WIDTH))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .bg(tone(PANEL, 0.45))
        .border_r_1()
        .border_color(border())
        .px(px(8.0))
        .pt(px(10.0))
        .pb(px(8.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .children(NAV_ITEMS.map(|section| nav_row(section, app.nav, cx))),
        )
        .child(div().flex_1())
        .child(nav_row(NavSection::Settings, app.nav, cx))
}

/// One nav row — Comet's recipe: selected rows wear the wash and medium
/// weight; the rest sit muted and brighten on hover.
fn nav_row(
    section: NavSection,
    active: NavSection,
    cx: &mut Context<PlayerApp>,
) -> gpui::Stateful<gpui::Div> {
    let selected = section == active;
    div()
        .id(SharedString::from(format!("nav-{}", section.label())))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .rounded(px(8.0))
        .px(px(8.0))
        .py(px(6.0))
        .text_size(px(13.0))
        .when(selected, |el| {
            el.bg(wash(0.11))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(TEXT))
        })
        .when(!selected, |el| el.text_color(rgb(MUTED)))
        .cursor_pointer()
        .hover(|style| style.bg(wash(0.07)).text_color(rgb(TEXT)))
        .on_click(cx.listener(move |app, _, _, cx| {
            app.nav = section;
            if let Some(library) = section.library() {
                app.send(Command::Browse(library));
            }
            cx.notify();
        }))
        .child(
            crate::icons::icon(section.icon())
                .size(px(16.0))
                .text_color(rgb(MUTED)),
        )
        .child(section.label())
}
