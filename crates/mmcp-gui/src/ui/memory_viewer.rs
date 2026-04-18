//! Right / central pane: frontmatter metadata + rendered markdown body.
//!
//! The frontmatter header is wrapped in a Frame-styled "card" so it
//! reads as a distinct metadata region rather than a floating label
//! run. Tags, kind, and the mandatory flag are rendered as pills via
//! the shared [`crate::ui::tag_pill`] helper so the visual vocabulary
//! stays consistent with the memory list's kind badges.
//!
//! A "Show raw / full metadata" toggle below the pill row exposes
//! every non-form frontmatter field plus the canonical rendered
//! TOML — the truth the mmcp write path would commit — so the GUI
//! is never hiding shape from the user.
//!
//! Owns a `CommonMarkCache` so the renderer doesn't re-parse the
//! markdown tree on every frame. The cache is long-lived (stored in
//! [`ViewerWidget`]), which matches `egui_commonmark`'s expected
//! lifecycle.

use eframe::egui;
use egui_commonmark::CommonMarkCache;
use egui_phosphor::regular as icons;
use mmcp_core::memory::{FrontmatterFormat, MemoryFile};

use crate::state::AppState;
use crate::ui::{markdown_render, tag_pill};

#[derive(Default)]
pub struct ViewerWidget {
    commonmark: CommonMarkCache,
    /// Per-widget toggle for the "full metadata" disclosure. Global
    /// rather than per-memory by design: users either trust the
    /// summarised card or they want to see the whole thing; a
    /// per-memory toggle would be noise.
    show_raw: bool,
}

impl ViewerWidget {
    pub fn show(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let Some(group) = state.selection.group else {
            Self::placeholder(ui, "Select a group to begin.");
            return;
        };
        let Some(slug) = state.selection.memory.as_deref() else {
            Self::placeholder(ui, "Select a memory to view its body.");
            return;
        };
        let Some(memory) = state.viewer.get(&group, slug) else {
            ui.spinner();
            ui.label("Loading memory body…");
            return;
        };

        egui::ScrollArea::vertical()
            .id_salt("mmcp_gui_viewer_outer")
            .show(ui, |ui| {
                Self::render_frontmatter_card(ui, &memory, slug, &mut self.show_raw);
                ui.add_space(12.0);
                markdown_render::render(ui, &mut self.commonmark, &memory.body);
            });

        if let Some(err) = &state.last_error {
            ui.separator();
            ui.colored_label(egui::Color32::from_rgb(220, 120, 120), err);
        }
    }

    fn placeholder(ui: &mut egui::Ui, msg: &str) {
        ui.vertical_centered(|ui| {
            ui.add_space(32.0);
            ui.label(msg);
        });
    }

    fn render_frontmatter_card(
        ui: &mut egui::Ui,
        memory: &MemoryFile,
        slug: &str,
        show_raw: &mut bool,
    ) {
        let fm = &memory.frontmatter;
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.heading(&fm.name);
                ui.label(
                    egui::RichText::new(slug)
                        .small()
                        .color(ui.visuals().widgets.inactive.fg_stroke.color),
                );
                ui.add_space(4.0);
                ui.label(&fm.description);
                ui.add_space(8.0);

                // Pill row: kind + optional mandatory + version + tags.
                ui.horizontal_wrapped(|ui| {
                    tag_pill::kind(ui, fm.kind.as_str());
                    if fm.mandatory {
                        tag_pill::mandatory(ui);
                    }
                    if let Some(v) = &fm.version {
                        tag_pill::tag(ui, &format!("v{v}"));
                    }
                    for tag in &fm.tags {
                        tag_pill::tag(ui, tag);
                    }
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let label = if *show_raw {
                        format!("{}  Hide full metadata", icons::CARET_DOWN)
                    } else {
                        format!("{}  Show full metadata", icons::CARET_RIGHT)
                    };
                    if ui.small_button(label).clicked() {
                        *show_raw = !*show_raw;
                    }
                });

                if *show_raw {
                    ui.add_space(6.0);
                    render_full_metadata(ui, memory);
                }
            });
    }
}

fn render_full_metadata(ui: &mut egui::Ui, memory: &MemoryFile) {
    let fm = &memory.frontmatter;
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            egui::Grid::new("mmcp_gui_viewer_full_metadata")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .min_col_width(120.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("format").small().weak());
                    ui.monospace(format_label(memory.format));
                    ui.end_row();

                    ui.label(egui::RichText::new("version").small().weak());
                    match &fm.version {
                        Some(v) => {
                            ui.monospace(v.to_string());
                        }
                        None => {
                            ui.label(egui::RichText::new("(unset — server-managed)").italics());
                        }
                    }
                    ui.end_row();

                    ui.label(egui::RichText::new("bump_intent").small().weak());
                    match &fm.bump_intent {
                        Some(b) => {
                            ui.monospace(format!("{b:?}"));
                        }
                        None => {
                            ui.label(egui::RichText::new("(none)").italics());
                        }
                    }
                    ui.end_row();

                    ui.label(egui::RichText::new("feature").small().weak());
                    match &fm.feature {
                        Some(feat) => {
                            ui.vertical(|ui| {
                                ui.monospace(format!("status = {:?}", feat.status));
                                if !feat.depends_on.is_empty() {
                                    ui.monospace(format!("depends_on = {:?}", feat.depends_on));
                                }
                                if !feat.blocks.is_empty() {
                                    ui.monospace(format!("blocks = {:?}", feat.blocks));
                                }
                            });
                        }
                        None => {
                            ui.label(egui::RichText::new("(not an FR memory)").italics());
                        }
                    }
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.label(egui::RichText::new("rendered TOML").small().strong());
            ui.add_space(4.0);
            render_rendered_toml(ui, memory);
        });
}

fn render_rendered_toml(ui: &mut egui::Ui, memory: &MemoryFile) {
    let rendered = match memory.to_toml_string() {
        Ok(s) => s,
        Err(err) => format!("(failed to render: {err})"),
    };
    // Show only the frontmatter block, not the body — the body is
    // already visible below as rendered markdown.
    let trimmed = trim_to_frontmatter(&rendered);
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(trimmed).monospace().size(12.0))
                    .selectable(true)
                    .wrap(),
            );
        });
}

/// Keep everything through the closing frontmatter fence. The body
/// follows verbatim after; we strip it because the rendered preview
/// below already shows the body and repeating it in the raw block
/// would double the height of the disclosure.
fn trim_to_frontmatter(rendered: &str) -> String {
    let mut lines = rendered.lines();
    let mut out = String::new();
    let mut seen_open = false;
    for line in lines.by_ref() {
        let is_fence = line.trim() == "+++" || line.trim() == "---";
        out.push_str(line);
        out.push('\n');
        if is_fence {
            if !seen_open {
                seen_open = true;
            } else {
                break;
            }
        }
    }
    out
}

fn format_label(fmt: FrontmatterFormat) -> &'static str {
    match fmt {
        FrontmatterFormat::TomlPlus => "TOML (+++ fences)",
        FrontmatterFormat::Yaml => "YAML (--- fences)",
        FrontmatterFormat::Json => "JSON (--- fences)",
        FrontmatterFormat::TomlDash => "TOML (--- fences)",
    }
}
