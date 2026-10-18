//! "Diagnose-Paket erstellen…" sheet: pick what to collect, run it over a separate SSH
//! connection, preview the results and save them as `.tar.gz` (optionally redacted).

use std::path::PathBuf;

use chrono::{DateTime, Local};
use egui::RichText;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::app::{App, Purpose};
use crate::backend::diag::{self, DiagEvent, DiagItem, ItemResult};
use crate::backend::runbook::{open_folder, output_dir, safe_name};
use crate::backend::{Sink, ssh::SshParams};
use crate::model::Protocol;
use crate::theme;
use crate::ui::icons::Icon;

#[derive(PartialEq)]
enum Phase {
    Config,
    Running,
    Done,
    Saved(PathBuf),
}

pub struct DiagUi {
    host: Uuid,
    name: String,
    address: String,
    items: Vec<(DiagItem, bool)>,
    custom: String,
    sudo: bool,
    /// Only kept until the collection starts.
    sudo_pw: String,
    redact: bool,
    phase: Phase,
    run_items: Vec<DiagItem>,
    results: Vec<Option<ItemResult>>,
    current: Option<usize>,
    hostname: String,
    user: String,
    used_sudo: bool,
    started: Option<DateTime<Local>>,
    error: Option<String>,
    rx: Option<UnboundedReceiver<DiagEvent>>,
    preview: Option<usize>,
}

pub fn open(app: &mut App, host: Uuid) {
    let Some(h) = app.lib.host(host).filter(|h| h.protocol == Protocol::Ssh) else {
        return;
    };
    app.ops.diag = Some(DiagUi {
        host,
        name: h.display_name().to_string(),
        address: h.hostname.trim().to_string(),
        items: diag::builtin_items()
            .into_iter()
            .map(|i| (i, true))
            .collect(),
        custom: String::new(),
        sudo: false,
        sudo_pw: String::new(),
        redact: false,
        phase: Phase::Config,
        run_items: Vec::new(),
        results: Vec::new(),
        current: None,
        hostname: String::new(),
        user: String::new(),
        used_sudo: false,
        started: None,
        error: None,
        rx: None,
        preview: None,
    });
}

/// Called by `open_session` once the login is known (`Purpose::Diag`).
pub fn start(app: &mut App, params: SshParams, ctx: &egui::Context) {
    let prompts = app.prompts_sink.clone();
    let Some(d) = app.ops.diag.as_mut() else {
        return;
    };
    if d.phase != Phase::Running {
        return;
    }
    let (sink, rx) = Sink::new(ctx.clone());
    d.rx = Some(rx);
    d.user = params.username.clone();
    let pw = std::mem::take(&mut d.sudo_pw);
    d.used_sudo = d.sudo;
    let sudo = d.sudo.then_some(pw);
    diag::spawn(&app.rt, params, d.run_items.clone(), sudo, sink, prompts);
}

fn begin(app: &mut App, ctx: &egui::Context) {
    let Some(d) = app.ops.diag.as_mut() else {
        return;
    };
    d.run_items = d
        .items
        .iter()
        .filter(|(_, on)| *on)
        .map(|(i, _)| i.clone())
        .chain(diag::custom_items(&d.custom))
        .collect();
    d.results = vec![None; d.run_items.len()];
    d.current = None;
    d.error = None;
    d.preview = None;
    d.started = Some(Local::now());
    d.phase = Phase::Running;
    let host = d.host;
    if let Some(h) = app.lib.host(host).cloned() {
        app.connect_host(h, Purpose::Diag, ctx);
    }
}

fn group_label(g: &str) -> &'static str {
    match g {
        "System" => "System",
        "Logs" => "Logs",
        "Dienste" => t!("Dienste"),
        "Container" => "Container",
        _ => t!("Eigene"),
    }
}

/// Builds the archive members: one file per item plus `summary.txt`.
fn bundle(d: &DiagUi, names: &[String]) -> (String, Vec<(String, Vec<u8>)>) {
    let started = d.started.unwrap_or_else(Local::now);
    let dir = format!("{}_{}", safe_name(&d.name), started.format("%Y-%m-%d_%H%M"));
    let mut red = d.redact.then(|| diag::Redactor::new(names));
    let mut clean = |data: &[u8]| -> Vec<u8> {
        match red.as_mut() {
            Some(r) => r.redact(&String::from_utf8_lossy(data)).into_bytes(),
            None => data.to_vec(),
        }
    };
    let mut files = Vec::new();
    let mut summary = format!(
        "{}\n\n{}: {} ({})\n{}: {}\n{}: {}\n{}: {}\nsudo: {}\n{}: {}\n\n",
        t!("Aurora Diagnose-Paket"),
        t!("Host"),
        d.name,
        d.address,
        t!("Hostname"),
        d.hostname,
        t!("Benutzer"),
        d.user,
        t!("Erstellt"),
        started.format("%Y-%m-%d %H:%M:%S %z"),
        if d.used_sudo { t!("ja") } else { t!("nein") },
        t!("Geschwärzt"),
        if d.redact { t!("ja") } else { t!("nein") },
    );
    for (item, res) in d.run_items.iter().zip(&d.results) {
        let line = match res {
            Some(r) => {
                let status = match &r.error {
                    Some(e) => format!("{}: {e}", t!("Fehler")),
                    None => format!("exit {}", r.status),
                };
                files.push((format!("{dir}/{}", item.file), clean(&r.output)));
                format!(
                    "{:<24} {:<10} {:>10} {:>8.1}s  {}\n",
                    item.file,
                    status,
                    crate::ui::human_size(r.output.len() as u64),
                    r.took.as_secs_f32(),
                    item.command
                )
            }
            None => format!(
                "{:<24} {}  {}\n",
                item.file,
                t!("nicht gelaufen"),
                item.command
            ),
        };
        summary.push_str(&line);
    }
    let summary = clean(summary.as_bytes());
    files.insert(0, (format!("{dir}/summary.txt"), summary));
    (dir, files)
}

fn save(app: &mut App) {
    let Some(d) = app.ops.diag.as_ref() else {
        return;
    };
    let mut names = vec![d.name.clone(), d.address.clone(), d.hostname.clone()];
    if let Some(f) = app.lib.facts.get(&d.host) {
        names.push(f.hostname.clone());
    }
    let (dir, files) = bundle(d, &names);
    let path = output_dir("Diagnose").join(format!("{dir}.tar.gz"));
    let mtime = d.started.map(|t| t.timestamp().max(0) as u64).unwrap_or(0);
    match diag::write_tar_gz(&path, &files, mtime) {
        Ok(()) => {
            app.toast(
                t!("Diagnose-Paket gespeichert: {}", path.display()),
                theme::green(),
            );
            if let Some(d) = app.ops.diag.as_mut() {
                d.phase = Phase::Saved(path);
            }
        }
        Err(e) => app.toast(
            t!("Speichern fehlgeschlagen: {}", format!("{e:#}")),
            theme::red(),
        ),
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(d) = app.ops.diag.as_mut() else {
        return;
    };
    while let Some(ev) = d.rx.as_mut().and_then(|rx| rx.try_recv().ok()) {
        match ev {
            DiagEvent::Connected { hostname } => d.hostname = hostname,
            DiagEvent::Started(i) => d.current = Some(i),
            DiagEvent::Finished(i, r) => {
                if let Some(slot) = d.results.get_mut(i) {
                    *slot = Some(r);
                }
            }
            DiagEvent::Failed(e) => {
                d.error = Some(e);
                d.phase = Phase::Config;
                d.rx = None;
                break;
            }
            DiagEvent::Done => {
                d.current = None;
                d.phase = Phase::Done;
                d.rx = None;
                break;
            }
        }
    }
    let title = t!("Diagnose-Paket · {}", d.name);
    #[derive(PartialEq)]
    enum Btn {
        None,
        Collect,
        Cancel,
        Close,
        Save,
        Back,
        Folder,
        CopyPath,
    }
    let can_collect = d.items.iter().any(|(_, on)| *on) || !d.custom.trim().is_empty();
    let can_collect = can_collect && (!d.sudo || !d.sudo_pw.is_empty());
    let stage = match &d.phase {
        Phase::Config => 0,
        Phase::Running => 1,
        Phase::Done => 2,
        Phase::Saved(_) => 3,
    };
    let (pressed, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("diag-sheet"),
        620.0,
        &title,
        |ui| match &d.phase {
            Phase::Config => config(ui, d),
            _ => results(ui, d),
        },
        |ui, ()| {
            let mut b = Btn::None;
            match stage {
                0 => {
                    ui.add_enabled_ui(can_collect, |ui| {
                        if theme::btn(ui, theme::Btn::Primary, Some(Icon::Pulse), t!("Sammeln"))
                            .clicked()
                        {
                            b = Btn::Collect;
                        }
                    });
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                        b = Btn::Close;
                    }
                }
                1 => {
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                        b = Btn::Cancel;
                    }
                }
                2 => {
                    if theme::btn(
                        ui,
                        theme::Btn::Primary,
                        Some(Icon::Download),
                        t!("Speichern"),
                    )
                    .clicked()
                    {
                        b = Btn::Save;
                    }
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Zurück")).clicked() {
                        b = Btn::Back;
                    }
                }
                _ => {
                    if theme::btn(ui, theme::Btn::Primary, None, t!("Schließen")).clicked() {
                        b = Btn::Close;
                    }
                    if theme::btn(
                        ui,
                        theme::Btn::Normal,
                        Some(Icon::Folder),
                        t!("Ordner öffnen"),
                    )
                    .clicked()
                    {
                        b = Btn::Folder;
                    }
                    if theme::btn(
                        ui,
                        theme::Btn::Normal,
                        Some(Icon::Copy),
                        t!("Pfad kopieren"),
                    )
                    .clicked()
                    {
                        b = Btn::CopyPath;
                    }
                }
            }
            b
        },
    );
    let running = d.phase == Phase::Running;
    let saved = match &d.phase {
        Phase::Saved(p) => Some(p.clone()),
        _ => None,
    };
    if running {
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    }
    match pressed {
        Btn::Collect => begin(app, ctx),
        Btn::Cancel => {
            // Dropping the receiver stops the collection after the current item.
            d.rx = None;
            d.phase = Phase::Config;
        }
        Btn::Back => d.phase = Phase::Config,
        Btn::Save => save(app),
        Btn::Folder => {
            if let Some(p) = &saved
                && let Some(dir) = p.parent()
                && let Err(e) = open_folder(dir)
            {
                app.toast(e.to_string(), theme::red());
            }
        }
        Btn::CopyPath => {
            if let Some(p) = &saved {
                ctx.copy_text(p.display().to_string());
                app.toast(t!("Pfad kopiert"), theme::accent());
            }
        }
        Btn::Close => app.ops.diag = None,
        Btn::None => {
            if dismissed && !running {
                app.ops.diag = None;
            }
        }
    }
}

fn config(ui: &mut egui::Ui, d: &mut DiagUi) {
    ui.label(
        RichText::new(t!(
            "Liest nur – über eine eigene SSH-Verbindung, ohne die offene Session zu stören. Nichts wird auf dem Server verändert."
        ))
        .size(12.5)
        .color(theme::text_dim()),
    );
    if let Some(e) = &d.error {
        ui.label(RichText::new(e).color(theme::red()));
    }
    let mut groups: Vec<&'static str> = Vec::new();
    for (i, _) in &d.items {
        if !groups.contains(&i.group) {
            groups.push(i.group);
        }
    }
    for g in groups {
        theme::form_caption(ui, group_label(g));
        theme::form(ui, 110.0, |ui, f| {
            for (item, on) in d.items.iter_mut().filter(|(i, _)| i.group == g) {
                f.full(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(on, RichText::new(&item.file).font(theme::semibold(13.0)));
                        ui.add(
                            egui::Label::new(
                                theme::mono(&item.command, 11.5).color(theme::text_dim()),
                            )
                            .truncate(),
                        );
                    });
                });
            }
        });
    }
    theme::form_caption(ui, t!("Eigene Befehle"));
    theme::form(ui, 110.0, |ui, f| {
        f.full(ui, |ui| {
            theme::frameless(ui);
            ui.add(
                egui::TextEdit::multiline(&mut d.custom)
                    .font(egui::TextStyle::Monospace)
                    .hint_text(t!("Ein Befehl pro Zeile, z. B. lsblk"))
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
        });
    });
    theme::form_caption(ui, t!("Optionen"));
    theme::form(ui, 140.0, |ui, f| {
        f.switch(ui, t!("Persönliche Daten schwärzen"), &mut d.redact)
            .on_hover_text(t!(
                "IPv4/IPv6-, MAC- und E-Mail-Adressen sowie Hostnamen werden im Paket durch Platzhalter ersetzt"
            ));
        f.switch(ui, t!("mit sudo"), &mut d.sudo);
        if d.sudo {
            f.field(ui, t!("sudo-Passwort"), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut d.sudo_pw)
                        .password(true)
                        .hint_text(t!("wird nicht gespeichert"))
                        .desired_width(f32::INFINITY),
                );
            });
        }
    });
}

fn results(ui: &mut egui::Ui, d: &mut DiagUi) {
    let done = d.results.iter().filter(|r| r.is_some()).count();
    let total = d.run_items.len();
    let bytes: usize = d.results.iter().flatten().map(|r| r.output.len()).sum();
    let head = match &d.phase {
        Phase::Running if d.current.is_none() && done == 0 => t!("Verbinde…").to_string(),
        Phase::Running => t!("{} von {} gesammelt", done, total),
        _ => t!(
            "{} Einträge · {} – Klick zeigt eine Vorschau",
            total,
            crate::ui::human_size(bytes as u64)
        ),
    };
    ui.horizontal(|ui| {
        if d.phase == Phase::Running {
            ui.add(egui::Spinner::new().size(14.0));
        }
        ui.label(RichText::new(head).color(theme::text_dim()));
    });
    if let Phase::Saved(p) = &d.phase {
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(t!("Gespeichert unter")).color(theme::text_dim()));
            ui.add(egui::Label::new(theme::mono(p.display().to_string(), 12.5)).wrap());
        });
    }
    theme::form(ui, 110.0, |ui, f| {
        for (i, item) in d.run_items.iter().enumerate() {
            let res = d.results.get(i).and_then(|r| r.as_ref());
            f.full(ui, |ui| {
                let resp = ui
                    .horizontal(|ui| {
                        let (color, mark) = match res {
                            Some(r) if r.error.is_some() => (theme::red(), "✖"),
                            Some(r) if r.status != 0 => (theme::amber(), "!"),
                            Some(_) => (theme::green(), "✔"),
                            None if d.current == Some(i) => (theme::accent(), "…"),
                            None => (theme::text_dim(), "·"),
                        };
                        ui.label(RichText::new(mark).color(color).strong());
                        ui.label(RichText::new(&item.file).font(theme::semibold(13.0)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(r) = res {
                                let mut meta = format!(
                                    "{} · {:.1} s",
                                    crate::ui::human_size(r.output.len() as u64),
                                    r.took.as_secs_f32()
                                );
                                if r.status != 0 {
                                    meta = format!("exit {} · {meta}", r.status);
                                }
                                ui.label(RichText::new(meta).size(12.0).color(theme::text_dim()));
                            }
                        });
                    })
                    .response;
                let click = ui.interact(
                    resp.rect,
                    ui.id().with(("diag-row", i)),
                    egui::Sense::click(),
                );
                if res.is_some()
                    && click
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                {
                    d.preview = if d.preview == Some(i) { None } else { Some(i) };
                }
                if d.preview == Some(i)
                    && let Some(r) = res
                {
                    let text = match &r.error {
                        Some(e) => e.clone(),
                        None => {
                            let s = String::from_utf8_lossy(&r.output);
                            let lines: Vec<&str> = s.lines().take(40).collect();
                            let more = s.lines().count().saturating_sub(40);
                            let mut t = lines.join("\n");
                            if more > 0 {
                                t.push_str(&format!("\n… (+{more})"));
                            }
                            t
                        }
                    };
                    egui::Frame::NONE
                        .fill(theme::bg_deep())
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.add(egui::Label::new(theme::mono(text, 11.5)).wrap());
                        });
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_has_summary_and_redacts() {
        let item = DiagItem {
            group: "System",
            file: "ip-addr.txt".into(),
            command: "ip addr".into(),
        };
        let d = DiagUi {
            host: Uuid::new_v4(),
            name: "web 01".into(),
            address: "10.1.2.3".into(),
            items: Vec::new(),
            custom: String::new(),
            sudo: false,
            sudo_pw: String::new(),
            redact: true,
            phase: Phase::Done,
            run_items: vec![
                item.clone(),
                DiagItem {
                    file: "x.txt".into(),
                    ..item
                },
            ],
            results: vec![
                Some(ItemResult {
                    status: 0,
                    output: b"inet 10.1.2.3/24 on web01\n".to_vec(),
                    took: std::time::Duration::from_millis(1200),
                    error: None,
                }),
                None,
            ],
            current: None,
            hostname: "web01".into(),
            user: "ops".into(),
            used_sudo: false,
            started: Some(Local::now()),
            error: None,
            rx: None,
            preview: None,
        };
        let (dir, files) = bundle(&d, &["web01".into(), "10.1.2.3".into()]);
        assert!(dir.starts_with("web_01_"));
        assert_eq!(files.len(), 2);
        assert!(files[0].0.ends_with("/summary.txt"));
        let summary = String::from_utf8(files[0].1.clone()).unwrap();
        assert!(summary.contains("ip-addr.txt"));
        assert!(!summary.contains("10.1.2.3") && !summary.contains("web01"));
        let ip = String::from_utf8(files[1].1.clone()).unwrap();
        assert_eq!(ip, "inet <ip-1>/24 on <host-1>\n");
    }
}
