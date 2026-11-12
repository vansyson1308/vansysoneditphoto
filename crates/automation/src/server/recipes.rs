//! `actions_play`: one recipe (a saved Action, or inline steps as in `command_batch`) applied to
//! several open documents in one call, with per-document overrides of step params (a star's
//! position, a fade line). Each document is brought to the front, then edited; with `atomic` (the
//! default) its steps are one transaction, so a failing document is rolled back and the others go on.
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{AutomationError, DocRef, MagicpicMcp, Value, Waiter, attach_memory_error_details, fail, json, ok_json};
use crate::budgets::BatchReplyBudget;
use crate::security::MAX_BATCH_STEPS;

/// Most documents one call edits.
pub(super) const MAX_DOCUMENTS: usize = 64;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecipeStep {
    /// Command id, e.g. `filter.render.starburst`.
    pub id: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Override {
    /// Which of `documents` this applies to (index or `{"id": n}`).
    pub document: DocRef,
    /// The 0-based step in `steps` whose params change.
    pub step: usize,
    /// Merged over that step's params (top-level keys replace).
    pub params: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlayParams {
    /// A saved Action by name (Actions panel, `actions.list`). Give this or `steps`.
    #[serde(default)]
    pub action: Option<String>,
    /// Or the recipe itself: `[{"id": …, "params": {…}}, …]`, as in `command_batch`.
    #[serde(default)]
    pub steps: Option<Vec<RecipeStep>>,
    /// The documents to apply it to, in order (index or `{"id": n}`), 1 to 64.
    pub documents: Vec<DocRef>,
    /// Per-document changes to `steps` params: `[{"document": 1, "step": 0, "params": {"points": […]}}]`.
    #[serde(default)]
    pub overrides: Option<Vec<Override>>,
    /// Each document all or nothing (default true): a failing step rolls that document back.
    #[serde(default)]
    pub atomic: Option<bool>,
    /// Stop at the first document that fails (default false: the others still run).
    #[serde(default)]
    pub stop_on_error: Option<bool>,
}

/// `base` with `patch`'s top-level keys set.
fn merged(base: Option<&Value>, patch: &Value) -> Value {
    let mut m = base.and_then(Value::as_object).cloned().unwrap_or_default();
    if let Some(p) = patch.as_object() {
        for (k, v) in p {
            m.insert(k.clone(), v.clone());
        }
    }
    Value::Object(m)
}

impl MagicpicMcp {
    pub(super) async fn play(&self, p: PlayParams, mut waiter: Waiter) -> CallToolResult {
        if p.documents.is_empty() || p.documents.len() > MAX_DOCUMENTS {
            return fail(format!("`documents` must list 1 to {MAX_DOCUMENTS} documents"));
        }
        let steps = match (&p.action, &p.steps) {
            (Some(_), Some(_)) | (None, None) => return fail("give `action` (a saved Action's name) or `steps`, not both"),
            (Some(_), None) if p.overrides.as_ref().is_some_and(|o| !o.is_empty()) => {
                return fail("`overrides` change `steps` params; to vary a saved Action per document, pass its steps as `steps`");
            }
            (_, Some(s)) if s.is_empty() || s.len() > MAX_BATCH_STEPS => return fail(format!("`steps` must hold 1 to {MAX_BATCH_STEPS} steps")),
            (_, s) => s.as_deref(),
        };
        // Every reference resolved first: a typo fails before any document is edited.
        let mut targets = Vec::with_capacity(p.documents.len());
        for r in &p.documents {
            match self.resolve(*r).await {
                Ok(t) => targets.push(t),
                Err(e) => return fail(e),
            }
        }
        let mut overrides: Vec<(u64, usize, &Value)> = Vec::new();
        for o in p.overrides.iter().flatten() {
            let id = match self.resolve(o.document).await {
                Ok((_, id)) => id,
                Err(e) => return fail(e),
            };
            if !targets.iter().any(|(_, t)| *t == id) {
                return fail(format!("an override names document {id}, which is not in `documents`"));
            }
            if steps.is_none_or(|s| o.step >= s.len()) {
                return fail(format!("an override names step {}, but the recipe has {} steps", o.step, steps.map_or(0, <[RecipeStep]>::len)));
            }
            if !o.params.is_object() {
                return fail("an override's `params` must be an object");
            }
            overrides.push((id, o.step, &o.params));
        }
        let atomic = p.atomic.unwrap_or(true);
        let stop = p.stop_on_error.unwrap_or(false);
        let mut budget = BatchReplyBudget::escaped();
        let mut rows = Vec::new();
        let (mut completed, mut failed) = (0usize, 0usize);
        let total = targets.len();
        for (n, (_, id)) in targets.into_iter().enumerate() {
            if waiter.is_cancelled() {
                break;
            }
            let outcome = self.play_one(id, p.action.as_deref(), steps, &overrides, atomic, &mut waiter, (n, total)).await;
            let summary = self.active_summary().await;
            let row = match outcome {
                Ok(result) => {
                    completed += 1;
                    json!({"document": id, "index": summary.get("index"), "name": summary.get("name"), "revision": summary.get("revision"), "ok": true, "result": result})
                }
                Err(e) => {
                    failed += 1;
                    let mut row = json!({"document": id, "ok": false, "error": e.to_string()});
                    attach_memory_error_details(&mut row, &e);
                    row
                }
            };
            let was_error = row["ok"] == false;
            if let Err(e) = budget.charge(&row) {
                rows.push(json!({"document": id, "ok": false, "error": format!("reply budget exceeded; this document may have been edited: {e}")}));
                break;
            }
            rows.push(row);
            waiter.report((n + 1) as f64, total as f64, "").await;
            if was_error && stop {
                break;
            }
        }
        ok_json(&json!({"completed": completed, "failed": failed, "atomic": atomic, "documents": rows}))
    }

    #[allow(clippy::too_many_arguments)]
    async fn play_one(
        &self,
        id: u64,
        action: Option<&str>,
        steps: Option<&[RecipeStep]>,
        overrides: &[(u64, usize, &Value)],
        atomic: bool,
        waiter: &mut Waiter,
        at: (usize, usize),
    ) -> Result<Value, AutomationError> {
        self.bring_to_front(DocRef::Selector(super::DocSelector { index: None, id: Some(id) })).await?;
        let Some(steps) = steps else {
            // A saved Action is already one history step (`Action: <name>`).
            return self.step("actions.play".into(), json!({"action": action}), true, waiter, at).await;
        };
        let concrete: Vec<Value> = steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let params = overrides
                    .iter()
                    .filter(|(d, step, _)| *d == id && *step == i)
                    .fold(s.params.clone().unwrap_or_else(|| json!({})), |base, (_, _, patch)| merged(Some(&base), patch));
                json!({"id": s.id, "params": params})
            })
            .collect();
        if atomic {
            return self.step("document.transaction".into(), json!({"steps": concrete, "expectDocument": id}), true, waiter, at).await;
        }
        let mut results = Vec::with_capacity(concrete.len());
        for s in concrete {
            let cmd = s["id"].as_str().unwrap_or_default().to_string();
            results.push(self.step(cmd, s["params"].clone(), true, waiter, at).await?);
        }
        Ok(json!({"completed": results.len(), "results": results}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_replace_top_level_keys_only() {
        let base = json!({"rays": 4, "points": [{"x": 1, "y": 2}], "glow": 0.5});
        assert_eq!(merged(Some(&base), &json!({"points": [{"x": 9, "y": 9}]})), json!({"rays": 4, "points": [{"x": 9, "y": 9}], "glow": 0.5}));
        assert_eq!(merged(None, &json!({"a": 1})), json!({"a": 1}));
    }
}
