//! Small presentational components. Nothing here owns state: every function
//! takes what it should draw and returns what the user did.

use eframe::egui;
use egui::{Color32, RichText, Rounding, Stroke};
use sweepmac::{human, SelectionSummary};

use super::style::{self, Tokens};

/// The window header: mark, name, safety status, last-scan age, Rescan.
pub struct HeaderResult {
    pub rescan_clicked: bool,
}

pub fn header(
    ui: &mut egui::Ui,
    t: &Tokens,
    mark: Option<&egui::TextureHandle>,
    last_scan: Option<&str>,
    busy: bool,
) -> HeaderResult {
    let mut rescan_clicked = false;
    ui.horizontal(|ui| {
        if let Some(tex) = mark {
            let size = 20.0;
            ui.add(
                egui::Image::new(tex)
                    .fit_to_exact_size(egui::vec2(size, size))
                    .tint(t.text),
            );
            ui.add_space(2.0);
        }
        ui.label(
            RichText::new("sweepmac")
                .size(style::T_TITLE)
                .strong()
                .color(t.text),
        );
        ui.add_space(4.0);
        status_pill(ui, t, "Safe cleanup", t.safe);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let btn = style::quiet_button(t, "Rescan");
            let resp = ui.add_enabled(!busy, btn);
            if resp.clicked() {
                rescan_clicked = true;
            }
            if busy {
                resp.on_hover_text("A scan or cleanup is already running");
            }
            if let Some(age) = last_scan {
                ui.add_space(2.0);
                ui.label(style::meta(t, format!("Scanned {age}")));
            }
        });
    });
    HeaderResult { rescan_clicked }
}

/// A small filled status chip — semantics stay in the text, colour only backs
/// it up (never the other way round).
pub fn status_pill(ui: &mut egui::Ui, t: &Tokens, text: &str, tint: Color32) {
    egui::Frame::default()
        .fill(tint.gamma_multiply(if t.dark { 0.18 } else { 0.14 }))
        .rounding(Rounding::same(999.0))
        .inner_margin(egui::Margin::symmetric(8.0, 2.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(style::T_META).strong().color(tint));
        });
}

/// The storage summary: the reclaimable anchor, disk context, and the meter.
pub fn storage_summary(
    ui: &mut egui::Ui,
    t: &Tokens,
    safe_reclaimable: u64,
    selected: u64,
    disk: (u64, u64),
    scanning: bool,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(human(safe_reclaimable))
                .size(style::T_HERO)
                .strong()
                .color(t.safe),
        );
        ui.add_space(2.0);
        ui.label(
            RichText::new("safely reclaimable")
                .size(style::T_TITLE)
                .color(t.text_secondary),
        );
        if scanning {
            ui.add_space(4.0);
            ui.add(egui::Spinner::new().size(13.0).color(t.text_muted));
        }
    });

    let (free, total) = disk;
    if total > 0 {
        ui.add_space(10.0);
        disk_meter(ui, t, free, total, selected);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} free of {}", human(free), human(total)))
                    .size(style::T_META)
                    .color(t.text_secondary),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if selected > 0 {
                    ui.label(
                        RichText::new(format!("+{} selected", human(selected)))
                            .size(style::T_META)
                            .strong()
                            .color(t.safe),
                    );
                }
            });
        });
    }
}

/// Neutral, informational capacity meter.
///
/// The track is used storage (neutral), a blue segment is free space, and a
/// green segment shows what the current selection would add to it. There is no
/// alarm state: the app has no deliberate critical-disk threshold, so inventing
/// a red bar here would be telling the user something it doesn't know.
pub fn disk_meter(ui: &mut egui::Ui, t: &Tokens, free: u64, total: u64, selected: u64) {
    let h = 8.0;
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::hover());
    let p = ui.painter();
    let r = Rounding::same(h / 2.0);

    // Track = capacity (the used portion reads as the unfilled remainder).
    let track = if t.dark {
        t.raised
    } else {
        t.raised.gamma_multiply(1.0)
    };
    p.rect_filled(rect, r, track);

    let frac = |b: u64| (b as f32 / total as f32).clamp(0.0, 1.0);
    // Free space grows from the right edge; the selection extends it leftward.
    let free_w = width * frac(free);
    let sel_w = width * frac(selected);

    if sel_w > 0.5 {
        let x0 = (rect.right() - free_w - sel_w).max(rect.left());
        let seg = egui::Rect::from_min_max(
            egui::pos2(x0, rect.top()),
            egui::pos2((rect.right() - free_w).max(x0), rect.bottom()),
        );
        p.rect_filled(seg, r, t.safe);
    }
    if free_w > 0.5 {
        let seg = egui::Rect::from_min_max(egui::pos2(rect.right() - free_w, rect.top()), rect.max);
        p.rect_filled(seg, r, t.accent);
    }

    let tip = if selected > 0 {
        format!(
            "{} free of {} · cleaning the selection would add {}",
            human(free),
            human(total),
            human(selected)
        )
    } else {
        format!("{} free of {}", human(free), human(total))
    };
    resp.on_hover_text(tip);
}

/// What a collapsible section group shows in its header.
pub struct GroupSpec<'a> {
    /// Stable id for the open/closed state.
    pub id: &'a str,
    pub title: &'a str,
    pub count: usize,
    pub bytes: u64,
    pub default_open: bool,
    /// Tints the title and the card edge — used by the caution and danger zones.
    pub tint: Option<Color32>,
}

/// A collapsible section group: title, item count and total size in the header,
/// `body` inside.
pub fn collapsing_group(
    ui: &mut egui::Ui,
    t: &Tokens,
    spec: GroupSpec<'_>,
    body: impl FnOnce(&mut egui::Ui),
) {
    let GroupSpec {
        id,
        title,
        count,
        bytes,
        default_open,
        tint,
    } = spec;
    let id = ui.make_persistent_id(id);
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    let frame_tint = tint;
    let draw = |ui: &mut egui::Ui| {
        let state = state;
        state
            .show_header(ui, |ui| {
                let color = frame_tint.unwrap_or(t.text);
                ui.label(
                    RichText::new(title)
                        .size(style::T_BODY)
                        .strong()
                        .color(color),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let summary = if count == 0 {
                        "nothing found".to_string()
                    } else if bytes > 0 {
                        format!("{count} item{} · {}", plural(count), human(bytes))
                    } else {
                        format!("{count} item{}", plural(count))
                    };
                    ui.label(style::meta(t, summary));
                });
            })
            .body(body);
    };
    match frame_tint {
        Some(c) => style::tinted_group(ui, t, c, draw),
        None => style::group(ui, t, draw),
    }
}

pub fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// One selectable line item.
pub struct RowView<'a> {
    pub name: &'a str,
    /// A short reassurance, only where it adds confidence.
    pub note: Option<&'a str>,
    pub size: u64,
    pub checked: bool,
    /// False when the item can't be acted on right now (e.g. work in flight).
    pub enabled: bool,
    /// True when the item ITSELF is locked (in use / mounted) — drives the
    /// lock pill, independently of transient `enabled` gating.
    pub locked: bool,
    /// Why it is locked ("in use by a container").
    pub lock_note: Option<&'a str>,
    /// Technical detail shown on hover (path, image id…).
    pub details: Option<&'a str>,
    /// Accent used for the selection control.
    pub tint: Color32,
}

/// The selection control: a box that fills solid with the row's risk tint and
/// draws a bold checkmark when ticked, so checked state is legible at a glance
/// (egui's stock checkbox only changes a thin checkmark). Returns true when
/// toggled; participates in keyboard focus (tab + space/enter).
pub fn risk_checkbox(
    ui: &mut egui::Ui,
    t: &Tokens,
    checked: bool,
    enabled: bool,
    tint: Color32,
) -> egui::Response {
    let side = 18.0;
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), sense);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, checked, "")
    });

    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let rounding = Rounding::same(5.0);
        let rect = rect.shrink(1.0);
        if checked {
            let fill = if enabled {
                tint
            } else {
                tint.gamma_multiply(0.45)
            };
            p.rect_filled(rect, rounding, fill);
            // Bold checkmark in the on-fill colour.
            let c = rect.center();
            let mark = Stroke::new(2.2_f32, t.on_accent);
            p.line_segment(
                [
                    egui::pos2(c.x - 4.5, c.y + 0.5),
                    egui::pos2(c.x - 1.0, c.y + 4.0),
                ],
                mark,
            );
            p.line_segment(
                [
                    egui::pos2(c.x - 1.0, c.y + 4.0),
                    egui::pos2(c.x + 4.8, c.y - 3.8),
                ],
                mark,
            );
        } else {
            let edge = if enabled { t.text_muted } else { t.divider };
            p.rect_stroke(rect, rounding, Stroke::new(1.5_f32, edge));
        }
        if response.hovered() || response.has_focus() {
            p.rect_stroke(
                rect.expand(2.0),
                Rounding::same(7.0),
                Stroke::new(1.5_f32, t.accent),
            );
        }
    }
    response
}

/// Draw a row with a selection control, name, size and optional detail. Returns
/// true when the selection was toggled.
///
/// Layout note: the right-aligned size (and lock pill) are laid out FIRST in a
/// right-to-left pass, and the name truncates into whatever width remains — so
/// a long volume hash or node_modules path can never paint underneath them.
pub fn select_row(ui: &mut egui::Ui, t: &Tokens, v: RowView<'_>) -> bool {
    let mut toggled = false;
    ui.horizontal(|ui| {
        let mut cb = risk_checkbox(ui, t, v.checked, v.enabled, v.tint);
        if cb.clicked() {
            toggled = true;
        }
        if let Some(why) = v.lock_note {
            if v.locked {
                cb = cb.on_hover_text(why);
            }
        }
        let _ = cb;

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let size_color = if v.size > 0 { t.text } else { t.text_muted };
            ui.label(
                RichText::new(if v.size > 0 {
                    human(v.size)
                } else {
                    "empty".to_string()
                })
                .size(style::T_LABEL)
                .color(size_color),
            );
            if let Some(lock) = v.lock_note {
                if v.locked {
                    ui.add_space(4.0);
                    status_pill(ui, t, lock, t.caution);
                }
            }
            // The name takes the remaining width and truncates within it.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let name_color = if v.enabled { t.text } else { t.text_muted };
                let name = ui.add(
                    egui::Label::new(RichText::new(v.name).size(style::T_BODY).color(name_color))
                        .truncate()
                        .selectable(false),
                );
                if let Some(d) = v.details {
                    name.on_hover_text(RichText::new(d).size(style::T_META).monospace());
                }
            });
        });
    });
    if let Some(note) = v.note {
        ui.horizontal(|ui| {
            ui.add_space(26.0); // align under the name, past the checkbox
            ui.label(style::meta(t, note));
        });
    }
    toggled
}

/// The persistent contextual action bar. Returns true when the primary action
/// is clicked.
pub fn action_bar(
    ui: &mut egui::Ui,
    t: &Tokens,
    summary: SelectionSummary,
    enabled: bool,
    busy_label: Option<&str>,
) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        if let Some(busy) = busy_label {
            ui.add(egui::Spinner::new().size(14.0).color(t.accent));
            ui.add_space(4.0);
            ui.label(
                RichText::new(busy)
                    .size(style::T_BODY)
                    .color(t.text_secondary),
            );
        } else if summary.is_empty() {
            // No selection: explain, don't threaten. Never a red zero-count CTA.
            ui.label(
                RichText::new("Select items to clean")
                    .size(style::T_BODY)
                    .color(t.text_secondary),
            );
        } else {
            ui.label(
                RichText::new(format!(
                    "{} item{} selected",
                    summary.count,
                    plural(summary.count)
                ))
                .size(style::T_BODY)
                .color(t.text),
            );
            ui.label(RichText::new("·").size(style::T_BODY).color(t.text_muted));
            ui.label(
                RichText::new(human(summary.bytes))
                    .size(style::T_BODY)
                    .strong()
                    .color(t.safe),
            );
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let resp = ui.add_enabled(enabled, style::primary_button(t, "Review & Clean"));
            if resp.clicked() {
                clicked = true;
            }
            if !enabled {
                resp.on_disabled_hover_text(if busy_label.is_some() {
                    "Waiting for the current operation to finish"
                } else {
                    "Tick at least one cache to enable cleaning"
                });
            }
        });
    });
    clicked
}

/// One entry in Recent activity.
pub struct Activity {
    pub ok: bool,
    pub summary: String,
    pub detail: Vec<String>,
}

/// Recent activity, collapsed until there is something to show.
pub fn activity_section(
    ui: &mut egui::Ui,
    t: &Tokens,
    entries: &[Activity],
    open: bool,
    running: Option<(&str, u64)>,
    live: &[String],
) {
    let id = ui.make_persistent_id("recent_activity");
    let mut state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    if open && !state.is_open() {
        state.set_open(true);
        state.store(ui.ctx());
    }
    style::group(ui, t, |ui| {
        state
            .show_header(ui, |ui| {
                ui.label(
                    RichText::new("Recent activity")
                        .size(style::T_BODY)
                        .strong()
                        .color(t.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some((label, secs)) = running {
                        ui.label(style::meta(t, format!("{label} · {secs}s")));
                    } else if let Some(last) = entries.last() {
                        let tint = if last.ok { t.safe } else { t.danger };
                        ui.label(RichText::new(&last.summary).size(style::T_META).color(tint));
                    } else {
                        ui.label(style::meta(t, "nothing yet"));
                    }
                });
            })
            .body(|ui| {
                if running.is_some() {
                    if live.is_empty() {
                        ui.label(style::meta(t, "starting…"));
                    } else {
                        ui.label(style::meta(t, "In progress"));
                        egui::ScrollArea::vertical()
                            .id_salt("live_log")
                            .max_height(120.0)
                            .auto_shrink([false, true])
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                for line in live.iter().rev().take(8).rev() {
                                    ui.label(
                                        RichText::new(line)
                                            .size(style::T_META)
                                            .monospace()
                                            .color(t.text_secondary),
                                    );
                                }
                            });
                        ui.add_space(8.0);
                    }
                }
                if entries.is_empty() {
                    if running.is_none() {
                        ui.label(style::meta(t, "No cleanups run in this session."));
                    }
                    return;
                }
                egui::ScrollArea::vertical()
                    .max_height(150.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (n, e) in entries.iter().enumerate().rev() {
                            if n + 1 != entries.len() {
                                ui.add_space(4.0);
                            }
                            let tint = if e.ok { t.safe } else { t.danger };
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(if e.ok { "Done" } else { "Failed" })
                                        .size(style::T_META)
                                        .strong()
                                        .color(tint),
                                );
                                ui.label(
                                    RichText::new(&e.summary)
                                        .size(style::T_META)
                                        .color(t.text_secondary),
                                );
                            });
                            if !e.detail.is_empty() {
                                let did = ui.make_persistent_id(("activity_detail", n));
                                egui::collapsing_header::CollapsingState::load_with_default_open(
                                    ui.ctx(),
                                    did,
                                    false,
                                )
                                .show_header(ui, |ui| {
                                    ui.label(style::meta(t, "View details"));
                                })
                                .body(|ui| {
                                    for line in &e.detail {
                                        ui.label(
                                            RichText::new(line)
                                                .size(style::T_META)
                                                .monospace()
                                                .color(t.text_muted),
                                        );
                                    }
                                });
                            }
                        }
                    });
            });
    });
}

/// A modal backdrop that dims the window behind a dialog.
pub fn backdrop(ctx: &egui::Context) {
    egui::Area::new(egui::Id::new("sweepmac_backdrop"))
        .order(egui::Order::Background)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, Color32::from_black_alpha(140));
        });
}

/// A scrollable, monospaced list of the exact items an action will touch.
pub fn item_manifest(ui: &mut egui::Ui, t: &Tokens, lines: &[(String, Option<u64>)]) {
    egui::Frame::default()
        .fill(if t.dark { t.canvas } else { t.raised })
        .rounding(Rounding::same(style::RADIUS_SMALL))
        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
        .stroke(Stroke::new(1.0_f32, t.divider))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .max_height(150.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (label, size) in lines {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(label)
                                        .size(style::T_META)
                                        .color(t.text_secondary),
                                )
                                .truncate(),
                            );
                            if let Some(s) = size {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(human(*s))
                                                .size(style::T_META)
                                                .monospace()
                                                .color(t.text_muted),
                                        );
                                    },
                                );
                            }
                        });
                    }
                });
        });
}
