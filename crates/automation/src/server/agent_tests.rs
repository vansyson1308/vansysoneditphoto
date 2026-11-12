//! The tool changes from the agent setup report (P1): `document` targeting in both backends,
//! bridge-mode export options, compact replies, the histogram and solo-layer previews. Driven
//! over JSON-RPC as a client does.
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::{Backend, MagicpicMcp};
use crate::BridgeClient;

/// A JSON-RPC session with a server.
struct Client {
    input: tokio::io::DuplexStream,
    lines: tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
    next: u64,
}

impl Client {
    async fn start(server: MagicpicMcp) -> Client {
        let (input, server_in) = tokio::io::duplex(1 << 22);
        let (server_out, output) = tokio::io::duplex(1 << 22);
        tokio::spawn(server.serve_io(server_in, server_out));
        let mut c = Client { input, lines: BufReader::new(output).lines(), next: 1 };
        c.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "agent-tests", "version": "1"}})).await;
        c.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
        c
    }

    async fn send(&mut self, v: Value) {
        self.input.write_all(format!("{v}\n").as_bytes()).await.unwrap();
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await;
        loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(30), self.lines.next_line()).await.expect("a reply").unwrap().unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == id {
                return v;
            }
        }
    }

    /// A tool call: `(is_error, [text blocks parsed as JSON where they are JSON])`.
    async fn call(&mut self, name: &str, args: Value) -> (bool, Vec<Value>) {
        let reply = self.request("tools/call", json!({"name": name, "arguments": args})).await;
        assert!(reply.get("error").is_none(), "{name}: {reply}");
        let result = &reply["result"];
        let blocks = result["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| match c["type"].as_str() {
                Some("text") => serde_json::from_str(c["text"].as_str().unwrap()).unwrap_or_else(|_| c["text"].clone()),
                _ => json!({"type": c["type"], "mimeType": c["mimeType"]}),
            })
            .collect();
        (result["isError"] == true, blocks)
    }

    async fn ok(&mut self, name: &str, args: Value) -> Vec<Value> {
        let (error, blocks) = self.call(name, args.clone()).await;
        assert!(!error, "{name} {args}: {blocks:?}");
        blocks
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn headless_document_targeting_and_compact_replies() {
    let mut c = Client::start(MagicpicMcp::headless()).await;
    c.ok("doc_new", json!({"width": 16, "height": 16, "name": "first"})).await;
    c.ok("doc_new", json!({"width": 24, "height": 24, "name": "second"})).await;
    let session = c.ok("session_list", json!({})).await.remove(0);
    assert_eq!(session.as_object().unwrap().keys().collect::<Vec<_>>(), ["active", "documents"], "compact: {session}");
    assert_eq!(session["active"], 1);
    let first = session["documents"][0]["id"].as_u64().unwrap();
    assert!(c.ok("session_list", json!({"verbose": true})).await[0].get("foreground").is_some());

    // The second tab is active, but `document` sends the command to the first.
    let blocks = c.ok("command_run", json!({"id": "layer.new.layer", "params": {"name": "Ink"}, "document": {"id": first}})).await;
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    assert_eq!(blocks[1]["document"], first, "the summary names the edited document: {blocks:?}");
    assert_eq!(blocks[1]["index"], 0);
    let first_doc = c.ok("doc_inspect", json!({"document": 0})).await.remove(0);
    assert!(first_doc["layers"].to_string().contains("Ink"), "{first_doc}");
    let second_doc = c.ok("doc_inspect", json!({"document": 1})).await.remove(0);
    assert!(!second_doc["layers"].to_string().contains("Ink"));

    // A batch on a document, and its reply names it with the revision.
    let batch = c.ok("command_batch", json!({"steps": [{"id": "layer.new.layer", "params": {"name": "A"}}], "document": 1})).await.remove(0);
    assert_eq!((batch["index"].as_u64(), batch["completed"].as_u64()), (Some(1), Some(1)), "{batch}");
    assert!(batch["revision"].as_u64().is_some());
    let atomic = c.ok("command_batch", json!({"steps": [{"id": "layer.new.layer"}], "document": {"id": first}, "atomic": true})).await.remove(0);
    assert_eq!((atomic["outcome"].as_str(), atomic["document"].as_u64(), atomic["index"].as_u64()), (Some("applied"), Some(first), Some(0)), "{atomic}");

    // doc_select by id; bad references are errors that change nothing.
    let selected = c.ok("doc_select", json!({"id": first})).await.remove(0);
    assert_eq!(selected["active"], 0);
    for (tool, args) in [
        ("command_run", json!({"id": "layer.new.layer", "document": 9})),
        ("command_run", json!({"id": "layer.new.layer", "document": {"id": 123_456}})),
        ("command_run", json!({"id": "filter.blur.gaussianBlur", "params": {"radius": 1}, "document": 0, "wait": false})),
        ("doc_inspect", json!({"index": 0, "document": 0})),
        ("doc_select", json!({})),
    ] {
        let (error, blocks) = c.call(tool, args.clone()).await;
        assert!(error, "{tool} {args}: {blocks:?}");
    }

    // command_list: brief rows, paged.
    let page = c.ok("command_list", json!({"brief": true, "filter": "layer.new", "limit": 2})).await.remove(0);
    assert_eq!(page["offset"], 0);
    assert_eq!(page["commands"].as_array().unwrap().len(), 2);
    assert_eq!(page["commands"][0].as_object().unwrap().len(), 3, "{page}");
    let next = page["next"].as_u64().unwrap();
    let rest = c.ok("command_list", json!({"brief": true, "filter": "layer.new", "offset": next})).await.remove(0);
    assert_eq!(rest["total"], page["total"]);
    assert!(c.ok("command_list", json!({"filter": "layer.new.layer"})).await[0].is_array(), "no paging params: the bare array, as before");
}

#[tokio::test(flavor = "multi_thread")]
async fn headless_histogram_and_solo_preview_change_nothing() {
    let mut c = Client::start(MagicpicMcp::headless()).await;
    c.ok("doc_new", json!({"width": 32, "height": 32, "background": "#204060"})).await;
    c.ok("command_run", json!({"id": "layer.new.layer", "params": {"name": "Stars"}})).await;
    let doc = c.ok("doc_inspect", json!({})).await.remove(0);
    let stars = doc["activeLayer"].as_u64().or_else(|| doc["layers"][0]["id"].as_u64()).unwrap();
    let revision = doc["revision"].clone();

    let h = c.ok("doc_histogram", json!({"channel": "blue"})).await.remove(0);
    assert_eq!(h["pixels"], 32 * 32);
    assert_eq!(h["channels"]["blue"]["percentiles"]["p50"], 0x60);
    let alone = c.ok("doc_histogram", json!({"layer": stars, "channel": "alpha"})).await.remove(0);
    assert_eq!(alone["transparent"], 32 * 32, "the new layer is empty: {alone}");

    let blocks = c.ok("render_preview", json!({"layer": stars, "max_side": 32})).await;
    assert_eq!(blocks[0]["type"], "image");
    assert_eq!(blocks[1]["layer"]["solo"], true, "{blocks:?}");
    let (error, _) = c.call("render_preview", json!({"solo": false})).await;
    assert!(error, "`solo` without `layer`");
    let after = c.ok("doc_inspect", json!({})).await.remove(0);
    assert_eq!(after["revision"], revision, "looking changed the document");
    assert_eq!(after["history"], doc["history"]);
}

/// A fake app for bridge tests: two documents, tab switching through `ui.menu.invoke`, and a log
/// of every control call it received.
fn fake_app(listener: tokio::net::TcpListener, token: &'static str, log: Arc<Mutex<Vec<Value>>>) {
    let active = Arc::new(Mutex::new(1u64));
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let (log, active) = (log.clone(), active.clone());
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut lines = BufReader::new(read).lines();
                let mut handshake = crate::security::ServerHandshake::new();
                loop {
                    let Ok(Some(line)) = lines.next_line().await else { return };
                    let (reply, step) = handshake.reply(&line, token);
                    write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
                    match step {
                        crate::security::AuthStep::Authenticated => break,
                        crate::security::AuthStep::Challenged => {}
                        crate::security::AuthStep::Rejected => return,
                    }
                }
                while let Ok(Some(line)) = lines.next_line().await {
                    let req: Value = serde_json::from_str(&line).unwrap();
                    log.lock().unwrap().push(json!({"method": req["method"], "params": req["params"]}));
                    let now = *active.lock().unwrap();
                    let session = json!({"active": now, "documents": [
                        {"index": 0, "id": 11, "name": "a.jpg", "dirty": false, "revision": 3},
                        {"index": 1, "id": 12, "name": "b.jpg", "dirty": true, "revision": 8},
                    ], "foreground": [0, 0, 0, 1]});
                    let result = match (req["method"].as_str().unwrap(), req["params"]["command"].as_str()) {
                        ("engine.execute", Some("session.inspect")) => session,
                        ("engine.execute", _) => json!({"ran": req["params"]["command"]}),
                        ("ui.menu.invoke", _) => {
                            let i = req["params"]["id"].as_str().unwrap().rsplit('.').next().unwrap().parse::<u64>().unwrap();
                            *active.lock().unwrap() = i;
                            json!({"document": i})
                        }
                        ("app.save", _) => json!({"path": req["params"]["path"], "warnings": []}),
                        ("app.open", _) => {
                            *active.lock().unwrap() = 1;
                            json!({"path": req["params"]["path"], "name": "b.jpg", "warnings": []})
                        }
                        ("ui.inspect", _) => json!({"tool": "move", "session": session}),
                        (m, _) => json!({"unexpected": m}),
                    };
                    write.write_all(format!("{}\n", json!({"id": req["id"], "ok": true, "result": result})).as_bytes()).await.unwrap();
                }
            });
        }
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn bridge_mode_targets_documents_and_takes_export_options() {
    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let log = Arc::new(Mutex::new(Vec::new()));
    fake_app(listener, TOKEN, log.clone());
    let server = MagicpicMcp::with_backend(Backend::Bridge(Arc::new(BridgeClient::new(addr, TOKEN).unwrap())));
    let mut c = Client::start(server).await;
    let methods = |log: &Arc<Mutex<Vec<Value>>>| -> Vec<String> {
        log.lock()
            .unwrap()
            .drain(..)
            .map(|v| format!("{} {}", v["method"].as_str().unwrap(), v["params"]["command"].as_str().or(v["params"]["id"].as_str()).unwrap_or("")))
            .collect()
    };

    // Compact session_list asks the engine, not the whole UI.
    let session = c.ok("session_list", json!({})).await.remove(0);
    assert_eq!(session["documents"].as_array().unwrap().len(), 2);
    assert!(session.get("foreground").is_none());
    assert_eq!(methods(&log), ["engine.execute session.inspect"]);
    c.ok("session_list", json!({"verbose": true})).await;
    assert_eq!(methods(&log), ["ui.inspect "]);

    // doc_open reports the index and id of what it opened.
    let opened = c.ok("doc_open", json!({"path": "b.jpg"})).await.remove(0);
    assert_eq!((opened["index"].as_u64(), opened["id"].as_u64()), (Some(1), Some(12)), "{opened}");
    methods(&log);

    // `document` brings the tab to the front, then runs the command.
    let blocks = c.ok("command_run", json!({"id": "image.adjustments.invert", "document": {"id": 11}})).await;
    assert_eq!(
        methods(&log),
        ["engine.execute session.inspect", "ui.menu.invoke window.document.0", "engine.execute image.adjustments.invert", "engine.execute session.inspect"]
    );
    assert_eq!((blocks[1]["document"].as_u64(), blocks[1]["index"].as_u64(), blocks[1]["revision"].as_u64()), (Some(11), Some(0), Some(3)), "{blocks:?}");

    // doc_select works in bridge mode now.
    let selected = c.ok("doc_select", json!({"index": 1})).await.remove(0);
    assert_eq!(selected["active"], 1);
    assert!(methods(&log).contains(&"ui.menu.invoke window.document.1".to_string()));

    // Export options for one save, and the document it saves.
    c.ok("doc_export", json!({"path": "out/b.jpg", "quality": 95, "document": 0})).await;
    let saved = log.lock().unwrap().iter().find(|v| v["method"] == "app.save").cloned().unwrap();
    assert_eq!(saved["params"], json!({"path": "out/b.jpg", "exportQuality": 95}));
    assert!(methods(&log).contains(&"ui.menu.invoke window.document.0".to_string()));
    for args in [json!({"path": "out/b.jpg", "format": "png"}), json!({"path": "out/b.jpg", "dpxBits": 10})] {
        let (error, blocks) = c.call("doc_export", args.clone()).await;
        assert!(error, "{args}: {blocks:?}");
    }
    assert!(methods(&log).iter().all(|m| !m.starts_with("app.save")), "a refused export saves nothing");
    c.ok("doc_export", json!({"path": "out/b.jpeg", "format": "jpg"})).await;
}

/// `actions_play` (agent report P2.17): one recipe on several documents, each with its own
/// star position, in one call.
#[tokio::test(flavor = "multi_thread")]
async fn one_recipe_runs_on_several_documents_with_overrides() {
    let mut c = Client::start(MagicpicMcp::headless()).await;
    for name in ["one", "two"] {
        c.ok("doc_new", json!({"width": 64, "height": 48, "name": name, "background": "black"})).await;
    }
    let session = c.ok("session_list", json!({})).await.remove(0);
    let (a, b) = (session["documents"][0]["id"].as_u64().unwrap(), session["documents"][1]["id"].as_u64().unwrap());
    let steps = json!([
        {"id": "filter.render.starburst", "params": {"points": [{"x": 10, "y": 10, "size": 2}], "rays": 4}},
        {"id": "layer.new.layer", "params": {"name": "Notes"}},
    ]);
    let reply = c
        .ok("actions_play", json!({"steps": steps, "documents": [{"id": a}, 1], "overrides": [{"document": {"id": b}, "step": 0, "params": {"points": [{"x": 50, "y": 38, "size": 2}]}}]}))
        .await
        .remove(0);
    assert_eq!((reply["completed"].as_u64(), reply["failed"].as_u64()), (Some(2), Some(0)), "{reply}");
    assert_eq!(reply["documents"][0]["document"], a);
    assert_eq!(reply["documents"][1]["document"], b);
    let bounds = |r: &Value, i: usize| r["documents"][i]["result"]["results"][0]["bounds"].clone();
    assert!(bounds(&reply, 0)[0].as_i64().unwrap() < 20, "{reply}");
    assert!(bounds(&reply, 1)[0].as_i64().unwrap() > 30, "the override moved the second document's star: {reply}");
    for i in [0, 1] {
        let doc = c.ok("doc_inspect", json!({"document": i})).await.remove(0);
        let names = doc["layers"].to_string();
        assert!(names.contains("Starburst") && names.contains("Notes"), "document {i}: {names}");
    }

    // A document that fails is rolled back; the other still runs.
    let reply = c
        .ok("actions_play", json!({"steps": [{"id": "layer.new.layer", "params": {"name": "X"}}, {"id": "no.such.command"}], "documents": [0, 1]}))
        .await
        .remove(0);
    assert_eq!((reply["completed"].as_u64(), reply["failed"].as_u64()), (Some(0), Some(2)), "{reply}");
    assert!(!c.ok("doc_inspect", json!({"document": 0})).await[0]["layers"].to_string().contains("\"X\""), "rolled back");
    // A saved Action that does not exist fails per document, not the call.
    let reply = c.ok("actions_play", json!({"action": "Nope", "documents": [0]})).await.remove(0);
    assert_eq!(reply["failed"], 1, "{reply}");

    for bad in [
        json!({"documents": [0]}),
        json!({"action": "A", "steps": [{"id": "layer.new.layer"}], "documents": [0]}),
        json!({"steps": [{"id": "layer.new.layer"}], "documents": []}),
        json!({"steps": [{"id": "layer.new.layer"}], "documents": [7]}),
        json!({"steps": [{"id": "layer.new.layer"}], "documents": [0], "overrides": [{"document": 1, "step": 0, "params": {}}]}),
        json!({"steps": [{"id": "layer.new.layer"}], "documents": [0], "overrides": [{"document": 0, "step": 3, "params": {}}]}),
        json!({"action": "A", "documents": [0], "overrides": [{"document": 0, "step": 0, "params": {}}]}),
    ] {
        let (error, blocks) = c.call("actions_play", bad.clone()).await;
        assert!(error, "{bad}: {blocks:?}");
    }
}
