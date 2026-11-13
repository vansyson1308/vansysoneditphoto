//! Which document a tool works on, in either backend: resolving a `document` reference, bringing
//! it to the front (the app's tab, or the headless session's active document), and the
//! `{document, index, name, revision, dirty}` summary added to command results.
use super::{AutomationError, Backend, DocRef, DocSelector, MagicpicMcp, Value, json};

/// The compact session: `{active, documents: [{index, id, name, path, width, height, dirty, revision}]}`.
fn compact(session: &Value) -> Value {
    json!({"active": session.get("active").cloned().unwrap_or(Value::Null), "documents": session.get("documents").cloned().unwrap_or(json!([]))})
}

/// The document `r` names in `session` (as `session.inspect` reports it): `(index, id)`.
pub(super) fn find(session: &Value, r: DocRef) -> Result<(usize, u64), AutomationError> {
    let docs = session.get("documents").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let entry = |d: &Value| Some((usize::try_from(d.get("index")?.as_u64()?).ok()?, d.get("id")?.as_u64()?));
    let listed = || docs.iter().filter_map(entry).map(|(i, id)| format!("{i} (id {id})")).collect::<Vec<_>>().join(", ");
    let none = |what: String| {
        AutomationError::BadRequest(format!("no open document {what}; open documents: {}", if docs.is_empty() { "none".into() } else { listed() }))
    };
    match r {
        DocRef::Index(i) | DocRef::Selector(DocSelector { index: Some(i), id: None }) => {
            docs.iter().filter_map(entry).find(|(index, _)| *index == i).ok_or_else(|| none(format!("at index {i}")))
        }
        DocRef::Selector(DocSelector { index: None, id: Some(id) }) => {
            docs.iter().filter_map(entry).find(|(_, d)| *d == id).ok_or_else(|| none(format!("with id {id}")))
        }
        DocRef::Selector(DocSelector { index: Some(i), id: Some(id) }) => match docs.iter().filter_map(entry).find(|(_, d)| *d == id) {
            Some((index, _)) if index == i => Ok((i, id)),
            Some((index, _)) => Err(AutomationError::BadRequest(format!("document {id} is at index {index}, not {i}"))),
            None => Err(none(format!("with id {id}"))),
        },
        DocRef::Selector(DocSelector { index: None, id: None }) => Err(AutomationError::BadRequest("`document` needs `index` or `id`".into())),
    }
}

/// `index` and `document` name the same thing; at most one may be given.
pub(super) fn one_ref(index: Option<usize>, document: Option<DocRef>) -> Result<Option<DocRef>, AutomationError> {
    match (index, document) {
        (Some(_), Some(_)) => Err(AutomationError::BadRequest("give `index` or `document`, not both".into())),
        (Some(i), None) => Ok(Some(DocRef::Index(i))),
        (None, d) => Ok(d),
    }
}

impl MagicpicMcp {
    /// The engine's `session.inspect`, in either backend (a read).
    pub(super) async fn session_state(&self) -> Result<Value, AutomationError> {
        if let Some(r) = self.headless_op(|h| Ok(h.session_list())).await {
            return r;
        }
        match self.bridge_client() {
            Some(b) => b.call_read("engine.execute", json!({"command": "session.inspect"})).await,
            None => Err(AutomationError::Other("internal error: no headless session or app bridge".into())),
        }
    }

    /// `session_list` without `verbose`: the documents and the active index only.
    pub(super) async fn session_compact(&self) -> Result<Value, AutomationError> {
        self.session_state().await.map(|s| compact(&s))
    }

    /// `(index, id)` of the document `r` names.
    pub(super) async fn resolve(&self, r: DocRef) -> Result<(usize, u64), AutomationError> {
        find(&self.session_state().await?, r)
    }

    /// Make the document `r` names the active one: in the app its tab comes to the front, as
    /// Window › <document> does. Returns its `(index, id)`.
    pub(super) async fn bring_to_front(&self, r: DocRef) -> Result<(usize, u64), AutomationError> {
        let session = self.session_state().await?;
        let (index, id) = find(&session, r)?;
        if session.get("active").and_then(Value::as_u64) == Some(index as u64) {
            return Ok((index, id));
        }
        match &*self.backend {
            Backend::Headless(_) => {
                if let Some(r) = self.headless_op(move |h| h.select(index)).await {
                    r?;
                }
            }
            Backend::Bridge(b) => {
                b.call("ui.menu.invoke", json!({"id": format!("window.document.{index}")})).await?;
            }
        }
        Ok((index, id))
    }

    /// `{document, index, name, revision, dirty}` of the active document, or null without one.
    pub(super) async fn active_summary(&self) -> Value {
        let Ok(session) = self.session_state().await else { return Value::Null };
        let Some(active) = session.get("active").and_then(Value::as_u64) else { return Value::Null };
        let doc = session.get("documents").and_then(Value::as_array).and_then(|d| d.iter().find(|d| d.get("index").and_then(Value::as_u64) == Some(active)));
        match doc {
            Some(d) => json!({"document": d.get("id"), "index": active, "name": d.get("name"), "revision": d.get("revision"), "dirty": d.get("dirty")}),
            None => Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Value {
        json!({"active": 1, "documents": [{"index": 0, "id": 7, "name": "a"}, {"index": 1, "id": 9, "name": "b"}], "foreground": [0, 0, 0, 1]})
    }

    #[test]
    fn references_resolve_by_index_or_id() {
        let s = session();
        assert_eq!(find(&s, DocRef::Index(0)).unwrap(), (0, 7));
        assert_eq!(find(&s, DocRef::Selector(DocSelector { index: None, id: Some(9) })).unwrap(), (1, 9));
        assert_eq!(find(&s, DocRef::Selector(DocSelector { index: Some(1), id: Some(9) })).unwrap(), (1, 9));
        let e = find(&s, DocRef::Index(5)).unwrap_err().to_string();
        assert!(e.contains("at index 5") && e.contains("0 (id 7), 1 (id 9)"), "{e}");
        assert!(find(&s, DocRef::Selector(DocSelector { index: Some(0), id: Some(9) })).unwrap_err().to_string().contains("at index 1, not 0"));
        assert!(find(&s, DocRef::Selector(DocSelector { index: None, id: None })).is_err());
        assert!(find(&json!({"documents": []}), DocRef::Index(0)).unwrap_err().to_string().contains("none"));
    }

    #[test]
    fn references_parse_as_a_number_or_an_object() {
        assert_eq!(serde_json::from_value::<DocRef>(json!(2)).unwrap(), DocRef::Index(2));
        assert_eq!(serde_json::from_value::<DocRef>(json!({"id": 4})).unwrap(), DocRef::Selector(DocSelector { index: None, id: Some(4) }));
        for bad in [json!(-1), json!("2"), json!({"name": "x"}), json!(null)] {
            assert!(serde_json::from_value::<DocRef>(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_compact_session_keeps_only_the_documents() {
        assert_eq!(compact(&session()), json!({"active": 1, "documents": session()["documents"]}));
        assert!(one_ref(Some(1), Some(DocRef::Index(1))).is_err());
        assert_eq!(one_ref(Some(1), None).unwrap(), Some(DocRef::Index(1)));
    }
}
