//! `document.transaction`: several commands on the active document as one all-or-nothing edit,
//! optionally only if the document is still the one, at the revision, the caller last saw.
//!
//! This is what an agent (or any remote caller) needs to edit safely next to a person: a batch
//! that fails halfway leaves nothing behind, and a batch planned against an old state of the
//! document is refused instead of being applied over newer work. The whole transaction runs
//! inside one command, so nothing else can touch the document between its steps.
//!
//! [`Checkpoint`] is the mechanism, shared with `seeing.variant`: the state of every open
//! document is noted before the steps run, each step is checked to have stayed on its own
//! document, and a failure puts every document back.

use magicpic_doc::DocId;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{DocState, EngineError, Result, Session};

const CMD: &str = "document.transaction";
/// Most steps in one transaction (the MCP batch limit).
pub const MAX_STEPS: usize = 256;

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: CMD.into(), msg: msg.into() }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    id: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Params {
    steps: Vec<Step>,
    #[serde(default)]
    expect_revision: Option<u64>,
    #[serde(default)]
    expect_document: Option<u64>,
}

/// Whether command `id` edits only the document it starts on, in a way putting that document
/// back undoes. A transaction (and a seeing recipe) refuses the rest up front: commands that
/// open, save, close, duplicate or switch documents, write or read files, step through history
/// (the transaction is the unit of undo), or change session-wide settings and libraries
/// (preferences, presets, plug-ins), none of which restoring documents takes back.
///
/// This list is the first line only. [`Checkpoint::strayed`] checks after every step that no
/// other document changed, appeared or went away, whatever the command was called.
pub(crate) fn stays_on_document(id: &str) -> bool {
    const LEAVES: &[&str] = &[
        "file.",
        "window.",
        "actions.",
        "automate.",
        "document.",
        "session.",
        "jobs.",
        "command.",
        "plugin.",
        "prefs.",
        "updates.",
        "help.",
        "history.",
        "edit.undo",
        "edit.redo",
        "edit.toggleLastState",
        "edit.stepForward",
        "edit.stepBackward",
        "edit.preferences",
        "edit.purge",
        "edit.keyboardShortcuts",
        "edit.menus",
        "edit.toolbar",
        "edit.presets",
        "edit.colorSettings",
        "edit.installedProfiles",
        "seeing.variant",
        "seeing.study",
        "image.duplicate",
        "channel.split",
        "channel.merge",
        "layer.copyToDocument",
        "layer.exportAs",
        "layer.quickExportAsPng",
        "layer.smartObjects.editContents",
        "layer.smartObjects.saveContents",
        "layer.smartObjects.exportContents",
        "layer.smartObjects.exportSvg",
        "layer.videoLayers.replaceFootage",
        "measurementLog.",
        "type.font.library",
    ];
    if LEAVES.iter().any(|prefix| id.starts_with(prefix)) || id.contains(".presets.") {
        return false;
    }
    // Libraries and files by their verb: swatches.export, pattern.import, contours.save, …
    let verb = id.rsplit('.').next().unwrap_or(id);
    !(["export", "import", "install", "saveDefault", "loadDefault"].iter().any(|v| verb.starts_with(v))
        || matches!(verb, "save" | "load" | "reset" | "reload" | "remove"))
}

/// Whether command `id` only steps through the active document's history. A transaction made of
/// nothing but these is allowed: "undo twice, but only if nobody edited since I looked" is the
/// guarded way back. Mixed with edits they are refused, since a step that edits and a step that
/// takes edits back do not make one edit.
fn steps_through_history(id: &str) -> bool {
    matches!(
        id,
        "edit.undo" | "edit.redo" | "edit.toggleLastState" | "history.stepBackward" | "history.stepForward" | "history.selectState" | "history.selectSnapshot"
    )
}

/// The session's documents as they were before a transaction or a variant: what a failure
/// puts back.
pub(crate) struct Checkpoint {
    docs: Vec<DocState>,
    order: Vec<DocId>,
    active: Option<DocId>,
    journal: usize,
}

impl Checkpoint {
    pub(crate) fn take(s: &Session) -> Self {
        Self { docs: s.docs.clone(), order: s.open_document_order.clone(), active: s.active().map(|d| d.doc.id), journal: s.journal.len() }
    }

    /// Whether `now` is the document `then` was: same document, history, snapshots, revision.
    fn same(then: &DocState, now: &DocState) -> bool {
        std::sync::Arc::ptr_eq(&then.doc, &now.doc)
            && then.revision == now.revision
            && then.history.past_len() == now.history.past_len()
            && then.history.can_redo() == now.history.can_redo()
            && then.snapshots.len() == now.snapshots.len()
    }

    /// Why the session is not where a step may leave it, if it is not: only the documents in
    /// `own` may have changed, `extra` are the documents allowed to exist beyond the checkpoint's
    /// (a variant), and `active` must be the active document.
    pub(crate) fn strayed(&self, s: &Session, own: &[DocId], extra: &[DocId], active: DocId) -> Option<String> {
        if s.active().map(|d| d.doc.id) != Some(active) {
            return Some("it switched to another document".into());
        }
        if let Some(d) = s.docs.iter().find(|d| !extra.contains(&d.doc.id) && !self.docs.iter().any(|b| b.doc.id == d.doc.id)) {
            return Some(format!("it opened another document (\u{201c}{}\u{201d})", d.doc.name));
        }
        for before in &self.docs {
            let Some(now) = s.docs.iter().find(|d| d.doc.id == before.doc.id) else {
                return Some(format!("it closed \u{201c}{}\u{201d}", before.doc.name));
            };
            if !own.contains(&before.doc.id) && !Self::same(before, now) {
                return Some(format!("it changed another document (\u{201c}{}\u{201d})", before.doc.name));
            }
        }
        None
    }

    /// Put every document back as it was: close the ones opened since (`Session::close`, so
    /// their jobs and caches go too), restore the ones that changed, bring back the ones that
    /// were closed, in their old tab order with the old one active; forget the journal entries
    /// made since, so Last Filter, Transform Again and a recording action do not replay an edit
    /// that was taken back; and re-sync the tool colours with the restored channel target.
    ///
    /// A restored document's revision moves on (never back), so every view of it redraws.
    /// Session state outside the documents (the clipboard, tool and brush settings, files a
    /// step wrote) is not part of a checkpoint.
    pub(crate) fn restore(self, s: &mut Session) {
        while let Some(i) = s.docs.iter().position(|d| !self.docs.iter().any(|b| b.doc.id == d.doc.id)) {
            s.close(i);
        }
        for (at, before) in self.docs.iter().enumerate() {
            match s.docs.iter().position(|d| d.doc.id == before.doc.id) {
                Some(i) => {
                    let Some(now) = s.docs.get_mut(i) else { continue };
                    if Self::same(before, now) {
                        continue;
                    }
                    let revision = now.revision.max(before.revision).wrapping_add(1);
                    let clean = !before.is_dirty();
                    *now = before.clone();
                    now.revision = revision;
                    if clean {
                        now.saved_revision = revision;
                    }
                    now.last_damage = None;
                    now.coalesce = None;
                }
                None => {
                    // A step closed it. Its editing state comes back whole; the jobs and
                    // caches `Session::close` dropped are rebuilt on demand.
                    let mut back = before.clone();
                    back.revision = back.revision.wrapping_add(1);
                    if !before.is_dirty() {
                        back.saved_revision = back.revision;
                    }
                    back.last_damage = None;
                    back.coalesce = None;
                    s.docs.insert(at.min(s.docs.len()), back);
                }
            }
        }
        let place = |id: DocId| self.docs.iter().position(|b| b.doc.id == id).unwrap_or(usize::MAX);
        s.docs.sort_by_key(|d| place(d.doc.id));
        s.open_document_order = self.order;
        s.active = self.active.and_then(|id| s.docs.iter().position(|d| d.doc.id == id)).or(if s.docs.is_empty() { None } else { Some(0) });
        s.note_active_change();
        s.journal.truncate(self.journal);
        s.sync_tools_mask_target();
    }
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let Params { steps, expect_revision, expect_document } = serde_json::from_value(p.clone())
        .map_err(|e| bad(format!("{e}; pass {{\"steps\":[{{\"id\":…,\"params\":{{…}}}}],\"expectRevision\":n?,\"expectDocument\":id?}}")))?;
    if steps.is_empty() || steps.len() > MAX_STEPS {
        return Err(bad(format!("pass 1 to {MAX_STEPS} steps")));
    }
    let only_history = steps.iter().all(|step| steps_through_history(&step.id));
    for (i, step) in steps.iter().enumerate() {
        if steps_through_history(&step.id) && !only_history {
            return Err(bad(format!(
                "step {} (`{}`) steps through history: a transaction may consist of such steps only (to go back, send just the undo steps), not mix them with edits. Nothing was run",
                i + 1,
                step.id
            )));
        }
        if !only_history && !stays_on_document(&step.id) {
            return Err(bad(format!(
                "step {} (`{}`) is not something a transaction can take back (it opens, saves, closes or switches documents, or changes session-wide settings); run it on its own. Nothing was run",
                i + 1,
                step.id
            )));
        }
        if !(step.params.is_object() || step.params.is_null()) {
            return Err(bad(format!("step {} (`{}`): `params` must be an object. Nothing was run", i + 1, step.id)));
        }
    }
    if s.active_job().is_some() {
        return Err(bad("a background job is running on this document; wait for it or cancel it first. Nothing was run"));
    }
    let (doc, name, revision) = {
        let st = s.active().ok_or(EngineError::NoDocument)?;
        (st.doc.id, st.doc.name.clone(), st.revision)
    };
    if expect_document.is_some_and(|expected| expected != doc.0) {
        return Err(bad(format!(
            "another document is active now: \u{201c}{name}\u{201d} (id {}), not the one you were editing (id {}). Nothing was run. Inspect the session again",
            doc.0,
            expect_document.unwrap_or_default()
        )));
    }
    if let Some(expected) = expect_revision
        && expected != revision
    {
        return Err(bad(format!(
            "the document changed since you last looked: it is at revision {revision}, not {expected}. Nothing was run. Inspect it again and plan the edit against what is there now"
        )));
    }
    let checkpoint = Checkpoint::take(s);
    // The first step must not fold into the step before the transaction.
    if let Some(st) = s.active_mut() {
        st.coalesce = None;
    }
    let total = steps.len();
    let mut results = Vec::with_capacity(total);
    for (i, step) in steps.into_iter().enumerate() {
        let params = if step.params.is_null() { json!({}) } else { step.params };
        let failed = match s.execute(&step.id, params) {
            Ok(v) => match checkpoint.strayed(s, &[doc], &[], doc) {
                None => {
                    results.push(v);
                    None
                }
                Some(why) => Some(why),
            },
            Err(e) => Some(e.to_string()),
        };
        if let Some(why) = failed {
            checkpoint.restore(s);
            return Err(bad(format!(
                "step {} of {total} (`{}`) failed: {why}. Every step was rolled back; the documents are as they were before the transaction",
                i + 1,
                step.id
            )));
        }
    }
    let revision = s.active().map(|d| d.revision);
    Ok(json!({"completed": total, "results": results, "revision": revision, "document": doc.0}))
}

/// Transaction command specs.
pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Run as Transaction",
        menu: &[],
        shortcut: None,
        params: r##"{"steps":[{"id":command id,"params":json}] (1..256),"expectRevision":int?,"expectDocument":id?} → {completed,results:[json],revision,document} (runs the steps in order on the active document as one all-or-nothing edit: if a step fails, every earlier step is rolled back, every open document, its history and snapshots are exactly as before, and the error names the step; with expectRevision / expectDocument nothing runs unless the active document still has that revision / id (both from document.inspect or session.list), so an edit planned against an old state or another document is refused; each step still records its own history state, so Undo steps back through them; steps may not open, save, close, duplicate or switch documents, or change preferences, presets or other session-wide settings; edit.undo / edit.redo / history.stepBackward|stepForward|selectState|selectSnapshot are allowed only in a transaction made of nothing else: the guarded way to go back; tool and brush settings a step changes and the clipboard are not rolled back; steps run to the end without progress reports)"##,
        enabled: |s| s.active().map(|_| ()).ok_or_else(|| "no document open".into()),
        run,
        // The steps journal themselves.
        journal: false,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 40, "height": 30})).unwrap();
        s.execute("layer.newAdjustmentLayer.invert", json!({})).unwrap();
        s
    }

    /// Everything a rollback must put back.
    #[derive(Debug, PartialEq)]
    struct State {
        pixels: Vec<[f32; 4]>,
        layers: usize,
        history: Vec<String>,
        snapshots: usize,
        active_layer: Option<magicpic_doc::LayerId>,
        dirty: bool,
    }

    fn state(s: &Session) -> State {
        let st = s.active().unwrap();
        State {
            pixels: magicpic_compose::flatten(&st.doc).px.clone(),
            layers: st.doc.layers.len(),
            history: st.history.entries(),
            snapshots: st.snapshots.len(),
            active_layer: st.active_layer,
            dirty: st.is_dirty(),
        }
    }

    #[test]
    fn all_steps_apply_and_each_is_a_history_state() {
        let mut s = session();
        let history = s.active().unwrap().history.past_len();
        let revision = s.active().unwrap().revision;
        let v = s
            .execute(
                CMD,
                json!({"expectRevision": revision, "steps": [
                    {"id": "layer.newAdjustmentLayer.exposure", "params": {"exposure": 1}},
                    {"id": "layer.newAdjustmentLayer.posterize", "params": {"levels": 4}},
                ]}),
            )
            .unwrap();
        assert_eq!(v["completed"], 2);
        assert_eq!(v["results"].as_array().unwrap().len(), 2);
        assert_eq!(v["revision"], s.active().unwrap().revision);
        assert!(s.active().unwrap().revision > revision);
        assert_eq!(s.active().unwrap().doc.layers.len(), 4);
        assert_eq!(s.active().unwrap().history.past_len(), history + 2);
        // Undo steps back through the transaction one command at a time.
        assert!(s.undo());
        assert_eq!(s.active().unwrap().doc.layers.len(), 3);
    }

    #[test]
    fn a_failing_step_rolls_everything_back() {
        let mut s = session();
        s.execute("history.newSnapshot", json!({"name": "mine"})).unwrap();
        let before = state(&s);
        let revision = s.active().unwrap().revision;
        let e = s
            .execute(
                CMD,
                json!({"steps": [
                    {"id": "layer.newAdjustmentLayer.exposure", "params": {"exposure": 2}},
                    {"id": "layer.newAdjustmentLayer.posterize", "params": {"levels": 3}},
                    {"id": "filter.blur.gaussianBlur", "params": {"radius": "soft"}},
                    {"id": "layer.newAdjustmentLayer.invert", "params": {}},
                ]}),
            )
            .unwrap_err()
            .to_string();
        assert!(e.contains("step 3 of 4") && e.contains("filter.blur.gaussianBlur") && e.contains("rolled back"), "{e}");
        assert_eq!(state(&s), before, "the document is not as it was");
        assert!(s.active().unwrap().revision > revision, "views must redraw after a rollback");
        // Nothing of the failed transaction is left to redo.
        assert!(!s.redo());
        // And the document is still editable and undoable.
        s.execute("layer.newAdjustmentLayer.invert", json!({})).unwrap();
        assert!(s.undo());
        assert_eq!(state(&s), before);
    }

    #[test]
    fn a_clean_document_stays_clean_after_a_rollback() {
        let mut s = session();
        if let Some(st) = s.active_mut() {
            st.saved_revision = st.revision;
        }
        assert!(!s.active().unwrap().is_dirty());
        assert!(s.execute(CMD, json!({"steps": [{"id": "layer.newAdjustmentLayer.invert"}, {"id": "no.such.command"}]})).is_err());
        assert!(!s.active().unwrap().is_dirty(), "a rolled-back transaction marked the document as changed");
    }

    #[test]
    fn a_stale_revision_runs_nothing() {
        let mut s = session();
        let seen = s.active().unwrap().revision;
        // Somebody else edits the document after the caller looked.
        s.execute("layer.newAdjustmentLayer.posterize", json!({})).unwrap();
        let before = state(&s);
        let now = s.active().unwrap().revision;
        let e = s.execute(CMD, json!({"expectRevision": seen, "steps": [{"id": "layer.flattenImage", "params": {}}]})).unwrap_err().to_string();
        assert!(e.contains(&format!("revision {now}, not {seen}")) && e.contains("Nothing was run"), "{e}");
        assert_eq!(state(&s), before);
        assert_eq!(s.active().unwrap().revision, now, "a refused transaction moved the revision");
        // With the current revision it runs.
        s.execute(CMD, json!({"expectRevision": now, "steps": [{"id": "layer.flattenImage", "params": {}}]})).unwrap();
        assert_eq!(s.active().unwrap().doc.layers.len(), 1);
    }

    #[test]
    fn steps_cannot_leave_the_document() {
        let mut s = session();
        let before = state(&s);
        for id in [
            "file.new",
            "file.close",
            "image.duplicate",
            "history.newDocument",
            "window.nextDocument",
            "seeing.variant",
            "document.transaction",
            "actions.play",
        ] {
            let e = s.execute(CMD, json!({"steps": [{"id": "layer.newAdjustmentLayer.invert"}, {"id": id}]})).unwrap_err().to_string();
            assert!(e.contains("not something a transaction can take back") && e.contains("Nothing was run"), "{id}: {e}");
            assert_eq!(state(&s), before, "{id} ran part of the transaction");
        }
        assert_eq!(s.documents().len(), 1);
    }

    /// The whole session, for transactions that reach beyond their own document.
    fn session_state(s: &Session) -> Vec<(String, u64, Vec<String>, usize, bool)> {
        s.documents().iter().map(|d| (d.doc.name.clone(), d.doc.id.0, d.history.entries(), d.doc.layers.len(), d.is_dirty())).collect()
    }

    #[test]
    fn the_wrong_document_is_refused_even_at_the_same_revision() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 8, "height": 8, "name": "a"})).unwrap();
        let (a, revision) = (s.active().unwrap().doc.id.0, s.active().unwrap().revision);
        s.execute("file.new", json!({"width": 8, "height": 8, "name": "b"})).unwrap();
        // Both are fresh documents at the same revision: the revision alone cannot tell them apart.
        assert_eq!(s.active().unwrap().revision, revision);
        let step = json!([{"id": "layer.newAdjustmentLayer.invert"}]);
        let e = s.execute(CMD, json!({"steps": step, "expectRevision": revision, "expectDocument": a})).unwrap_err().to_string();
        assert!(e.contains("another document is active") && e.contains("Nothing was run"), "{e}");
        assert_eq!(s.active().unwrap().doc.layers.len(), 1);
        s.set_active(0);
        s.execute(CMD, json!({"steps": step, "expectRevision": revision, "expectDocument": a})).unwrap();
        assert_eq!(s.documents()[0].doc.layers.len(), 2);
        assert_eq!(s.documents()[1].doc.layers.len(), 1);
    }

    #[test]
    fn a_step_that_reaches_another_document_is_caught_and_everything_comes_back() {
        // None of these ids is on the refuse list by name alone in a way that matters here: the
        // check that runs after each step is what catches them.
        let mut s = session();
        s.execute("file.new", json!({"width": 40, "height": 30, "name": "other"})).unwrap();
        s.execute("layer.newAdjustmentLayer.posterize", json!({})).unwrap();
        s.set_active(0);
        let before = session_state(&s);
        let journal = s.journal.len();
        let checkpoint = Checkpoint::take(&s);
        let own = s.active().unwrap().doc.id;
        // An edit to the other document.
        s.set_active(1);
        s.execute("layer.newAdjustmentLayer.invert", json!({})).unwrap();
        assert!(checkpoint.strayed(&s, &[own], &[], own).is_some_and(|why| why.contains("switched")));
        s.set_active(0);
        assert!(checkpoint.strayed(&s, &[own], &[], own).is_some_and(|why| why.contains("changed another document")));
        // A new document, and the other one closed.
        s.execute("file.new", json!({"width": 4, "height": 4, "name": "stray"})).unwrap();
        s.set_active(0);
        assert!(checkpoint.strayed(&s, &[own], &[], own).is_some_and(|why| why.contains("opened another document")));
        s.close(1);
        s.set_active(0);
        assert!(checkpoint.strayed(&s, &[own], &[], own).is_some());
        s.execute("layer.newAdjustmentLayer.vibrance", json!({})).unwrap();
        checkpoint.restore(&mut s);
        assert_eq!(session_state(&s), before, "the session is not as it was");
        assert_eq!(s.active_index(), Some(0));
        assert_eq!(s.journal.len(), journal, "the journal kept commands that were taken back");
        // Both documents still edit and undo.
        s.set_active(1);
        s.execute("layer.newAdjustmentLayer.invert", json!({})).unwrap();
        assert!(s.undo());
        assert_eq!(session_state(&s), before);
    }

    #[test]
    fn session_wide_and_history_commands_are_refused_up_front() {
        let mut s = session();
        let before = state(&s);
        for id in [
            "edit.purge.histories",
            "edit.purge.all",
            "layer.copyToDocument",
            "channel.split",
            "channel.merge",
            "document.activate",
            "document.move",
            "history.newSnapshot",
            "history.newDocument",
            "history.clear",
            "prefs.reset",
            "edit.preferences.general",
            "swatches.export",
            "swatches.reset",
            "pattern.import",
            "brush.presets.delete",
            "plugin.install",
            "layer.exportAs",
            "layer.layerStyle.contours.save",
            "type.saveDefaultTypeStyles",
        ] {
            assert!(!stays_on_document(id), "{id} is allowed in a transaction");
            assert!(s.execute(CMD, json!({"steps": [{"id": id}]})).is_err(), "{id}");
        }
        assert_eq!(state(&s), before);
        for id in [
            "layer.newAdjustmentLayer.curves",
            "filter.blur.gaussianBlur",
            "image.crop",
            "select.rect",
            "layer.smartObjects.resetTransform",
            "seeing.note",
            "notes.add",
            "cloneSource.resetTransform",
        ] {
            assert!(stays_on_document(id), "{id} is refused in a transaction");
        }
    }

    #[test]
    fn undo_steps_alone_are_a_guarded_way_back() {
        let mut s = session();
        s.execute("layer.newAdjustmentLayer.exposure", json!({"exposure": 1})).unwrap();
        s.execute("layer.newAdjustmentLayer.posterize", json!({})).unwrap();
        let (id, revision) = (s.active().unwrap().doc.id.0, s.active().unwrap().revision);
        assert_eq!(s.active().unwrap().doc.layers.len(), 4);
        // Mixed with an edit they are refused, and nothing runs.
        let e = s.execute(CMD, json!({"steps": [{"id": "edit.undo"}, {"id": "layer.newAdjustmentLayer.invert"}]})).unwrap_err().to_string();
        assert!(e.contains("steps through history") && e.contains("Nothing was run"), "{e}");
        assert_eq!(s.active().unwrap().doc.layers.len(), 4);
        // A stale guard stops them like any other transaction.
        assert!(s.execute(CMD, json!({"steps": [{"id": "edit.undo"}], "expectRevision": revision - 1})).is_err());
        assert_eq!(s.active().unwrap().doc.layers.len(), 4);
        // Two undo steps, guarded: back before both layers, and Redo brings them back.
        let v = s.execute(CMD, json!({"steps": [{"id": "edit.undo"}, {"id": "edit.undo"}], "expectRevision": revision, "expectDocument": id})).unwrap();
        assert_eq!(v["completed"], 2);
        assert_eq!(s.active().unwrap().doc.layers.len(), 2);
        s.execute(CMD, json!({"steps": [{"id": "edit.redo"}]})).unwrap();
        assert_eq!(s.active().unwrap().doc.layers.len(), 3);
    }

    #[test]
    fn a_rollback_forgets_the_journal_entries_and_keeps_the_floating_piece() {
        let mut s = session();
        s.execute("layer.select", json!({"index": 0})).ok();
        let journal = s.journal.len();
        let e = s.execute(CMD, json!({"steps": [{"id": "layer.newAdjustmentLayer.exposure", "params": {"exposure": 1}}, {"id": "no.such.command"}]}));
        assert!(e.is_err());
        assert_eq!(s.journal.len(), journal, "a rolled-back step stayed in the journal");
        // A successful transaction journals its steps (so an action being recorded gets them).
        s.execute(CMD, json!({"steps": [{"id": "layer.newAdjustmentLayer.exposure", "params": {"exposure": 1}}]})).unwrap();
        assert_eq!(s.journal.len(), journal + 1);
    }

    #[test]
    fn bad_params_are_errors() {
        let mut s = session();
        let before = state(&s);
        let many: Vec<Value> = (0..MAX_STEPS + 1).map(|_| json!({"id": "layer.newAdjustmentLayer.invert"})).collect();
        for p in [
            json!({}),
            json!({"steps": []}),
            json!({"steps": many}),
            json!({"steps": "all"}),
            json!({"steps": [{"params": {}}]}),
            json!({"steps": [{"id": 5}]}),
            json!({"steps": [{"id": "layer.newAdjustmentLayer.invert", "params": [1]}]}),
            json!({"steps": [{"id": "layer.newAdjustmentLayer.invert", "wait": false}]}),
            json!({"steps": [{"id": "layer.newAdjustmentLayer.invert"}], "expectRevision": -1}),
            json!({"steps": [{"id": "layer.newAdjustmentLayer.invert"}], "expectRevision": "latest"}),
            json!({"steps": [{"id": "layer.newAdjustmentLayer.invert"}], "atomic": true}),
        ] {
            assert!(s.execute(CMD, p.clone()).is_err(), "{p}");
            assert_eq!(state(&s), before, "{p}");
        }
        assert!(Session::new().execute(CMD, json!({"steps": [{"id": "layer.newAdjustmentLayer.invert"}]})).is_err());
    }
}
