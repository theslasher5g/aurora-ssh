//! Migration importer: the sheet (pick a file → preview → result). Parsing happens in
//! `backend::import_*`; this file only holds the state and the drawing.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Color32, RichText};
use uuid::Uuid;

use crate::app::App;
use crate::backend::import_model::{ImportDoc, Kind, Skipped};
use crate::backend::import_parse;
use crate::backend::import_plan::{self, Conflict, Options, Report};
use crate::model::{Library, Protocol};
use crate::theme;

const ROW_H: f32 = 26.0;

#[derive(Default)]
pub struct State {
    pub open: bool,
    path: String,
    rx: Option<Receiver<Result<ImportDoc, String>>>,
    error: Option<String>,
    loaded: Option<Box<Loaded>>,
    done: Option<Done>,
}

impl State {
    /// Opens the sheet on the file picker.
    pub fn open(&mut self) {
        *self = State {
            open: true,
            ..Default::default()
        };
    }
}

struct Done {
    report: Report,
    source: String,
    skipped: Vec<Skipped>,
    skipped_total: usize,
    warnings: Vec<String>,
}

struct Folder {
    name: String,
    parent: usize,
    folders: Vec<usize>,
    items: Vec<usize>,
    total: usize,
    sel: usize,
}

#[derive(Clone, Copy)]
enum Row {
    Folder(usize, usize),
    Item(usize, usize),
}

struct Loaded {
    doc: ImportDoc,
    file: String,
    selected: Vec<bool>,
    conflicts: Vec<bool>,
    n_conflicts: usize,
    folders: Vec<Folder>,
    item_folder: Vec<usize>,
    expanded: HashSet<usize>,
    rows: Vec<Row>,
    rows_dirty: bool,
    filter: String,
    /// 0 = new group, 1 = existing group, 2 = top level.
    target: usize,
    new_name: String,
    parent: Option<Uuid>,
    groups: Vec<(Uuid, String)>,
    conflict: Conflict,
    import_passwords: bool,
    map: HashMap<Kind, Option<Protocol>>,
    has_passwords: bool,
    show_skipped: bool,
}

/// Folder tree of the group paths (folder 0 is the invisible root).
fn build_tree(doc: &ImportDoc) -> (Vec<Folder>, Vec<usize>) {
    let mut f = vec![Folder {
        name: String::new(),
        parent: 0,
        folders: Vec::new(),
        items: Vec::new(),
        total: 0,
        sel: 0,
    }];
    let mut index: HashMap<(usize, String), usize> = HashMap::new();
    let mut item_folder = Vec::with_capacity(doc.items.len());
    for (n, it) in doc.items.iter().enumerate() {
        let mut cur = 0;
        for p in &it.group {
            let k = (cur, p.to_lowercase());
            cur = match index.get(&k) {
                Some(i) => *i,
                None => {
                    let i = f.len();
                    f.push(Folder {
                        name: p.clone(),
                        parent: cur,
                        folders: Vec::new(),
                        items: Vec::new(),
                        total: 0,
                        sel: 0,
                    });
                    f[cur].folders.push(i);
                    index.insert(k, i);
                    i
                }
            };
        }
        f[cur].items.push(n);
        item_folder.push(cur);
        let mut a = cur;
        loop {
            f[a].total += 1;
            if a == 0 {
                break;
            }
            a = f[a].parent;
        }
    }
    for i in 0..f.len() {
        let mut kids = std::mem::take(&mut f[i].folders);
        kids.sort_by_key(|k| f[*k].name.to_lowercase());
        f[i].folders = kids;
    }
    (f, item_folder)
}

fn group_paths(lib: &Library) -> Vec<(Uuid, String)> {
    let by_id: HashMap<Uuid, &crate::model::Group> = lib.groups.iter().map(|g| (g.id, g)).collect();
    let mut out: Vec<(Uuid, String)> = lib
        .groups
        .iter()
        .map(|g| {
            let mut parts = vec![g.name.clone()];
            let mut next = g.parent_id;
            let mut guard = 0;
            while let Some(p) = next.and_then(|id| by_id.get(&id)) {
                parts.push(p.name.clone());
                next = p.parent_id;
                guard += 1;
                if guard > 32 {
                    break;
                }
            }
            parts.reverse();
            (g.id, parts.join(" / "))
        })
        .collect();
    out.sort_by_key(|(_, p)| p.to_lowercase());
    out
}

impl Loaded {
    fn new(doc: ImportDoc, file: String, lib: &Library) -> Self {
        let (folders, item_folder) = build_tree(&doc);
        let n = doc.items.len();
        let conflicts = import_plan::find_conflicts(lib, &doc);
        let n_conflicts = conflicts.iter().filter(|c| **c).count();
        let src = doc.source.map_or("", |s| s.label());
        let has_passwords = doc.items.iter().any(|i| !i.password.is_empty());
        let mut l = Loaded {
            new_name: format!("{src} {}", chrono::Local::now().format("%Y-%m-%d")),
            file,
            selected: vec![true; n],
            conflicts,
            n_conflicts,
            folders,
            item_folder,
            expanded: HashSet::new(),
            rows: Vec::new(),
            rows_dirty: true,
            filter: String::new(),
            target: 0,
            parent: None,
            groups: group_paths(lib),
            conflict: Conflict::Skip,
            import_passwords: false,
            map: import_plan::default_map(),
            has_passwords,
            show_skipped: false,
            doc,
        };
        if n <= 40 {
            l.expanded = (0..l.folders.len()).collect();
        }
        l.recount();
        l
    }

    fn mapped(&self, i: usize) -> bool {
        self.map
            .get(&self.doc.items[i].kind())
            .is_some_and(|m| m.is_some())
    }

    /// Items that will be imported (ticked and with a protocol that is not "skip").
    fn effective(&self) -> Vec<bool> {
        (0..self.selected.len())
            .map(|i| self.selected[i] && self.mapped(i))
            .collect()
    }

    fn recount(&mut self) {
        for f in &mut self.folders {
            f.sel = 0;
        }
        for (i, on) in self.selected.iter().enumerate() {
            if !*on {
                continue;
            }
            let mut a = self.item_folder[i];
            loop {
                self.folders[a].sel += 1;
                if a == 0 {
                    break;
                }
                a = self.folders[a].parent;
            }
        }
    }

    fn items_below(&self, folder: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![folder];
        while let Some(f) = stack.pop() {
            out.extend(&self.folders[f].items);
            stack.extend(&self.folders[f].folders);
        }
        out
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        let q = self.filter.trim().to_lowercase();
        if !q.is_empty() {
            for (i, it) in self.doc.items.iter().enumerate() {
                if it.name.to_lowercase().contains(&q)
                    || it.host.to_lowercase().contains(&q)
                    || it.group.iter().any(|g| g.to_lowercase().contains(&q))
                {
                    self.rows.push(Row::Item(i, 0));
                }
            }
        } else {
            enum Work {
                Visit(usize, usize),
                Item(usize, usize),
            }
            let mut stack = vec![Work::Visit(0, 0)];
            let mut out = Vec::new();
            while let Some(w) = stack.pop() {
                match w {
                    Work::Item(i, d) => out.push(Row::Item(i, d)),
                    Work::Visit(f, d) => {
                        let child_depth = if f == 0 {
                            0
                        } else {
                            out.push(Row::Folder(f, d));
                            if !self.expanded.contains(&f) {
                                continue;
                            }
                            d + 1
                        };
                        // Folders come before items, so they are pushed last.
                        for i in self.folders[f].items.iter().rev() {
                            stack.push(Work::Item(*i, child_depth));
                        }
                        for sub in self.folders[f].folders.iter().rev() {
                            stack.push(Work::Visit(*sub, child_depth));
                        }
                    }
                }
            }
            self.rows = out;
        }
        self.rows_dirty = false;
    }
}

fn start_load(st: &mut State, ctx: &egui::Context, path: &str) {
    let p = path.trim().trim_matches('"').to_string();
    if p.is_empty() {
        return;
    }
    let expanded = match p.strip_prefix("~/") {
        Some(rest) => directories::UserDirs::new()
            .map(|d| d.home_dir().join(rest))
            .unwrap_or_else(|| PathBuf::from(&p)),
        None => PathBuf::from(&p),
    };
    st.path = p;
    st.error = None;
    let (tx, rx) = std::sync::mpsc::channel();
    st.rx = Some(rx);
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let r = import_parse::parse_file(&expanded);
        let _ = tx.send(r);
        ctx.request_repaint();
    });
}

enum Act {
    None,
    Close,
    Back,
    Import,
}

/// Draws the sheet; called once per frame.
pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.import.open {
        return;
    }
    let mut st = std::mem::take(&mut app.import);
    // A file dropped on the window.
    let dropped = ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|f| f.path().to_string_lossy().into_owned())
            .find(|p| !p.is_empty())
    });
    if let Some(p) = dropped
        && st.rx.is_none()
        && st.done.is_none()
    {
        st.loaded = None;
        start_load(&mut st, ctx, &p);
    }
    if let Some(rx) = &st.rx {
        match rx.try_recv() {
            Ok(r) => {
                st.rx = None;
                match r {
                    Ok(doc) => {
                        let file = std::path::Path::new(&st.path)
                            .file_name()
                            .map_or(st.path.clone(), |n| n.to_string_lossy().into_owned());
                        st.loaded = Some(Box::new(Loaded::new(doc, file, &app.lib)));
                    }
                    Err(e) => st.error = Some(e),
                }
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(80))
            }
            Err(TryRecvError::Disconnected) => {
                st.rx = None;
                st.error = Some(t!("Lesen abgebrochen.").to_string());
            }
        }
    }

    let loading = st.rx.is_some();
    let n_effective = st
        .loaded
        .as_ref()
        .map_or(0, |l| l.effective().iter().filter(|b| **b).count());
    let stage = if st.done.is_some() {
        2
    } else if st.loaded.is_some() {
        1
    } else {
        0
    };
    let title = t!("Aus anderem Verbindungsmanager importieren");
    let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());

    let (act, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("import_sheet"),
        680.0,
        title,
        |ui| match stage {
            0 => pick_page(&mut st, ui, ctx, loading, hovering),
            1 => {
                if let Some(l) = st.loaded.as_mut() {
                    preview_page(l, ui, &app.vault.is_some());
                }
            }
            _ => {
                if let Some(d) = &st.done {
                    done_page(d, ui);
                }
            }
        },
        |ui, _| match stage {
            0 => {
                if theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked() {
                    Act::Close
                } else {
                    Act::None
                }
            }
            1 => {
                let mut a = Act::None;
                ui.add_enabled_ui(n_effective > 0, |ui| {
                    let label = t!("{} importieren", n_effective);
                    if theme::btn(ui, theme::Btn::Primary, None, &label).clicked() {
                        a = Act::Import;
                    }
                });
                if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                    a = Act::Close;
                }
                if theme::btn(ui, theme::Btn::Normal, None, t!("Andere Datei")).clicked() {
                    a = Act::Back;
                }
                a
            }
            _ => {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Fertig")).clicked() {
                    Act::Close
                } else {
                    Act::None
                }
            }
        },
    );

    match act {
        Act::Close => st.open = false,
        Act::Back => {
            st.loaded = None;
            st.error = None;
        }
        Act::Import => run_import(app, &mut st),
        Act::None => {}
    }
    if dismissed {
        st.open = false;
    }
    if !st.open {
        st = State::default();
    }
    app.import = st;
}

fn run_import(app: &mut App, st: &mut State) {
    let Some(l) = st.loaded.take() else { return };
    let opts = Options {
        parent: if l.target == 1 { l.parent } else { None },
        new_group: (l.target == 0).then(|| l.new_name.clone()),
        conflict: l.conflict,
        import_passwords: l.import_passwords && app.vault.is_some(),
        map: l.map.clone(),
    };
    let sel = l.selected.clone();
    let report = import_plan::apply(&mut app.lib, &mut app.keychain, &l.doc, &sel, &opts);
    app.save_library();
    if report.passwords_stored > 0 {
        app.save_keychain();
    }
    let mut warnings = l.doc.warnings.clone();
    if l.doc.passwords_not_imported > 0 {
        warnings.push(t!(
            "{} Passwörter aus der Quelle wurden nicht importiert.",
            l.doc.passwords_not_imported
        ));
    }
    app.toast(
        t!(
            "{} Hosts importiert, {} übersprungen",
            report.imported + report.overwritten,
            report.skipped_total()
        ),
        theme::green(),
    );
    st.done = Some(Done {
        report,
        source: l
            .doc
            .source
            .map_or(String::new(), |s| s.label().to_string()),
        skipped: l.doc.skipped.clone(),
        skipped_total: l.doc.skipped_total,
        warnings,
    });
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.5).color(theme::text_dim())).wrap());
}

fn pick_page(
    st: &mut State,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    loading: bool,
    hovering: bool,
) {
    hint(
        ui,
        t!(
            "Hosts aus mRemoteNG, Royal TS, Remote Desktop Manager oder einer CSV-Tabelle übernehmen. Die Datei wird nur lokal gelesen."
        ),
    );
    ui.add_space(4.0);
    let mut go = false;
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 100.0).max(120.0);
        let r = ui.add(
            egui::TextEdit::singleline(&mut st.path)
                .hint_text(RichText::new(t!("Pfad zur Exportdatei …")).color(theme::text_dim()))
                .desired_width(w),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            go = true;
        }
        ui.add_enabled_ui(!loading && !st.path.trim().is_empty(), |ui| {
            if theme::btn(ui, theme::Btn::Primary, None, t!("Öffnen")).clicked() {
                go = true;
            }
        });
    });
    if go {
        let p = st.path.clone();
        start_load(st, ctx, &p);
    }
    // Drop zone
    let h = 64.0;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(10),
        egui::Stroke::new(
            1.5,
            if hovering {
                theme::accent()
            } else {
                theme::line()
            },
        ),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        if loading {
            t!("Lese Datei …")
        } else {
            t!("Datei hierher ziehen")
        },
        egui::FontId::proportional(14.0),
        theme::text_dim(),
    );
    if let Some(e) = &st.error {
        ui.add(egui::Label::new(RichText::new(e).color(theme::red())).wrap());
    }
    ui.add_space(6.0);
    theme::section_title(ui, t!("So exportierst du"));
    theme::settings_card(ui, |ui, rows| {
        for (name, how) in [
            (
                "mRemoteNG",
                t!(
                    "confCons.xml (unverschlüsselt) – liegt unter %APPDATA%\\mRemoteNG oder per Datei → Verbindungen exportieren."
                ),
            ),
            (
                "Royal TS / TSX",
                t!(
                    "Datei → Importieren & Exportieren → Exportieren → Royal JSON (.json). Passwörter werden von Royal nicht im Klartext exportiert."
                ),
            ),
            (
                "Remote Desktop Manager",
                t!(
                    "Datei → Exportieren → JSON (oder .rdm als XML). Anmeldedaten-Einträge werden übersprungen."
                ),
            ),
            (
                "CSV",
                t!(
                    "Spalten name, host, port, protocol, user, group, tags, description (Kopfzeile, ; oder , als Trenner, optional password)."
                ),
            ),
        ] {
            rows.full(ui, |ui| {
                ui.label(RichText::new(name).strong().color(theme::text()));
                hint(ui, how);
            });
        }
    });
}

fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Ssh => theme::accent(),
        Kind::Rdp => theme::amber(),
        Kind::Vnc | Kind::Telnet => theme::green(),
        Kind::Web | Kind::Other => theme::text_dim(),
    }
}

fn preview_page(l: &mut Loaded, ui: &mut egui::Ui, vault_open: &bool) {
    let src = l.doc.source.map_or("", |s| s.label());
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(src).strong().color(theme::text()));
        ui.label(RichText::new(&l.file).color(theme::text_dim()));
    });
    let folders = l.folders.len() - 1;
    ui.label(
        RichText::new(t!("{} Einträge in {} Ordnern", l.doc.items.len(), folders))
            .color(theme::text_dim()),
    );
    ui.horizontal_wrapped(|ui| {
        for (k, n) in l.doc.kind_counts() {
            theme::pill(ui, &format!("{} {n}", k.label()), kind_color(k));
        }
        if l.n_conflicts > 0 {
            theme::pill(ui, &t!("{} Konflikte", l.n_conflicts), theme::amber());
        }
    });
    if l.doc.encrypted {
        ui.add(
            egui::Label::new(
                RichText::new(t!(
                    "Verschlüsselte Datei – es kann nichts importiert werden."
                ))
                .color(theme::red())
                .strong(),
            )
            .wrap(),
        );
    }
    for w in &l.doc.warnings {
        ui.add(
            egui::Label::new(
                RichText::new(format!("⚠ {w}"))
                    .size(12.5)
                    .color(theme::amber()),
            )
            .wrap(),
        );
    }
    if l.doc.passwords_not_imported > 0 {
        ui.add(
            egui::Label::new(
                RichText::new(t!(
                    "Passwörter werden nicht importiert ({} Einträge hatten eines).",
                    l.doc.passwords_not_imported
                ))
                .size(12.5)
                .color(theme::amber()),
            )
            .wrap(),
        );
    }
    if l.doc.skipped_total > 0 {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(t!(
                    "{} Einträge nicht lesbar / nicht unterstützt",
                    l.doc.skipped_total
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );
            let label = if l.show_skipped {
                t!("Ausblenden")
            } else {
                t!("Anzeigen")
            };
            if theme::btn(ui, theme::Btn::Small, None, label).clicked() {
                l.show_skipped = !l.show_skipped;
            }
        });
        if l.show_skipped {
            for s in l.doc.skipped.iter().take(60) {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{} – {}", s.name, s.reason))
                            .size(12.0)
                            .color(theme::text_dim()),
                    )
                    .truncate(),
                );
            }
        }
    }

    // Target
    ui.add_space(6.0);
    theme::section_title(ui, t!("Ziel"));
    let names = [
        t!("Neue Gruppe"),
        t!("Bestehende Gruppe"),
        t!("Oberste Ebene"),
    ];
    if let Some(i) = theme::segmented(ui, &names, l.target) {
        l.target = i;
    }
    match l.target {
        0 => {
            ui.add(
                egui::TextEdit::singleline(&mut l.new_name)
                    .hint_text(RichText::new(t!("Name der neuen Gruppe")).color(theme::text_dim()))
                    .desired_width(f32::INFINITY),
            );
        }
        1 => {
            let cur = l
                .groups
                .iter()
                .find(|(id, _)| Some(*id) == l.parent)
                .map_or_else(|| t!("Gruppe wählen …").to_string(), |(_, p)| p.clone());
            egui::ComboBox::from_id_salt("imp_parent")
                .width(ui.available_width().min(520.0))
                .selected_text(cur)
                .show_ui(ui, |ui| {
                    for (id, path) in &l.groups {
                        ui.selectable_value(&mut l.parent, Some(*id), path);
                    }
                });
            if l.groups.is_empty() {
                hint(ui, t!("Es gibt noch keine Gruppe."));
            }
        }
        _ => {}
    }

    // Conflicts
    if l.n_conflicts > 0 {
        ui.add_space(6.0);
        theme::section_title(ui, t!("Schon vorhanden (gleicher Name und Host)"));
        let opts = [
            t!("Überspringen"),
            t!("Überschreiben"),
            t!("Beide behalten"),
        ];
        let cur = match l.conflict {
            Conflict::Skip => 0,
            Conflict::Overwrite => 1,
            Conflict::KeepBoth => 2,
        };
        if let Some(i) = theme::segmented(ui, &opts, cur) {
            l.conflict = [Conflict::Skip, Conflict::Overwrite, Conflict::KeepBoth][i];
        }
    }

    // Protocol mapping
    ui.add_space(6.0);
    theme::section_title(ui, t!("Protokolle zuordnen"));
    let counts = l.doc.kind_counts();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
        for (k, n) in counts {
            ui.label(RichText::new(format!("{} ({n}) →", k.label())).color(theme::text()));
            let cur = l.map.get(&k).copied().flatten();
            let text = cur.map_or_else(
                || t!("Nicht importieren").to_string(),
                |p| p.label().to_string(),
            );
            egui::ComboBox::from_id_salt(("imp_map", k.label()))
                .selected_text(text)
                .show_ui(ui, |ui| {
                    for p in [
                        Protocol::Ssh,
                        Protocol::Rdp,
                        Protocol::Vnc,
                        Protocol::Telnet,
                        Protocol::Web,
                    ] {
                        if ui.selectable_label(cur == Some(p), p.label()).clicked() {
                            l.map.insert(k, Some(p));
                        }
                    }
                    if ui
                        .selectable_label(cur.is_none(), t!("Nicht importieren"))
                        .clicked()
                    {
                        l.map.insert(k, None);
                    }
                });
            ui.add_space(10.0);
        }
    });

    // Passwords (clear text sources only)
    if l.has_passwords {
        ui.add_space(6.0);
        theme::settings_card(ui, |ui, rows| {
            rows.setting(
                ui,
                t!("Passwörter aus der Datei im Tresor speichern"),
                Some(if *vault_open {
                    t!("Die Datei enthält Passwörter im Klartext. Sie landen nur im verschlüsselten Tresor (Schlüsselbund), nie in der Hostliste. Danach die Datei löschen.")
                } else {
                    t!("Der Tresor ist nicht entsperrt.")
                }),
                |ui| {
                    ui.add_enabled_ui(*vault_open, |ui| {
                        theme::toggle(ui, &mut l.import_passwords);
                    });
                },
            );
        });
    }

    // Entries
    ui.add_space(6.0);
    theme::section_title(ui, t!("Einträge"));
    let mut dirty = false;
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(&mut l.filter)
                .hint_text(RichText::new(t!("Suchen …")).color(theme::text_dim()))
                .desired_width((ui.available_width() - 170.0).max(80.0)),
        );
        if r.changed() {
            dirty = true;
        }
        let all = theme::btn(ui, theme::Btn::Small, None, t!("Alle")).clicked();
        let none = theme::btn(ui, theme::Btn::Small, None, t!("Keine")).clicked();
        if all || none {
            if l.rows_dirty {
                l.rebuild_rows();
            }
            let targets: Vec<usize> = if l.filter.trim().is_empty() {
                (0..l.selected.len()).collect()
            } else {
                l.rows
                    .iter()
                    .filter_map(|r| {
                        if let Row::Item(i, _) = r {
                            Some(*i)
                        } else {
                            None
                        }
                    })
                    .collect()
            };
            for i in targets {
                l.selected[i] = all;
            }
            l.recount();
        }
    });
    if dirty {
        l.rows_dirty = true;
    }
    if l.rows_dirty {
        l.rebuild_rows();
    }
    let sel_n = l.selected.iter().filter(|b| **b).count();
    hint(ui, &t!("{} von {} ausgewählt", sel_n, l.selected.len()));
    let avail = ui.ctx().content_rect().height();
    let h = (avail * 0.4).clamp(130.0, 320.0);
    let mut toggles: Vec<(bool, usize)> = Vec::new(); // (is_folder, id)
    let mut expand: Vec<usize> = Vec::new();
    let rows = l.rows.clone();
    egui::ScrollArea::vertical()
        .id_salt("imp_rows")
        .max_height(h)
        .auto_shrink([false, false])
        .show_rows(ui, ROW_H, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for r in &rows[range] {
                match *r {
                    Row::Folder(f, depth) => {
                        let fo = &l.folders[f];
                        ui.horizontal(|ui| {
                            ui.set_height(ROW_H);
                            ui.add_space(depth as f32 * 16.0);
                            let open = l.expanded.contains(&f);
                            let (cr, cresp) = ui
                                .allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                            let c = cr.center();
                            let pts = if open {
                                vec![
                                    c + egui::vec2(-4.0, -2.0),
                                    c + egui::vec2(4.0, -2.0),
                                    c + egui::vec2(0.0, 3.0),
                                ]
                            } else {
                                vec![
                                    c + egui::vec2(-2.0, -4.0),
                                    c + egui::vec2(3.0, 0.0),
                                    c + egui::vec2(-2.0, 4.0),
                                ]
                            };
                            ui.painter().add(egui::Shape::convex_polygon(
                                pts,
                                theme::text_dim(),
                                egui::Stroke::NONE,
                            ));
                            if cresp.clicked() {
                                expand.push(f);
                            }
                            let mut on = fo.sel == fo.total && fo.total > 0;
                            if ui.checkbox(&mut on, "").changed() {
                                toggles.push((true, f));
                            }
                            let txt = format!("{}  ({}/{})", fo.name, fo.sel, fo.total);
                            let resp = ui.add(
                                egui::Label::new(RichText::new(txt).strong().color(theme::text()))
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            );
                            if resp.clicked() {
                                expand.push(f);
                            }
                        });
                    }
                    Row::Item(i, depth) => {
                        let it = &l.doc.items[i];
                        let mapped = l.mapped(i);
                        ui.horizontal(|ui| {
                            ui.set_height(ROW_H);
                            ui.add_space(depth as f32 * 16.0 + 20.0);
                            let mut on = l.selected[i];
                            if ui.checkbox(&mut on, "").changed() {
                                toggles.push((false, i));
                            }
                            egui::Sides::new().shrink_left().truncate().show(
                                ui,
                                |ui| {
                                    let c = if mapped {
                                        theme::text()
                                    } else {
                                        theme::text_dim()
                                    };
                                    ui.label(RichText::new(&it.name).color(c));
                                    ui.label(
                                        RichText::new(&it.host).size(12.0).color(theme::text_dim()),
                                    );
                                },
                                |ui| {
                                    if l.conflicts[i] {
                                        theme::pill(ui, t!("Konflikt"), theme::amber());
                                    }
                                    theme::pill(ui, it.kind().label(), kind_color(it.kind()));
                                },
                            );
                        });
                    }
                }
            }
        });
    for f in expand {
        if !l.expanded.remove(&f) {
            l.expanded.insert(f);
        }
        l.rows_dirty = true;
    }
    if !toggles.is_empty() {
        for (is_folder, id) in toggles {
            if is_folder {
                let on = !(l.folders[id].sel == l.folders[id].total);
                for i in l.items_below(id) {
                    l.selected[i] = on;
                }
            } else {
                l.selected[id] = !l.selected[id];
            }
        }
        l.recount();
    }
}

fn done_page(d: &Done, ui: &mut egui::Ui) {
    let ok = d.report.imported + d.report.overwritten;
    ui.label(
        RichText::new(t!("{} Hosts importiert", ok))
            .size(18.0)
            .strong()
            .color(theme::green()),
    );
    theme::settings_card(ui, |ui, rows| {
        let mut line = |ui: &mut egui::Ui, a: &str, n: usize| {
            rows.setting(ui, a, None, |ui| {
                ui.label(RichText::new(n.to_string()).color(theme::text()));
            });
        };
        line(ui, t!("Neu angelegt"), d.report.imported);
        if d.report.overwritten > 0 {
            line(ui, t!("Überschrieben"), d.report.overwritten);
        }
        line(ui, t!("Neue Gruppen"), d.report.groups_created);
        if d.report.passwords_stored > 0 {
            line(ui, t!("Passwörter im Tresor"), d.report.passwords_stored);
        }
        line(ui, t!("Übersprungen"), d.report.skipped_total());
    });
    if !d.report.skipped.is_empty() {
        ui.add_space(4.0);
        theme::section_title(ui, t!("Gründe fürs Überspringen"));
        for (reason, n) in &d.report.skipped {
            ui.add(
                egui::Label::new(RichText::new(format!("{n} × {reason}")).color(theme::text_dim()))
                    .wrap(),
            );
        }
    }
    for w in &d.warnings {
        ui.add(
            egui::Label::new(
                RichText::new(format!("⚠ {w}"))
                    .size(12.5)
                    .color(theme::amber()),
            )
            .wrap(),
        );
    }
    if !d.skipped.is_empty() {
        ui.add_space(4.0);
        theme::section_title(ui, &format!("{} – {}", d.source, t!("Nicht gelesen")));
        for s in d.skipped.iter().take(100) {
            ui.add(
                egui::Label::new(
                    RichText::new(format!("{} – {}", s.name, s.reason))
                        .size(12.0)
                        .color(theme::text_dim()),
                )
                .truncate(),
            );
        }
        if d.skipped_total > 100 {
            hint(ui, &t!("… und {} weitere", d.skipped_total - 100));
        }
    }
}
