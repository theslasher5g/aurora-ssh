//! Operations features hung into the app through one state field and one action:
//! runbooks, maintenance windows and diagnostics bundles.

use uuid::Uuid;

use crate::app::{Action, App};
use crate::ui::maintenance::Target;

#[derive(Default)]
pub struct OpsState {
    /// Snippets page: 0 = snippets, 1 = runbooks.
    pub page: usize,
    pub runbooks: crate::ui::runbooks::RunbookUi,
    pub maint: crate::ui::maintenance::MaintUi,
    pub diag: Option<crate::ui::diag::DiagUi>,
}

#[derive(Clone, Debug)]
pub enum OpsAction {
    NewRunbook,
    EditRunbook(Uuid),
    DuplicateRunbook(Uuid),
    DeleteRunbook(Uuid),
    ExportRunbook(Uuid),
    ImportRunbook,
    /// Start sheet: pick hosts/sessions, fill in variables. Optional preselected tab.
    RunRunbook(Uuid, Option<u64>),
    /// "Wartung starten…" for a host or group.
    MaintStart(Target),
    MaintExtend(Uuid, i64),
    MaintEnd(Uuid),
    /// "Diagnose-Paket erstellen…" for a host.
    Diag(Uuid),
}

pub fn apply(app: &mut App, a: OpsAction, ctx: &egui::Context) {
    use crate::ui::{diag, maintenance, runbooks};
    match a {
        OpsAction::MaintStart(t) => maintenance::open_sheet(app, t),
        OpsAction::MaintExtend(id, min) => maintenance::extend(app, id, min),
        OpsAction::MaintEnd(id) => maintenance::end(app, id),
        OpsAction::Diag(h) => diag::open(app, h),
        other => runbooks::apply(app, other, ctx),
    }
}

/// Sheets, timers and side tasks; called once per frame.
pub fn show(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    crate::ui::runbooks::windows(app, ctx, actions);
    crate::ui::maintenance::show(app, ctx, actions);
    crate::ui::diag::show(app, ctx);
}
