//! The words of the playing track: Spicy Lyrics Mobile's renderer, in the
//! Now Playing panel's card and in full screen beside the cover.

use egui::{
    Align, Color32, CornerRadius, Frame, Layout, Margin, Rect, Sense, UiBuilder, pos2, vec2,
};

use crate::app::App;
use crate::i18n::gettext;
use crate::lyrics::Lyrics;
use crate::model::{Action, Loadable};
use crate::settings::LyricsStyle;
use crate::theme::{self, Icon};

use super::topbar;
use super::widgets;

mod animator;
mod backdrop;
mod bloom;
mod gpu;
mod layout;
mod motion;
mod scroll;
mod stage;
mod timeline;

#[cfg(test)]
pub use stage::drawn;

/// Frees the lyrics renderer's graphics objects while its window's context is
/// current.
pub(crate) fn release_gpu(ctx: &egui::Context, gl: &eframe::glow::Context) {
    gpu::release(ctx, gl);
}

/// What the lyrics view shows instead of words, when it has none.
enum Missing {
    Nothing,
    Loading,
    Offline,
    Failed(String),
    None,
    Instrumental,
}

fn shown_lyrics(app: &App) -> Result<Lyrics, Missing> {
    if app.now_playing().is_none() {
        return Err(Missing::Nothing);
    }
    match &app.lyrics {
        Loadable::NotLoaded | Loadable::Loading if !app.network_reachable => Err(Missing::Offline),
        Loadable::NotLoaded | Loadable::Loading => Err(Missing::Loading),
        Loadable::Failed(_) if !app.network_reachable => Err(Missing::Offline),
        Loadable::Failed(error) => Err(Missing::Failed(error.clone())),
        Loadable::Loaded(None) => Err(Missing::None),
        Loadable::Loaded(Some(lyrics)) if lyrics.instrumental || lyrics.lines.is_empty() => {
            Err(Missing::Instrumental)
        }
        Loadable::Loaded(Some(lyrics)) => Ok(lyrics.clone()),
    }
}

fn missing(app: &mut App, ui: &mut egui::Ui, why: Missing) {
    let palette = theme::Palette::dark();
    let locale = app.locale;
    match why {
        Missing::Nothing => widgets::empty_state(
            ui,
            &palette,
            Icon::Mic,
            &gettext(locale, "Nothing playing"),
            &gettext(locale, "Play a song to see its lyrics."),
        ),
        Missing::Loading => widgets::loading_row(ui, &palette, locale),
        Missing::Offline => widgets::empty_state(
            ui,
            &palette,
            Icon::WifiOff,
            &gettext(locale, "You're offline"),
            &gettext(
                locale,
                "Spotifast will reconnect automatically when your connection returns.",
            ),
        ),
        Missing::Failed(error) => {
            let message = gettext(
                locale,
                // Translators: Keep {error} unchanged. It is the original failure detail.
                "Couldn't fetch the lyrics: {error}",
            )
            .replace("{error}", &error);
            ui.add_space(8.0);
            theme::text(ui, message, theme::regular(13.0), palette.secondary);
            ui.add_space(8.0);
            if theme::pill_button(ui, &palette, &gettext(locale, "Try again"), false).clicked() {
                app.actions.push(Action::RetryLyrics);
            }
        }
        Missing::None => widgets::empty_state(
            ui,
            &palette,
            Icon::Mic,
            &gettext(locale, "No lyrics"),
            &gettext(locale, "No lyrics found for this track."),
        ),
        Missing::Instrumental => widgets::empty_state(
            ui,
            &palette,
            Icon::Music,
            &gettext(locale, "Instrumental"),
            &gettext(locale, "No timed lyrics for this track."),
        ),
    }
}

/// The lyric size for a view `width` points wide: Spicy's
/// `clamp(1.85rem, 7cqw, 3.5rem)`, times the Lyrics Size setting.
fn lyric_size(app: &App, width: f32) -> f32 {
    layout::base_font(timeline::Kind::Syllable, width) * app.settings.lyrics_font_size as f32
        / 100.0
}

/// The lyrics with their credits, painted into `rect`.
fn lyrics_view(app: &mut App, ui: &mut egui::Ui, rect: Rect, id: &str, look: stage::Look) {
    let lyrics = match shown_lyrics(app) {
        Ok(lyrics) => lyrics,
        Err(why) => {
            let mut inner = ui.new_child(
                UiBuilder::new().max_rect(rect.shrink2(vec2(look.pad_left.min(24.0), 16.0))),
            );
            missing(app, &mut inner, why);
            return;
        }
    };
    let clip = ui.clip_rect().intersect(rect);
    let mut view = ui.new_child(UiBuilder::new().max_rect(rect));
    view.set_clip_rect(clip);
    let shown = stage::show(app, &mut view, rect, id, &lyrics, look);
    if app.settings.lyrics_show_credits
        && let Some(footer) = shown.footer
        && footer.top() < rect.bottom()
    {
        let palette = theme::Palette::dark();
        let position_ms = app.now_playing().map_or(0, |now| now.position_ms);
        let mut credits = view.new_child(
            UiBuilder::new()
                .max_rect(Rect::from_min_size(footer.min, vec2(footer.width(), 400.0)))
                .layout(Layout::top_down(Align::Min)),
        );
        credits.set_clip_rect(clip);
        attribution(
            &mut credits,
            &lyrics,
            &palette,
            app.locale,
            position_ms,
            app.settings.lyrics_refined_credits,
            app.settings.lyrics_credits_follow_last_line,
        );
        stage::set_footer_height(ui.ctx(), id, credits.min_rect().height());
    }
}

/// A round, frosted button, as Spicy's view controls are.
fn view_control(ui: &mut egui::Ui, icon: Icon, size: f32, tooltip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();
    let scale = if pressed {
        0.94
    } else if hovered {
        1.05
    } else {
        1.0
    };
    let radius = size * 0.5 * scale;
    let painter = ui.painter();
    painter.circle_filled(
        rect.center(),
        radius,
        Color32::from_white_alpha(if hovered { 31 } else { 20 }),
    );
    painter.circle_stroke(
        rect.center(),
        radius - 0.5,
        egui::Stroke::new(1.0, Color32::from_white_alpha(41)),
    );
    theme::paint_icon(
        ui,
        icon,
        Rect::from_center_size(rect.center(), vec2(18.0, 18.0) * scale),
        18.0 * scale,
        Color32::WHITE,
    );
    response
        .on_hover_text(tooltip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn side_panel(app: &mut App, ui: &mut egui::Ui) {
    let palette = theme::Palette::dark();
    let fit = super::yielding_panel(
        ui.ctx(),
        "lyrics-panel",
        theme::SIDE_PANEL_MIN_WIDTH..=640.0,
        app.settings.lyrics_width,
        ui.available_width() - super::topbar::least_width(ui.ctx()),
    );
    let panel = egui::Panel::right("lyrics-panel")
        .resizable(true)
        .default_size(app.settings.lyrics_width)
        .size_range(fit.range.clone())
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(Color32::from_rgb(18, 18, 20))
                .inner_margin(Margin {
                    left: 16,
                    right: 16,
                    top: 12,
                    bottom: 12,
                }),
        );
    let response = panel.show(ui, |ui| {
        theme::apply_local(ui, &palette);
        let back = ui.max_rect().expand2(vec2(16.0, 12.0));
        let painted = !app.settings.lyrics_low_performance || app.now_playing().is_some();
        if painted && app.now_playing().is_some() {
            backdrop::paint(app, ui, back, (2.65, 0.6));
        }
        let window_controls = super::window_controls_reservation(
            ui.ctx(),
            app.show_queue_panel,
            app.show_lyrics_panel,
            app.show_now_playing_panel,
            ui.available_width(),
        );
        ui.add_space(window_controls.lyrics_top);
        panel_header(app, ui);
        ui.add_space(10.0);
        if app.now_playing().is_some() {
            lyrics_panel_view(app, ui);
        } else {
            missing(app, ui, Missing::Nothing);
        }
    });
    let current_width = response.response.rect.width();
    if (app.settings.lyrics_width - current_width).abs() > 1.0
        && super::panel_width_chosen(ui.ctx(), "lyrics-panel", &fit)
    {
        app.settings.lyrics_width = current_width;
        app.actions.push(Action::SettingsChanged);
    }
}

fn panel_header(app: &mut App, ui: &mut egui::Ui) {
    let palette = theme::Palette::dark();
    let locale = app.locale;
    let mut close = false;
    egui::Sides::new().shrink_left().spacing(6.0).show(
        ui,
        |ui| {
            if let Some(from) = app.playing_from() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(from.name)
                            .font(theme::bold(16.0))
                            .color(palette.text),
                    )
                    .truncate(),
                );
            } else {
                theme::text(
                    ui,
                    gettext(locale, "Lyrics"),
                    theme::bold(16.0),
                    palette.text,
                );
            }
        },
        |ui| {
            close = theme::icon_button(
                ui,
                Icon::X,
                18.0,
                palette.secondary,
                palette.text,
                &gettext(locale, "Close"),
            )
            .clicked();
        },
    );
    if close {
        app.actions.push(Action::ToggleLyricsPanel);
    }
}

/// The lyrics view: the words in Spicy Lyrics' frosted card fill the panel,
/// with the playing song in brief above them when the setting keeps it.
fn lyrics_panel_view(app: &mut App, ui: &mut egui::Ui) {
    let palette = theme::Palette::dark();
    let locale = app.locale;
    let width = ui.available_width();
    // The playing song in brief, as Spotify's lyrics view keeps its Now
    // Playing row above the words: a small cover, the title, the artist.
    if app.settings.lyrics_now_playing_header
        && let Some(now) = app.now_playing()
    {
        egui::Sides::new().shrink_left().spacing(10.0).show(
            ui,
            |ui| {
                let (slot, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
                let art = Rect::from_center_size(slot.center(), vec2(44.0, 44.0));
                widgets::playing_cover(
                    ui,
                    &palette,
                    app.backend.art(),
                    "lyrics-song-brief",
                    app.small_art(&now, 44.0),
                    art,
                    4.0,
                );
            },
            |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&now.title)
                                .font(theme::bold(15.0))
                                .color(palette.text),
                        )
                        .truncate(),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&now.subtitle)
                                .font(theme::regular(12.5))
                                .color(palette.secondary),
                        )
                        .truncate(),
                    );
                });
            },
        );
        ui.add_space(12.0);
    }

    // The card: rgba(255,255,255,0.07), radius 8, the header's 38-point
    // controls, the words filling what is left.
    let card_top = ui.cursor().top();
    let rest = (ui.available_rect_before_wrap().bottom() - card_top).max(120.0);
    let card = Rect::from_min_size(pos2(ui.max_rect().left(), card_top), vec2(width, rest));
    ui.advance_cursor_after_rect(card);
    ui.painter()
        .rect_filled(card, CornerRadius::same(8), Color32::from_white_alpha(18));
    let header = Rect::from_min_size(card.min + vec2(16.0, 8.0), vec2(card.width() - 32.0, 38.0));
    let mut title = ui.new_child(
        UiBuilder::new()
            .max_rect(header)
            .layout(Layout::left_to_right(Align::Center)),
    );
    theme::text(
        &mut title,
        gettext(locale, "Lyrics"),
        theme::bold(16.0),
        Color32::WHITE,
    );
    let mut buttons = ui.new_child(
        UiBuilder::new()
            .max_rect(header)
            .layout(Layout::right_to_left(Align::Center)),
    );
    buttons.spacing_mut().item_spacing.x = 4.0;
    let fullscreen = view_control(
        &mut buttons,
        Icon::Expand,
        38.0,
        &gettext(locale, "Full screen lyrics"),
    );
    let settings = view_control(
        &mut buttons,
        Icon::Settings,
        38.0,
        &gettext(locale, "Lyrics settings"),
    );
    lyrics_settings_popup(app, &settings);
    if fullscreen.clicked() {
        app.actions.push(Action::SetLyricsFullscreen(true));
    }
    let body = Rect::from_min_max(pos2(card.left(), header.bottom()), card.max);
    // The expanded card: clamp(1rem, 8cqw, 2.25rem) sides and lyrics of
    // clamp(2.2rem, 7.5cqw, 3.6rem), the line 26.7% down.
    let w = body.width();
    let side = (w * 0.08).clamp(16.0, 36.0);
    let size = (w * 0.075).clamp(35.2, 57.6) * app.settings.lyrics_font_size as f32 / 100.0;
    let look = stage::Look {
        font: size,
        pad_left: side,
        pad_right: side,
        anchor: stage::Anchor::Top(body.height() * 0.267),
        mask: (8.0, 28.0),
        minimal: false,
    };
    let mut clipped = ui.new_child(UiBuilder::new().max_rect(body));
    clipped.set_clip_rect(body.intersect(ui.clip_rect()));
    lyrics_view(app, &mut clipped, body, "panel", look);
}

/// The lyrics settings, mobile's own: the style, the size, Fade Away
/// Previous Lines, and everything else on the settings page.
fn lyrics_settings_popup(app: &mut App, response: &egui::Response) {
    let palette = theme::Palette::dark();
    egui::Popup::menu(response)
        .frame(widgets::menu_frame(&palette).inner_margin(Margin::same(14)))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            theme::apply_local(ui, &palette);
            let locale = app.locale;
            let mut changed = false;
            ui.set_width(260.0);
            theme::text(
                ui,
                gettext(locale, "Lyrics"),
                theme::bold(15.0),
                palette.text,
            );
            ui.add_space(10.0);
            theme::text(
                ui,
                gettext(locale, "Style"),
                theme::semibold(11.5),
                palette.secondary,
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for style in LyricsStyle::ALL {
                    if theme::soft_button(
                        ui,
                        &palette,
                        None,
                        &style.label(locale),
                        app.settings.lyrics_style == style,
                    )
                    .clicked()
                    {
                        app.settings.lyrics_style = style;
                        changed = true;
                    }
                }
            });
            ui.add_space(10.0);
            theme::text(
                ui,
                gettext(locale, "Lyrics size"),
                theme::semibold(11.5),
                palette.secondary,
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for size in [85, 100, 115, 130, 160] {
                    if theme::soft_button(
                        ui,
                        &palette,
                        None,
                        &format!("{size}%"),
                        app.settings.lyrics_font_size == size,
                    )
                    .clicked()
                    {
                        app.settings.lyrics_font_size = size;
                        changed = true;
                    }
                }
            });
            ui.add_space(10.0);
            theme::text(
                ui,
                gettext(locale, "Line spacing"),
                theme::semibold(11.5),
                palette.secondary,
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for spacing in [85, 100, 115, 130, 160] {
                    if theme::soft_button(
                        ui,
                        &palette,
                        None,
                        &format!("{spacing}%"),
                        app.settings.lyrics_line_spacing == spacing,
                    )
                    .clicked()
                    {
                        app.settings.lyrics_line_spacing = spacing;
                        changed = true;
                    }
                }
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                theme::text(
                    ui,
                    gettext(locale, "Fade away previous lines"),
                    theme::regular(13.0),
                    palette.text,
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    changed |= widgets::switch(
                        ui,
                        &palette,
                        "fade-away",
                        &mut app.settings.lyrics_minimal_mode,
                    )
                    .changed();
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                theme::text(
                    ui,
                    gettext(locale, "Split view"),
                    theme::regular(13.0),
                    palette.text,
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let before = app.settings.lyrics_split_view;
                    if widgets::switch(
                        ui,
                        &palette,
                        "lyrics-split-view",
                        &mut app.settings.lyrics_split_view,
                    )
                    .changed()
                    {
                        if !before && app.lyrics_fullscreen.is_some() {
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                        }
                        changed = true;
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                theme::text(
                    ui,
                    gettext(locale, "Song above the lyrics"),
                    theme::regular(13.0),
                    palette.text,
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    changed |= widgets::switch(
                        ui,
                        &palette,
                        "song-above-lyrics",
                        &mut app.settings.lyrics_now_playing_header,
                    )
                    .changed();
                });
            });
            ui.add_space(8.0);
            widgets::menu_separator(ui, &palette);
            if widgets::menu_item(
                ui,
                &palette,
                Some(Icon::Settings),
                &gettext(locale, "All settings…"),
            ) {
                ui.close();
                app.actions.push(Action::Open(crate::model::Page::Settings));
            }
            if changed {
                app.actions.push(Action::SettingsChanged);
            }
        });
}

pub fn fullscreen(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::default()
        .frame(Frame::new().fill(Color32::BLACK))
        .show(ui, |ui| {
            theme::apply_local(ui, &theme::Palette::dark());
            let rect = ui.max_rect();
            background(app, ui, rect);
            #[cfg(feature = "milkdrop")]
            if ui
                .interact(rect, ui.id().with("lyrics-backdrop"), Sense::click())
                .clicked()
            {
                app.actions.push(Action::ToggleLyricsMilkdrop);
            }
            fullscreen_page(app, ui, rect);
        });
}

/// The Now Playing view: the playing song, its cover and what is queued
/// next, in a right panel of its own. It stands beside the pages, and it is
/// the split view's column.
pub fn now_playing_side_panel(app: &mut App, ui: &mut egui::Ui) {
    let palette = theme::Palette::dark();
    let locale = app.locale;
    let split = app.settings.lyrics_split_view && app.lyrics_fullscreen.is_some();
    let fit = super::yielding_panel(
        ui.ctx(),
        "now-playing-panel",
        260.0..=420.0,
        320.0,
        ui.available_width() - super::topbar::least_width(ui.ctx()),
    );
    egui::Panel::right("now-playing-panel")
        .resizable(true)
        .default_size(320.0)
        .size_range(fit.range.clone())
        .show_separator_line(false)
        .frame(Frame::new().fill(palette.window).inner_margin(Margin {
            left: 18,
            right: 18,
            top: 12,
            bottom: 12,
        }))
        .show(ui, |ui| {
            theme::apply_local(ui, &palette);
            let window_controls = super::window_controls_reservation(
                ui.ctx(),
                app.show_queue_panel,
                app.show_lyrics_panel,
                app.show_now_playing_panel,
                ui.available_width(),
            );
            // Beside the split view's top bar, whose own content sits below
            // the system title bar's space.
            let titlebar = if split {
                theme::titlebar_inset(ui.ctx())
            } else {
                0.0
            };
            ui.add_space(window_controls.now_playing_top.max(titlebar));
            let Some(now) = app.now_playing() else {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::Music,
                    &gettext(locale, "Nothing playing"),
                    &gettext(locale, "Pick a song, album, or playlist"),
                );
                return;
            };
            let mut close = false;
            egui::Sides::new().shrink_left().spacing(6.0).show(
                ui,
                |ui| {
                    theme::text(
                        ui,
                        gettext(locale, "Now playing"),
                        theme::bold(16.0),
                        palette.text,
                    );
                },
                |ui| {
                    close = theme::icon_button(
                        ui,
                        Icon::X,
                        18.0,
                        palette.secondary,
                        palette.text,
                        &gettext(locale, "Close"),
                    )
                    .clicked();
                },
            );
            if close {
                app.actions.push(Action::ToggleNowPlayingPanel);
            }
            ui.add_space(12.0);
            let side = ui.available_width();
            let art = Rect::from_min_size(
                pos2(ui.cursor().left(), ui.cursor().top()),
                vec2(side, side),
            );
            ui.advance_cursor_after_rect(art);
            widgets::playing_cover(
                ui,
                &palette,
                app.backend.art(),
                "now-playing-panel",
                app.small_art(&now, side),
                art,
                8.0,
            );
            ui.add_space(12.0);
            let saved = !now.is_episode && app.is_saved(&now.uri).unwrap_or(false);
            let mut toggle_save = false;
            egui::Sides::new().shrink_left().spacing(8.0).show(
                ui,
                |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&now.title)
                                    .font(theme::bold(20.0))
                                    .color(palette.text),
                            )
                            .truncate(),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&now.subtitle)
                                    .font(theme::regular(14.0))
                                    .color(palette.secondary),
                            )
                            .truncate(),
                        );
                    });
                },
                |ui| {
                    if now.is_episode {
                        return;
                    }
                    let tooltip = if saved {
                        gettext(locale, "Remove from Liked Songs")
                    } else {
                        gettext(locale, "Save to Liked Songs")
                    };
                    if saved {
                        let (rect, response) =
                            ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                        ui.painter()
                            .circle_filled(rect.center(), 12.0, palette.accent);
                        theme::paint_icon(
                            ui,
                            Icon::Check,
                            Rect::from_center_size(rect.center(), vec2(14.0, 14.0)),
                            14.0,
                            palette.on_accent,
                        );
                        toggle_save = response.on_hover_text(tooltip.as_ref()).clicked();
                    } else {
                        toggle_save = theme::icon_button(
                            ui,
                            Icon::Heart,
                            20.0,
                            palette.secondary,
                            palette.text,
                            &tooltip,
                        )
                        .clicked();
                    }
                },
            );
            if toggle_save {
                app.actions.push(Action::ToggleSaved(now.uri.clone()));
            }
            if !now.album_name.is_empty() {
                ui.add_space(4.0);
                theme::text(
                    ui,
                    now.album_name.clone(),
                    theme::regular(12.0),
                    palette.secondary,
                );
            }
            ui.add_space(18.0);
            let Some(queue) = app.queue.get() else {
                return;
            };
            theme::text(
                ui,
                gettext(locale, "Next up"),
                theme::semibold(12.0),
                palette.secondary,
            );
            ui.add_space(6.0);
            for item in queue
                .queue
                .iter()
                .filter(|item| item.uri() != now.uri.as_str())
                .take(8)
            {
                theme::text(ui, item.name(), theme::regular(13.0), palette.text);
                let subtitle = item.subtitle();
                if !subtitle.is_empty() {
                    theme::text(ui, subtitle, theme::regular(11.5), palette.secondary);
                }
                ui.add_space(7.0);
            }
        });
}

/// The central lyrics page of the split view: the top bar, the Kawarp
/// background, and the words, with no cover taking half the page.
pub fn split_page(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::default()
        .frame(Frame::new().fill(Color32::BLACK))
        .show(ui, |ui| {
            theme::apply_local(ui, &theme::Palette::dark());
            let rect = ui.max_rect();
            background(app, ui, rect);
            topbar::show(app, ui);
            let body = Rect::from_min_max(
                pos2(rect.left(), ui.cursor().top()),
                pos2(rect.right(), rect.bottom()),
            );
            if body.height() < 80.0 {
                return;
            }
            let side = (body.width() * 0.07).clamp(20.0, 80.0);
            let look = stage::Look {
                font: lyric_size(app, body.width()),
                pad_left: side,
                pad_right: side,
                anchor: stage::Anchor::Center {
                    lift: 24.0 * lyric_size(app, body.width()) / 56.0,
                    rest_top: 0.3,
                },
                mask: (16.0, 64.0),
                minimal: false,
            };
            lyrics_view(app, ui, body, "split", look);
            view_controls(app, ui, body);
        });
}

/// Spicy Lyrics' full screen page: the cover and song on the left half,
/// the lyrics beside it; below 1133 points wide, the compact header bar
/// with the lyrics under it.
fn fullscreen_page(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    let top = theme::titlebar_inset(ui.ctx());
    let page = Rect::from_min_max(pos2(rect.left(), rect.top() + top), rect.max);
    let (w, h) = (page.width(), page.height());
    let queue = if app.show_queue_panel {
        app.settings.queue_width.clamp(
            super::queue::FULLSCREEN_COLUMN_MIN,
            super::queue::FULLSCREEN_COLUMN_MAX,
        ) + 32.0
    } else {
        0.0
    };
    let page = Rect::from_min_max(page.min, pos2(page.right() - queue, page.bottom()));
    if app.show_queue_panel {
        let column = Rect::from_min_max(
            pos2(page.right() + 16.0, page.top() + 56.0),
            pos2(rect.right() - 16.0, rect.bottom() - 16.0),
        );
        let mut queue_ui = ui.new_child(UiBuilder::new().max_rect(column));
        super::queue::fullscreen_column(app, &mut queue_ui);
    }
    let w = w - queue;
    let words = matches!(&app.lyrics, Loadable::Loaded(Some(lyrics)) if !lyrics.instrumental && !lyrics.lines.is_empty());
    let compact = w < 1133.0;
    let font = lyric_size(app, w * if compact { 1.0 } else { 0.5 });
    if compact {
        // The compact bar: 15% of the height, the cover as tall, its text
        // 13% of the bar from it, at the lyrics' 5% inset.
        let inset = w * 0.05;
        let bar = (h * 0.15).max(72.0);
        let bar_top = page.top() + inset.max(16.0);
        let art = Rect::from_min_size(pos2(page.left() + inset, bar_top), vec2(bar, bar));
        cover(app, ui, art, 0.03);
        let scale = font / (w * 0.07).clamp(48.0, 64.0);
        let text = Rect::from_min_max(
            pos2(art.right() + bar * 0.13, art.top()),
            pos2(page.right() - inset - 90.0, art.bottom()),
        );
        song_text(app, ui, text, 40.0 * scale, 24.0 * scale, Align::Min, true);
        let lyrics = Rect::from_min_max(pos2(page.left(), art.bottom()), page.max);
        let look = stage::Look {
            font,
            pad_left: w * 0.05,
            pad_right: w * 0.05,
            anchor: stage::Anchor::Top(85.0 * scale),
            mask: (16.0, 64.0),
            minimal: true,
        };
        lyrics_view(app, ui, lyrics, "fullscreen", look);
        // After the lyrics, so their click area cannot take the buttons.
        view_controls(app, ui, page);
        return;
    }
    // --NowBarWidth: min(30cqw, 52cqh) * 1.1, a quarter of it between the
    // cover and the middle, and the edge spacing 70% of what is left.
    let bar = (w * 0.30).min(h * 0.52) * 1.1;
    let inner = bar / 4.0;
    let edge = (w * 0.5 - bar - inner) * 0.7;
    let left = if words {
        page.left() + edge
    } else {
        page.left() + (w - bar) / 2.0
    };
    let art_top = page.top() + h * 0.5 - bar * 0.6;
    let art = Rect::from_min_size(pos2(left, art_top), vec2(bar, bar));
    cover(app, ui, art, 0.02);
    // The song's text under it, as on desktop: 3% of the cover, the name
    // 2.25 times, the artists 1.5.
    let base = (bar * 0.03).clamp(8.0, 38.4);
    let text = Rect::from_min_size(
        pos2(art.left(), art.bottom() + bar * 0.05),
        vec2(bar, base * 2.25 * 1.2 + base * 1.5 * 1.4),
    );
    song_text(app, ui, text, base * 2.25, base * 1.5, Align::Center, false);
    if !words {
        let below = Rect::from_min_size(pos2(art.left(), text.bottom() + 12.0), vec2(bar, 120.0));
        let mut note = ui.new_child(
            UiBuilder::new()
                .max_rect(below)
                .layout(Layout::top_down(Align::Center)),
        );
        match shown_lyrics(app) {
            Ok(_) | Err(Missing::Nothing) => {}
            Err(why) => missing(app, &mut note, why),
        }
        view_controls(app, ui, page);
        return;
    }
    // The lyrics from (50cqw - inner * 0.3) * 0.9 to edge * 0.35 from the
    // far side, in a box with the renderer's own 5% inset.
    let text_start = (w * 0.5 - inner * 0.3) * 0.9;
    let text_end = w - edge * 0.35;
    let lyrics = Rect::from_min_max(
        pos2(page.left() + text_start - 24.0, page.top()),
        pos2(page.left() + text_end, page.bottom()),
    );
    let look = stage::Look {
        font,
        pad_left: 24.0,
        pad_right: 0.0,
        anchor: stage::Anchor::Center {
            lift: 30.0 * font / 56.0,
            rest_top: 0.25,
        },
        mask: (16.0, 64.0),
        minimal: true,
    };
    lyrics_view(app, ui, lyrics, "fullscreen", look);
    // After the lyrics, so their click area cannot take the buttons.
    view_controls(app, ui, page);
}
/// The settings and leave buttons at the page's top right.
fn view_controls(app: &mut App, ui: &mut egui::Ui, page: Rect) {
    let locale = app.locale;
    let strip = Rect::from_min_max(
        pos2(page.right() - 220.0, page.top() + 12.0),
        pos2(page.right() - 20.0, page.top() + 52.0),
    );
    let mut controls = ui.new_child(
        UiBuilder::new()
            .max_rect(strip)
            .layout(Layout::right_to_left(Align::Center)),
    );
    controls.spacing_mut().item_spacing.x = 8.0;
    if view_control(
        &mut controls,
        Icon::Shrink,
        38.0,
        &gettext(locale, "Leave full screen (Esc)"),
    )
    .clicked()
    {
        app.actions.push(Action::SetLyricsFullscreen(false));
    }
    let settings = view_control(
        &mut controls,
        Icon::Settings,
        38.0,
        &gettext(locale, "Lyrics settings"),
    );
    lyrics_settings_popup(app, &settings);
}

/// The cover, rounded by `corner` of its side, with Spicy's soft shadow.
fn cover(app: &mut App, ui: &mut egui::Ui, rect: Rect, corner: f32) {
    let Some(now) = app.now_playing() else { return };
    let radius = rect.width() * corner;
    ui.painter().add(
        egui::epaint::Shadow {
            offset: [0, 9],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(69),
        }
        .as_shape(rect, radius),
    );
    widgets::playing_cover(
        ui,
        &theme::Palette::dark(),
        app.backend.art(),
        "fullscreen-lyrics",
        now.art_url.as_deref().or(now.art_small.as_deref()),
        rect,
        radius,
    );
    let art = now.art_url.is_some() || now.art_small.is_some();
    cover_opens_immersive(app, ui, rect, art);
}

/// The song's name (weight 900 on desktop; the heaviest here) at 0.95 and
/// the artists at 0.7.
fn song_text(
    app: &App,
    ui: &mut egui::Ui,
    rect: Rect,
    title: f32,
    artists: f32,
    align: Align,
    centred: bool,
) {
    let Some(now) = app.now_playing() else { return };
    let layout = if centred {
        Layout::top_down(align)
            .with_main_align(Align::Center)
            .with_main_justify(true)
    } else {
        Layout::top_down(align)
    };
    let mut text = ui.new_child(UiBuilder::new().max_rect(rect).layout(layout));
    text.spacing_mut().item_spacing.y = 2.0;
    let inner = |ui: &mut egui::Ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(&now.title)
                    .font(theme::lyrics_face(700, title))
                    .color(Color32::from_white_alpha(242)),
            )
            .truncate(),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(&now.subtitle)
                    .font(theme::regular(artists))
                    .color(Color32::from_white_alpha(179)),
            )
            .truncate(),
        );
    };
    if centred {
        text.vertical(inner);
    } else {
        inner(&mut text);
    }
}

fn background(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    #[cfg(feature = "milkdrop")]
    if milkdrop_backdrop(app, ui, rect) {
        return;
    }
    // filter: saturate(2.5) brightness(0.65), over the whole page.
    backdrop::paint(app, ui, rect, (2.5, 0.65));
}

/// The credits required beside the words, at the end of the scroll.
#[allow(clippy::too_many_arguments)]
fn attribution(
    ui: &mut egui::Ui,
    lyrics: &Lyrics,
    palette: &theme::Palette,
    locale: crate::i18n::Locale,
    position_ms: u32,
    refined: bool,
    follow_last_line: bool,
) {
    let Some(attribution) = &lyrics.attribution else {
        return;
    };
    // A song that ends on a duet line sung from the right has its credits
    // there too, stepped in by the same amount as the line.
    let indent = if follow_last_line
        && lyrics
            .lines
            .last()
            .is_some_and(|line| line.opposite_aligned && !crate::bidi::is_rtl(&line.text))
    {
        (ui.available_width() * 0.07).min(56.0)
    } else {
        0.0
    };
    if refined {
        // Quiet type close under the last line, dim until the song has been
        // sung through.
        let last_end = lyrics
            .lines
            .iter()
            .filter_map(|line| line.end_ms.or(line.at_ms))
            .max()
            .unwrap_or(0);
        let fade = if position_ms >= last_end { 1.0 } else { 0.5 };
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(indent);
            ui.vertical(|ui| {
                attribution_text(ui, attribution, lyrics, palette, locale, fade, false);
            });
        });
        ui.add_space(14.0);
        return;
    }
    ui.add_space(24.0);
    let (fill, stroke) = if palette.dark {
        (Color32::from_white_alpha(8), Color32::from_white_alpha(18))
    } else {
        (Color32::from_black_alpha(6), Color32::from_black_alpha(14))
    };
    Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(12)
        .inner_margin(Margin {
            left: 14 + indent as i8,
            right: 14,
            top: 12,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            attribution_text(ui, attribution, lyrics, palette, locale, 1.0, true);
        });
    ui.add_space(14.0);
}

/// The provider, the people and the writers, shared by the card and the
/// refined row. `fade` dims the whole block while the song is still playing.
fn attribution_text(
    ui: &mut egui::Ui,
    attribution: &crate::lyrics::Attribution,
    lyrics: &Lyrics,
    palette: &theme::Palette,
    locale: crate::i18n::Locale,
    fade: f32,
    card: bool,
) {
    theme::text(
        ui,
        crate::lyrics::attribution_title(attribution, locale),
        theme::semibold(if card { 12.0 } else { 11.0 }),
        palette.text.gamma_multiply(0.9 * fade),
    );
    if attribution.source.is_empty() {
        ui.add_space(3.0);
        theme::text(
            ui,
            gettext(locale, "Source unknown"),
            theme::regular(11.0),
            palette.dim.gamma_multiply(fade),
        );
    }
    for (verb, credit) in [
        (
            gettext(locale, "Uploaded by"),
            attribution.uploader.as_ref(),
        ),
        (gettext(locale, "Made by"), attribution.maker.as_ref()),
    ] {
        let Some(credit) = credit else {
            continue;
        };
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            theme::text(
                ui,
                verb,
                theme::regular(11.0),
                palette.dim.gamma_multiply(fade),
            );
            let label = egui::RichText::new(&credit.name)
                .font(theme::semibold(11.0))
                .color(palette.accent.gamma_multiply(fade));
            let response = ui.add(egui::Label::new(label).sense(Sense::click()));
            if response.hovered() {
                ui.ctx().set_cursor_icon(if credit.url.is_some() {
                    egui::CursorIcon::PointingHand
                } else {
                    egui::CursorIcon::Default
                });
            }
            if response.clicked()
                && let Some(url) = &credit.url
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        });
    }
    if !lyrics.song_writers.is_empty() {
        ui.add_space(5.0);
        theme::text(
            ui,
            gettext(locale, "Written by {names}")
                .replace("{names}", &lyrics.song_writers.join(", ")),
            theme::regular(11.0),
            palette.dim.gamma_multiply(fade),
        );
    }
}

/// The romanisation, joined by its own word markers when the source gave
/// them, since it can split words where the original text does not.
fn joined_transliteration(syllables: &[crate::lyrics::Syllable]) -> Option<String> {
    joined_field_owned(
        syllables,
        |syllable| syllable.transliterated.as_deref(),
        |syllable| {
            syllable
                .transliterated_part_of_word
                .unwrap_or(syllable.part_of_word)
        },
    )
}

fn joined_field_owned(
    syllables: &[crate::lyrics::Syllable],
    field: impl Fn(&crate::lyrics::Syllable) -> Option<&str>,
    joins_next: impl Fn(&crate::lyrics::Syllable) -> bool,
) -> Option<String> {
    let mut text = String::new();
    let mut previous_joined = None;
    for syllable in syllables {
        let Some(part) = field(syllable).filter(|part| !part.trim().is_empty()) else {
            continue;
        };
        if previous_joined == Some(false) {
            text.push(' ');
        }
        text.push_str(part.trim());
        previous_joined = Some(joins_next(syllable));
    }
    (!text.is_empty()).then_some(text)
}

/// How bright the brightest of the picture may stay behind the words, so a
/// white preset reads like the dimmed art does.
#[cfg(feature = "milkdrop")]
const BACKDROP_GLOW: f32 = 0.2;
/// The dimming's limits: even a dark preset's sparks stay this far under the
/// white of the words, and a bright one is never lost altogether.
#[cfg(feature = "milkdrop")]
const BACKDROP_DIM: std::ops::RangeInclusive<f32> = 0.2..=0.32;

/// How much of the picture's brightness to keep, for one this bright.
#[cfg(feature = "milkdrop")]
fn backdrop_dim(glow: f32) -> f32 {
    (BACKDROP_GLOW / glow.max(0.01)).clamp(*BACKDROP_DIM.start(), *BACKDROP_DIM.end())
}

/// MilkDrop filling `rect`, dimmed so the words stay clear whatever it draws.
/// `false` leaves the art: there is no picture yet, or none to be had.
#[cfg(feature = "milkdrop")]
fn milkdrop_backdrop(app: &mut App, ui: &mut egui::Ui, rect: Rect) -> bool {
    use crate::milkdrop::pane::{Picture, UV};
    if !app.milkdrop_behind_lyrics() {
        return false;
    }
    let ctx = ui.ctx().clone();
    let pixels = rect.size() * ctx.pixels_per_point();
    let picture = app.milkdrop_picture(&ctx, [pixels.x, pixels.y]);
    let Picture::Frame(texture) = picture else {
        if picture == Picture::Waiting {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            // No MilkDrop to be had: as if the art was never clicked.
            app.actions.push(Action::LeaveImmersive);
        }
        return false;
    };
    app.immersive_arrived();
    let dim = ctx.animate_value_with_time(
        egui::Id::new("lyrics-backdrop-dim"),
        backdrop_dim(app.milkdrop_glow()),
        crate::motion::seconds(&ctx, 0.5),
    );
    let tint = Color32::from_gray((dim * 255.0).round() as u8);
    ui.painter()
        .with_clip_rect(rect)
        .image(texture, rect, UV, tint);
    if app.now_playing().is_some_and(|now| now.playing) {
        theme::visualiser_frame(&ctx, Some(app.milkdrop_pane_fps()));
    }
    true
}

/// MilkDrop alone, edge to edge and at full brightness, under the root of the
/// window so that the player bar comes and goes over it without resizing it.
/// Returns whether the controls show.
#[cfg(feature = "milkdrop")]
pub fn immersive(app: &mut App, ui: &mut egui::Ui) -> bool {
    use crate::milkdrop::pane::{Picture, UV};
    let ctx = ui.ctx().clone();
    let rect = ctx.viewport_rect();
    ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
    let pixels = rect.size() * ctx.pixels_per_point();
    match app.milkdrop_picture(&ctx, [pixels.x, pixels.y]) {
        Picture::Frame(texture) => {
            ui.painter().image(texture, rect, UV, Color32::WHITE);
        }
        Picture::Waiting => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        Picture::None => app.actions.push(Action::LeaveImmersive),
    }
    if app.now_playing().is_some_and(|now| now.playing) {
        theme::visualiser_frame(&ctx, Some(app.milkdrop_pane_fps()));
    }
    let moved = ctx.input(|input| input.pointer.delta() != egui::Vec2::ZERO);
    let now = ctx.input(|input| input.time);
    let left = app.immersive_controls(moved, now);
    match left {
        Some(left) => ctx.request_repaint_after(std::time::Duration::from_secs_f64(left)),
        None => ctx.set_cursor_icon(egui::CursorIcon::None),
    }
    left.is_some()
}

/// Everything in immersive mode that is not a control: any click there brings
/// the view back.
#[cfg(feature = "milkdrop")]
pub fn immersive_click(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::default()
        .frame(Frame::new())
        .show(ui, |ui| {
            let rect = ui.max_rect();
            if ui
                .interact(rect, ui.id().with("immersive"), Sense::click())
                .clicked()
            {
                app.actions.push(Action::LeaveImmersive);
            }
        });
}

/// The cover is the way into immersive mode, if there is MilkDrop to go to.
fn cover_opens_immersive(app: &mut App, ui: &egui::Ui, cover: Rect, art: bool) {
    #[cfg(feature = "milkdrop")]
    if art
        && ui
            .interact(cover, ui.id().with("lyrics-cover"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    {
        app.actions.push(Action::EnterImmersive);
    }
    #[cfg(not(feature = "milkdrop"))]
    let _ = (app, ui, cover, art);
}
