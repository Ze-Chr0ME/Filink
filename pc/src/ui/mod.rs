//! egui front-end for the DroidTransfer PC app. All networking lives in `droidtransfer::core`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use droidtransfer::core::{Core, Direction, PeerView, Snapshot, TransferState, TransferView};
use eframe::egui::{self, Align, Color32, Layout, RichText, Rounding, Stroke, Vec2};

const ACCENT: Color32 = Color32::from_rgb(0x2e, 0x7d, 0x5b);
const ONLINE: Color32 = Color32::from_rgb(0x3c, 0xb3, 0x71);
const OFFLINE: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
const DANGER: Color32 = Color32::from_rgb(0xc6, 0x3c, 0x3c);

pub struct App {
    core: Core,
    selected: Option<String>,
    text_draft: String,
    name_draft: Option<String>,
    status: Option<(String, SystemTime)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, core: Core) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 6.0);
        cc.egui_ctx.set_style(style);
        Self {
            core,
            selected: None,
            text_draft: String::new(),
            name_draft: None,
            status: None,
        }
    }

    fn flash(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), SystemTime::now()));
    }

    /// The peer that actions apply to: the selected paired peer, or the only paired peer.
    fn target<'a>(&self, snap: &'a Snapshot) -> Option<&'a PeerView> {
        if let Some(id) = &self.selected {
            if let Some(p) = snap.peers.iter().find(|p| &p.id == id && p.paired) {
                return Some(p);
            }
        }
        let mut paired = snap.peers.iter().filter(|p| p.paired);
        let first = paired.next()?;
        if paired.next().is_none() {
            return Some(first);
        }
        // Several paired: prefer the only online one.
        let mut online = snap.peers.iter().filter(|p| p.paired && p.online);
        let o = online.next()?;
        online.next().is_none().then_some(o)
    }

    fn send_paths(&mut self, snap: &Snapshot, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        match self.target(snap) {
            Some(p) => {
                let n = paths.len();
                self.core.send_paths(&p.id, paths);
                self.flash(format!("Sending {n} item(s) to {}", p.name));
            }
            None => self.flash("Select a paired phone first"),
        }
    }

    fn send_text(&mut self, snap: &Snapshot, text: String) {
        if text.trim().is_empty() {
            return;
        }
        match self.target(snap) {
            Some(p) => {
                self.core.send_text(&p.id, text);
                self.flash(format!("Text sent to {}", p.name));
            }
            None => self.flash("Select a paired phone first"),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Online/offline status and pairing countdowns change with time alone.
        ctx.request_repaint_after(Duration::from_secs(1));
        let snap = self.core.snapshot();

        // Drag & drop from Explorer.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.send_paths(&snap, dropped);
        }
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());

        self.top_bar(ctx, &snap);
        self.devices_panel(ctx, &snap);
        self.activity_panel(ctx, &snap);
        egui::CentralPanel::default().show(ctx, |ui| self.main_area(ui, &snap, hovering));
        self.pair_dialogs(ctx, &snap);
    }
}

// ---------------------------------------------------------------- panels

impl App {
    fn top_bar(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("DroidTransfer")
                        .size(20.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.separator();
                match &mut self.name_draft {
                    Some(draft) => {
                        let r = ui.add(egui::TextEdit::singleline(draft).desired_width(180.0));
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if enter || ui.button("Save").clicked() {
                            let name = draft.trim().to_string();
                            if !name.is_empty() {
                                self.core.set_name(name);
                            }
                            self.name_draft = None;
                        } else if ui.button("Cancel").clicked() {
                            self.name_draft = None;
                        }
                    }
                    None => {
                        ui.label(RichText::new(&snap.my_name).strong());
                        if ui
                            .small_button("✏")
                            .on_hover_text("Rename this PC")
                            .clicked()
                        {
                            self.name_draft = Some(snap.my_name.clone());
                        }
                    }
                }
                let ips: Vec<String> = snap.local_ips.iter().map(|ip| ip.to_string()).collect();
                if !ips.is_empty() {
                    ui.label(RichText::new(ips.join("  ·  ")).weak());
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button("Change…")
                        .on_hover_text("Choose where received files go")
                        .clicked()
                    {
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_directory(&snap.download_dir)
                            .pick_folder()
                        {
                            self.core.set_download_dir(dir);
                        }
                    }
                    if ui.button("📂 Open").clicked() {
                        let _ = std::fs::create_dir_all(&snap.download_dir);
                        open_in_explorer(&snap.download_dir, false);
                    }
                    ui.label(
                        RichText::new(snap.download_dir.display().to_string())
                            .monospace()
                            .weak(),
                    );
                    ui.label("Save to:");
                });
            });
            if let Some(err) = &snap.server_error {
                ui.colored_label(DANGER, format!("⚠ {err}"));
            }
            ui.add_space(4.0);
        });
    }

    fn devices_panel(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        egui::SidePanel::left("devices").resizable(false).exact_width(250.0).show(ctx, |ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Phones").size(16.0).strong());
            ui.add_space(4.0);

            if snap.peers.is_empty() {
                ui.label(RichText::new("No phones yet.").weak());
                ui.label(
                    RichText::new(
                        "Open DroidTransfer on your phone (same Wi-Fi) and tap Pair next to this PC.",
                    )
                    .weak(),
                );
            }

            let target_id = self.target(snap).map(|p| p.id.clone());
            let mut unpair: Option<String> = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for p in &snap.peers {
                    let selected = target_id.as_deref() == Some(p.id.as_str());
                    let fill = if selected { ACCENT.linear_multiply(0.18) } else { ui.visuals().faint_bg_color };
                    let stroke = if selected { Stroke::new(1.5_f32, ACCENT) } else { Stroke::NONE };
                    let resp = egui::Frame::none()
                        .fill(fill)
                        .stroke(stroke)
                        .rounding(Rounding::same(8.0))
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                                ui.painter().circle_filled(rect.center(), 5.0, if p.online { ONLINE } else { OFFLINE });
                                ui.label(RichText::new(&p.name).strong());
                            });
                            let ip = p.ip.map(|ip| ip.to_string()).unwrap_or_else(|| "unknown IP".into());
                            let seen = if p.online {
                                "online".to_string()
                            } else {
                                match p.last_seen_secs {
                                    Some(s) => format!("seen {} ago", fmt_ago(s)),
                                    None => "offline".into(),
                                }
                            };
                            ui.label(RichText::new(format!("{ip} · {seen}")).small().weak());
                            if !p.paired {
                                ui.label(RichText::new("Not paired: tap Pair on the phone").small().color(OFFLINE));
                            }
                        })
                        .response
                        .interact(egui::Sense::click());
                    if p.paired {
                        if resp.clicked() {
                            self.selected = Some(p.id.clone());
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Unpair").clicked() {
                                unpair = Some(p.id.clone());
                                ui.close_menu();
                            }
                        });
                    }
                    ui.add_space(4.0);
                }
            });
            if let Some(id) = unpair {
                self.core.unpair(&id);
                if self.selected.as_deref() == Some(id.as_str()) {
                    self.selected = None;
                }
            }
        });
    }

    fn activity_panel(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        egui::SidePanel::right("activity")
            .resizable(true)
            .default_width(340.0)
            .min_width(260.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Transfers").size(16.0).strong());
                    let finished = snap
                        .transfers
                        .iter()
                        .any(|t| t.state != TransferState::Running);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if finished && ui.small_button("Clear").clicked() {
                            self.core.clear_finished();
                        }
                    });
                });
                let half = (ui.available_height() * 0.6).max(120.0);
                egui::ScrollArea::vertical()
                    .id_salt("transfers")
                    .max_height(half)
                    .show(ui, |ui| {
                        if snap.transfers.is_empty() {
                            ui.label(RichText::new("Nothing yet.").weak());
                        }
                        for t in &snap.transfers {
                            self.transfer_row(ui, t);
                        }
                    });

                ui.separator();
                ui.label(RichText::new("Text & links").size(16.0).strong());
                egui::ScrollArea::vertical()
                    .id_salt("texts")
                    .show(ui, |ui| {
                        if snap.texts.is_empty() {
                            ui.label(
                                RichText::new(
                                    "Texts from the phone land on your clipboard and show up here.",
                                )
                                .weak(),
                            );
                        }
                        for (i, t) in snap.texts.iter().enumerate() {
                            egui::Frame::none()
                                .fill(ui.visuals().faint_bg_color)
                                .rounding(Rounding::same(6.0))
                                .inner_margin(8.0)
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        let dir = if t.incoming {
                                            format!("from {}", t.peer_name)
                                        } else {
                                            format!("to {}", t.peer_name)
                                        };
                                        ui.label(RichText::new(dir).small().weak());
                                        ui.label(RichText::new(fmt_time_ago(t.at)).small().weak());
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if ui.small_button("Copy").clicked() {
                                                    ui.output_mut(|o| {
                                                        o.copied_text = t.text.clone()
                                                    });
                                                    self.flash("Copied");
                                                }
                                                if is_url(&t.text)
                                                    && ui.small_button("Open").clicked()
                                                {
                                                    ui.ctx().open_url(egui::OpenUrl::new_tab(
                                                        t.text.trim(),
                                                    ));
                                                }
                                            },
                                        );
                                    });
                                    let shown = truncate(&t.text, 400);
                                    ui.add(egui::Label::new(shown).wrap());
                                });
                            ui.push_id(i, |_| {});
                            ui.add_space(2.0);
                        }
                    });
            });
    }

    fn transfer_row(&mut self, ui: &mut egui::Ui, t: &TransferView) {
        egui::Frame::none()
            .fill(ui.visuals().faint_bg_color)
            .rounding(Rounding::same(6.0))
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (arrow, label) = match t.direction {
                        Direction::Send => ("⬆", format!("To {}", t.peer_name)),
                        Direction::Receive => ("⬇", format!("From {}", t.peer_name)),
                    };
                    ui.label(RichText::new(arrow).color(ACCENT).strong());
                    ui.label(RichText::new(label).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match &t.state {
                        TransferState::Running => {
                            if ui.small_button("Cancel").clicked() {
                                self.core.cancel_transfer(t.id);
                            }
                        }
                        TransferState::Done => {
                            if let Some(dest) = &t.dest {
                                if ui.small_button("Show").clicked() {
                                    open_in_explorer(dest, dest.is_file());
                                }
                            }
                            ui.label(RichText::new("✔ Done").color(ONLINE));
                        }
                        TransferState::Cancelled => {
                            ui.label(RichText::new("Cancelled").weak());
                        }
                        TransferState::Failed(_) => {
                            ui.label(RichText::new("Failed").color(DANGER));
                        }
                    });
                });

                if !t.current_file.is_empty() {
                    ui.label(
                        RichText::new(truncate(&t.current_file, 60))
                            .small()
                            .monospace(),
                    );
                }

                let frac = if t.bytes_total > 0 {
                    (t.bytes_done as f32 / t.bytes_total as f32).clamp(0.0, 1.0)
                } else if t.files_total > 0 {
                    t.files_done as f32 / t.files_total as f32
                } else {
                    0.0
                };
                if t.state == TransferState::Running {
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .fill(ACCENT)
                            .desired_height(8.0),
                    );
                }

                let mut info = format!(
                    "{}/{} files · {} of {}",
                    t.files_done,
                    t.files_total,
                    fmt_bytes(t.bytes_done),
                    fmt_bytes(t.bytes_total)
                );
                if t.state == TransferState::Running && t.speed_bps > 0.0 {
                    info.push_str(&format!(" · {}/s", fmt_bytes(t.speed_bps as u64)));
                    if t.bytes_total > t.bytes_done {
                        let eta = (t.bytes_total - t.bytes_done) as f64 / t.speed_bps;
                        info.push_str(&format!(" · {} left", fmt_ago(eta as u64)));
                    }
                }
                ui.label(RichText::new(info).small().weak());
                if let TransferState::Failed(msg) = &t.state {
                    ui.label(RichText::new(msg).small().color(DANGER));
                }
            });
        ui.add_space(2.0);
    }

    fn main_area(&mut self, ui: &mut egui::Ui, snap: &Snapshot, hovering: bool) {
        let target = self.target(snap).cloned();
        ui.add_space(8.0);

        match &target {
            Some(p) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Send to").size(16.0));
                    ui.label(RichText::new(&p.name).size(16.0).strong());
                    if !p.online {
                        ui.label(RichText::new("(offline — open the app on the phone)").weak());
                    }
                });
            }
            None => {
                let any_paired = snap.peers.iter().any(|p| p.paired);
                let msg = if any_paired {
                    "Select a phone on the left."
                } else {
                    "Pair a phone to start sending."
                };
                ui.label(RichText::new(msg).size(16.0));
            }
        }
        ui.add_space(6.0);

        // Drop zone.
        let enabled = target.is_some();
        let height = (ui.available_height() - 190.0).max(140.0);
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), height),
            egui::Sense::click(),
        );
        let active = hovering && enabled;
        let visuals = ui.visuals();
        let fill = if active {
            ACCENT.linear_multiply(0.22)
        } else {
            visuals.faint_bg_color
        };
        let stroke_color = if active {
            ACCENT
        } else {
            visuals.widgets.noninteractive.bg_stroke.color
        };
        ui.painter().rect(
            rect,
            Rounding::same(12.0),
            fill,
            Stroke::new(if active { 2.5_f32 } else { 1.5 }, stroke_color),
        );
        let text_color = if enabled {
            visuals.text_color()
        } else {
            visuals.weak_text_color()
        };
        let center = rect.center();
        ui.painter().text(
            center - Vec2::new(0.0, 16.0),
            egui::Align2::CENTER_CENTER,
            if active {
                "Release to send"
            } else {
                "Drop files or folders here"
            },
            egui::FontId::proportional(22.0),
            text_color,
        );
        ui.painter().text(
            center + Vec2::new(0.0, 16.0),
            egui::Align2::CENTER_CENTER,
            "or click to choose files",
            egui::FontId::proportional(14.0),
            visuals.weak_text_color(),
        );
        if enabled && resp.clicked() {
            if let Some(files) = rfd::FileDialog::new().pick_files() {
                self.send_paths(snap, files);
            }
        }
        let resp = resp.on_hover_cursor(if enabled {
            egui::CursorIcon::PointingHand
        } else {
            egui::CursorIcon::Default
        });
        let _ = resp;

        ui.add_space(6.0);
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                if ui.button("📄 Send files…").clicked() {
                    if let Some(files) = rfd::FileDialog::new().pick_files() {
                        self.send_paths(snap, files);
                    }
                }
                if ui.button("📁 Send folder…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.send_paths(snap, vec![dir]);
                    }
                }
                if ui
                    .button("📋 Send clipboard")
                    .on_hover_text("Send the text on your PC clipboard")
                    .clicked()
                {
                    match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                        Ok(text) if !text.trim().is_empty() => self.send_text(snap, text),
                        _ => self.flash("Clipboard has no text"),
                    }
                }
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let w = ui.available_width() - 90.0;
                let r = ui.add(
                    egui::TextEdit::multiline(&mut self.text_draft)
                        .hint_text("Type or paste text / a link… (Ctrl+Enter to send)")
                        .desired_rows(3)
                        .desired_width(w),
                );
                let ctrl_enter = r.has_focus()
                    && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
                if ui
                    .add_sized([80.0, 30.0], egui::Button::new("Send"))
                    .clicked()
                    || ctrl_enter
                {
                    let text = std::mem::take(&mut self.text_draft);
                    self.send_text(snap, text);
                }
            });
        });

        if let Some((msg, at)) = &self.status {
            if at
                .elapsed()
                .map(|e| e < Duration::from_secs(4))
                .unwrap_or(false)
            {
                ui.label(RichText::new(msg).weak());
            } else {
                self.status = None;
            }
        }
    }

    fn pair_dialogs(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        let Some(req) = snap.pair_requests.first() else {
            return;
        };
        let mut decision: Option<bool> = None;
        egui::Window::new("Pairing request")
            .id(egui::Id::new(("pair", req.id)))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(320.0);
                ui.label(RichText::new(&req.peer_name).size(18.0).strong());
                ui.label(RichText::new(req.ip.to_string()).weak());
                ui.add_space(8.0);
                ui.label("Check that the phone shows the same code:");
                ui.label(
                    RichText::new(spaced_code(&req.code))
                        .size(32.0)
                        .monospace()
                        .strong()
                        .color(ACCENT),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Accept").strong())
                                .fill(ACCENT.linear_multiply(0.6)),
                        )
                        .clicked()
                    {
                        decision = Some(true);
                    }
                    if ui.button("Reject").clicked() {
                        decision = Some(false);
                    }
                    ui.label(RichText::new(format!("{}s", req.expires_in_secs)).weak());
                });
            });
        if let Some(accept) = decision {
            self.core.respond_pair(req.id, accept);
            if accept {
                self.selected = Some(req.peer_id.clone());
            }
        }
    }
}

// ---------------------------------------------------------------- helpers

fn open_in_explorer(path: &Path, select: bool) {
    let mut cmd = std::process::Command::new("explorer");
    if select {
        cmd.arg(format!("/select,{}", path.display()));
    } else {
        cmd.arg(path);
    }
    let _ = cmd.spawn();
}

pub fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

fn fmt_ago(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

fn fmt_time_ago(t: SystemTime) -> String {
    let s = t.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    if s < 5 {
        "just now".into()
    } else {
        format!("{} ago", fmt_ago(s))
    }
}

fn spaced_code(code: &str) -> String {
    if code.len() == 6 {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

fn is_url(s: &str) -> bool {
    let s = s.trim();
    (s.starts_with("http://") || s.starts_with("https://")) && !s.contains(char::is_whitespace)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}
