//! The seeing tools, guarded edits and atomic batches over MCP, in-process: headless against a
//! real workspace on disk, and bridged against an app stand-in that runs the real engine.

use base64::Engine as _;
use magicpic_automation::{AuthorizedWorkspace, Headless, MagicpicMcp};
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientConfig};
use rmcp::service::RunningService;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const CONTROL_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Clone, Default)]
struct Client;
impl ClientHandler for Client {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}

type Mcp = RunningService<RoleClient, Client>;

async fn connect(server: MagicpicMcp) -> Mcp {
    let (s, c) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(s).await {
            let _ = running.waiting().await;
        }
    });
    Client.serve(c).await.expect("client init")
}

async fn call(client: &Mcp, name: &str, args: Value) -> CallToolResult {
    let mut p = CallToolRequestParams::new(name.to_owned());
    if let Value::Object(m) = args {
        p = p.with_arguments(m);
    }
    client.call_tool(p).await.expect("call_tool transport")
}

fn text(r: &CallToolResult) -> String {
    r.content.iter().filter_map(|c| c.as_text()).map(|t| t.text.clone()).collect::<Vec<_>>().join("\n")
}

fn json_of(r: &CallToolResult) -> Value {
    assert_ne!(r.is_error, Some(true), "tool error: {}", text(r));
    serde_json::from_str(&text(r)).unwrap_or_else(|e| panic!("not JSON ({e}): {}", text(r)))
}

fn error_of(r: &CallToolResult) -> String {
    assert_eq!(r.is_error, Some(true), "expected an error, got: {}", text(r));
    text(r)
}

/// The image of a tool result: its decoded pixels, MIME type and the JSON note beside it.
fn image_of(r: &CallToolResult) -> (magicpic_codecs::Image, String, Value) {
    assert_ne!(r.is_error, Some(true), "tool error: {}", text(r));
    let img = r.content.iter().find_map(|c| c.as_image()).expect("image content");
    let bytes = base64::engine::general_purpose::STANDARD.decode(&img.data).unwrap();
    let note = serde_json::from_str(&text(r)).unwrap_or(Value::Null);
    (magicpic_codecs::decode(&bytes).unwrap(), img.mime_type.clone(), note)
}

struct Workspace(std::path::PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!("pc-seeing-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Workspace(d)
    }
    fn server(&self) -> MagicpicMcp {
        MagicpicMcp::headless_with_workspace(AuthorizedWorkspace::new(Some(&self.0), Some(&self.0)).expect("test workspace"))
    }
    /// A 120×80 photo-like PNG: a sky gradient over dark ground with a bright warm square.
    fn photo(&self, name: &str) {
        let (w, h) = (120u32, 80u32);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let c: [u8; 4] = if (40..60).contains(&x) && (30..50).contains(&y) {
                    [240, 190, 60, 255]
                } else if y < 45 {
                    [90, 130, (160 + y) as u8, 255]
                } else {
                    [30 + (x % 7) as u8, 40, 25, 255]
                };
                px.extend(c);
            }
        }
        let img = magicpic_codecs::Image::from_u8(w, h, magicpic_codecs::ChannelLayout::Rgba, px).unwrap();
        std::fs::write(self.0.join(name), magicpic_codecs::encode(&img, magicpic_codecs::Format::Png, &Default::default()).unwrap()).unwrap();
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn v_all(client: &Mcp) -> Value {
    json_of(&call(client, "files_list", json!({})).await)["entries"].clone()
}

async fn revision(client: &Mcp) -> u64 {
    json_of(&call(client, "doc_inspect", json!({})).await)["revision"].as_u64().expect("revision")
}

#[tokio::test(flavor = "multi_thread")]
async fn files_list_finds_the_newest_picture_and_stays_inside_the_roots() {
    let ws = Workspace::new("list");
    ws.photo("old.png");
    std::fs::create_dir(ws.0.join("inbox")).unwrap();
    std::fs::write(ws.0.join("inbox/notes.txt"), "hello").unwrap();
    std::fs::write(ws.0.join(".hidden.png"), "x").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    ws.photo("new.png");
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/hostname", ws.0.join("link.png")).unwrap();
    let client = connect(ws.server()).await;

    let v = json_of(&call(&client, "files_list", json!({})).await);
    let names: Vec<&str> = v["entries"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert_eq!(names.first(), Some(&"new.png"), "newest first: {names:?}");
    assert!(names.contains(&"old.png") && names.contains(&"inbox"), "{names:?}");
    assert!(!names.contains(&".hidden.png") && !names.contains(&"link.png"), "hidden files and links are not listed: {names:?}");
    let entry = v["entries"].as_array().unwrap().iter().find(|e| e["path"] == "new.png").unwrap();
    assert_eq!(entry["kind"], "file");
    assert!(entry["bytes"].as_u64().unwrap() > 0 && entry["modifiedUnix"].as_u64().unwrap() > 1_600_000_000);
    assert_eq!(v["truncated"], false);

    let v = json_of(&call(&client, "files_list", json!({"dir": "inbox"})).await);
    assert_eq!(v["entries"][0]["path"], "inbox/notes.txt");
    let v = json_of(&call(&client, "files_list", json!({"limit": 1})).await);
    assert_eq!(v["entries"].as_array().unwrap().len(), 1);
    assert_eq!(v["truncated"], true);

    for dir in ["..", "../..", "/etc", "inbox/../..", "nowhere", "C:\\Windows"] {
        let e = error_of(&call(&client, "files_list", json!({"dir": dir})).await);
        assert!(!e.contains("hostname") && !e.contains("passwd"), "{dir}: {e}");
    }
    // The listed path opens, and `dir: ""` (what the reply calls the roots) lists them too.
    json_of(&call(&client, "doc_open", json!({"path": names[0]})).await);
    assert_eq!(json_of(&call(&client, "files_list", json!({"dir": ""})).await)["entries"], v_all(&client).await);
    client.cancel().await.unwrap();

    // Two roots holding the same name: a relative path opens the first root's file, so each
    // entry also carries the full path that names exactly that file. A name `doc_open` would
    // refuse is not listed at all.
    let second = Workspace::new("list-second");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    // The second root's picture has another size, so which file opened can be told apart.
    let small = magicpic_codecs::Image::from_u8(60, 40, magicpic_codecs::ChannelLayout::Rgba, vec![200; 60 * 40 * 4]).unwrap();
    std::fs::write(second.0.join("old.png"), magicpic_codecs::encode(&small, magicpic_codecs::Format::Png, &Default::default()).unwrap()).unwrap();
    std::fs::write(second.0.join("odd name. "), "x").unwrap();
    let both = AuthorizedWorkspace::with_read_roots([ws.0.as_path(), second.0.as_path()], None).unwrap();
    let client = connect(MagicpicMcp::headless_with_workspace(both)).await;
    let v = json_of(&call(&client, "files_list", json!({})).await);
    let entries = v["entries"].as_array().unwrap();
    assert!(entries.iter().all(|e| e["path"] != "odd name. "), "{entries:?}");
    let twins: Vec<&Value> = entries.iter().filter(|e| e["path"] == "old.png").collect();
    assert_eq!(twins.len(), 2);
    assert_eq!((twins[0]["root"].as_u64(), twins[1]["root"].as_u64()), (Some(1), Some(0)), "newest first");
    let opened = json_of(&call(&client, "doc_open", json!({"path": twins[0]["absolutePath"]})).await);
    assert_eq!((opened["name"].as_str(), opened["width"].as_u64()), (Some("old.png"), Some(60)), "the second root's file was asked for: {opened}");
    // The relative path is the first root's file, as a read has always resolved it.
    let opened = json_of(&call(&client, "doc_open", json!({"path": "old.png"})).await);
    assert_eq!(opened["width"].as_u64(), Some(120));
    client.cancel().await.unwrap();

    // Without roots there is nothing to list.
    let client = connect(MagicpicMcp::headless()).await;
    assert!(error_of(&call(&client, "files_list", json!({})).await).contains("not granted"));
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn previews_show_views_and_regions_without_editing() {
    let ws = Workspace::new("views");
    ws.photo("photo.png");
    let client = connect(ws.server()).await;
    json_of(&call(&client, "doc_open", json!({"path": "photo.png"})).await);
    let before = json_of(&call(&client, "doc_inspect", json!({})).await);

    // The plain preview is unchanged by this work.
    let plain = call(&client, "render_preview", json!({"max_side": 60})).await;
    assert_eq!(image_of(&plain).0.dimensions(), (60, 40));

    for view in ["normal", "luminance", "masses", "shapes", "lines", "color"] {
        let r = call(&client, "render_preview", json!({"view": view, "max_side": 60})).await;
        let (img, mime, note) = image_of(&r);
        assert_eq!(img.dimensions(), (60, 40), "{view}");
        assert_eq!(mime, "image/png");
        assert_eq!(note["view"], view);
        assert_eq!(note["analysisOnly"], view != "normal");
        assert!(note.get("base64").is_none(), "the note must not repeat the image");
        assert_eq!(note["document"]["revision"], before["revision"]);
    }
    let (grey, _, _) = image_of(&call(&client, "doc_render_preview", json!({"view": "luminance"})).await);
    assert!(grey.to_rgba8().chunks(4).all(|p| p[0] == p[1] && p[1] == p[2]));

    // A region: only the warm square, at its own size.
    let r = call(&client, "render_preview", json!({"region": {"x": 40, "y": 30, "width": 20, "height": 20}})).await;
    let (img, _, note) = image_of(&r);
    assert_eq!(img.dimensions(), (20, 20));
    assert!(img.to_rgba8().chunks(4).all(|p| p[0] > 200 && p[2] < 100));
    assert_eq!(note["region"]["width"], 20);

    // JPEG is offered for clients with small result limits.
    let (img, mime, _) = image_of(&call(&client, "render_preview", json!({"format": "jpeg", "quality": 70})).await);
    assert_eq!((img.dimensions(), mime.as_str()), ((120, 80), "image/jpeg"));

    for bad in [
        json!({"view": "pretty"}),
        json!({"region": {"x": 110, "y": 0, "width": 20, "height": 20}}),
        json!({"view": "shapes", "levels": 99}),
        json!({"view": "masses", "strength": 3.0}),
        json!({"format": "bmp"}),
        json!({"view": "lines", "max_side": 5000}),
        json!({"view": "lines", "index": 4}),
    ] {
        error_of(&call(&client, "render_preview", bad).await);
    }
    // Looking changed nothing: same revision, same history, same layers.
    assert_eq!(json_of(&call(&client, "doc_inspect", json!({})).await), before);
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn variants_are_reproducible_and_survive_save_and_reopen() {
    let ws = Workspace::new("variants");
    ws.photo("photo.png");
    let client = connect(ws.server()).await;
    json_of(&call(&client, "doc_open", json!({"path": "photo.png"})).await);
    let source = json_of(&call(&client, "doc_inspect", json!({})).await);

    // Three different ways of seeing the same picture, from one seed.
    let proposed =
        json_of(&call(&client, "command_run", json!({"id": "seeing.experiments", "params": {"seed": 21, "kinds": ["overexpose", "shake", "chance"]}})).await);
    let recipes = proposed["experiments"].as_array().unwrap().clone();
    assert_eq!(recipes.len(), 3);
    let mut previews = Vec::new();
    for (i, recipe) in recipes.iter().enumerate() {
        json_of(&call(&client, "doc_select", json!({"index": 0})).await);
        let v = json_of(&call(&client, "command_run", json!({"id": "seeing.variant", "params": {"recipe": recipe}})).await);
        assert_eq!(v["document"], i + 1);
        let (img, _, _) = image_of(&call(&client, "render_preview", json!({"view": "normal", "index": i + 1, "max_side": 0})).await);
        previews.push(img.to_rgba8());
    }
    assert!(previews[0] != previews[1] && previews[1] != previews[2], "the variants look alike");
    // The source is untouched and still first.
    json_of(&call(&client, "doc_select", json!({"index": 0})).await);
    assert_eq!(json_of(&call(&client, "doc_inspect", json!({})).await), source);

    // Save the first variant as a project and as a flat picture; both are verified on disk.
    let saved = json_of(&call(&client, "doc_save", json!({"index": 1, "path": "bright.pcraft"})).await);
    assert_eq!(saved["verified"], true);
    assert_eq!(saved["bytes"].as_u64(), Some(std::fs::metadata(ws.0.join("bright.pcraft")).unwrap().len()));
    assert!(std::path::Path::new(saved["absolutePath"].as_str().unwrap()).is_absolute());
    let exported = json_of(&call(&client, "doc_export", json!({"index": 1, "path": "bright.png"})).await);
    assert_eq!(exported["verified"], true);
    // The export opens again and is the picture that was previewed.
    let flat = magicpic_codecs::decode(&std::fs::read(ws.0.join("bright.png")).unwrap()).unwrap();
    assert_eq!(flat.dimensions(), (120, 80));
    assert_eq!(flat.to_rgba8(), previews[0], "preview and export differ");

    // Reopen the project: layers, and the recipe, are still there.
    let opened = json_of(&call(&client, "doc_open", json!({"path": "bright.pcraft"})).await);
    let reopened = opened["index"].as_u64().or_else(|| opened["document"].as_u64()).unwrap_or(4);
    let inspected = json_of(&call(&client, "doc_inspect", json!({})).await);
    assert_eq!(inspected["layers"].as_array().unwrap().len(), 2, "the adjustment layer was flattened away: {inspected}");
    let stored = json_of(&call(&client, "command_run", json!({"id": "seeing.recipe"})).await);
    assert_eq!(stored["recipe"]["steps"], recipes[0]["steps"]);
    assert_eq!(stored["recipe"]["source"]["name"], "photo.png");
    let (img, _, _) = image_of(&call(&client, "render_preview", json!({"view": "normal", "max_side": 0})).await);
    assert_eq!(img.to_rgba8(), previews[0], "document {reopened} changed across save and reopen");

    // Replaying the stored recipe on the source gives the same pixels again.
    json_of(&call(&client, "doc_select", json!({"index": 0})).await);
    let again = json_of(&call(&client, "command_run", json!({"id": "seeing.variant", "params": {"recipe": stored["recipe"]}})).await);
    let (img, _, _) = image_of(&call(&client, "render_preview", json!({"view": "normal", "index": again["document"], "max_side": 0})).await);
    assert_eq!(img.to_rgba8(), previews[0], "the recipe did not reproduce its variant");
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_revision_is_refused_and_a_current_one_runs() {
    let client = connect(MagicpicMcp::headless()).await;
    json_of(&call(&client, "doc_new", json!({"width": 32, "height": 32})).await);
    let seen = revision(&client).await;
    // Somebody else edits after the agent looked.
    json_of(&call(&client, "command_run", json!({"id": "layer.newAdjustmentLayer.invert"})).await);
    let now = revision(&client).await;
    assert!(now > seen);

    let stale = json!({"id": "layer.flattenImage", "expect_revision": seen});
    let e = error_of(&call(&client, "command_run", stale).await);
    assert!(e.contains(&format!("revision {now}, not {seen}")) && e.contains("Nothing was run"), "{e}");
    assert_eq!(revision(&client).await, now, "the refused command changed the document");

    // With the current revision the command runs and returns its own result.
    let r = json_of(&call(&client, "command_run", json!({"id": "layer.newAdjustmentLayer.posterize", "params": {"levels": 3}, "expect_revision": now})).await);
    assert!(r.get("completed").is_none(), "the transaction wrapper leaked into the reply: {r}");
    assert_eq!(json_of(&call(&client, "doc_inspect", json!({})).await)["layers"].as_array().unwrap().len(), 3);

    let e = error_of(&call(&client, "command_run", json!({"id": "filter.blur.gaussianBlur", "expect_revision": 1, "wait": false})).await);
    assert!(e.contains("wait: false"), "{e}");

    // Two fresh documents share a revision: only the id tells which one the edit was meant for.
    let first = json_of(&call(&client, "doc_inspect", json!({})).await);
    json_of(&call(&client, "doc_new", json!({"width": 16, "height": 16, "name": "second"})).await);
    let second = json_of(&call(&client, "doc_inspect", json!({})).await);
    assert_ne!(first["id"], second["id"]);
    let listed = json_of(&call(&client, "session_list", json!({})).await);
    assert_eq!(listed["documents"][1]["id"], second["id"]);
    let meant_for_first = json!({"id": "layer.newAdjustmentLayer.invert", "expect_document": first["id"]});
    let e = error_of(&call(&client, "command_run", meant_for_first.clone()).await);
    assert!(e.contains("another document is active") && e.contains("Nothing was run"), "{e}");
    assert_eq!(json_of(&call(&client, "doc_inspect", json!({})).await), second, "the wrong document was edited");
    json_of(&call(&client, "doc_select", json!({"index": 0})).await);
    json_of(&call(&client, "command_run", meant_for_first).await);
    let steps = json!([{"id": "layer.newAdjustmentLayer.invert"}]);
    let r = json_of(&call(&client, "command_batch", json!({"steps": steps, "atomic": true, "expect_document": second["id"]})).await);
    assert_eq!(r["outcome"], "not_run");
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_atomic_batch_rolls_back_and_a_plain_batch_does_not() {
    let client = connect(MagicpicMcp::headless()).await;
    json_of(&call(&client, "doc_new", json!({"width": 32, "height": 32})).await);
    let before = json_of(&call(&client, "doc_inspect", json!({})).await);
    let steps = json!([
        {"id": "layer.newAdjustmentLayer.exposure", "params": {"exposure": 1}},
        {"id": "layer.newAdjustmentLayer.invert"},
        {"id": "filter.blur.gaussianBlur", "params": {"radius": "soft"}},
        {"id": "layer.newAdjustmentLayer.posterize"},
    ]);

    let r = json_of(&call(&client, "command_batch", json!({"steps": steps, "atomic": true})).await);
    assert_eq!((r["completed"].as_u64(), r["failed"].as_u64(), r["atomic"].as_bool()), (Some(0), Some(1), Some(true)));
    let error = r["results"][0]["error"].as_str().unwrap();
    assert!(error.contains("step 3 of 4") && error.contains("rolled back"), "{error}");
    assert_eq!(r["outcome"], "rolled_back");
    let after = json_of(&call(&client, "doc_inspect", json!({})).await);
    assert_eq!(after["layers"], before["layers"], "layers left behind by a rolled-back batch");
    assert_eq!(after["history"], before["history"], "history left behind by a rolled-back batch");
    assert_eq!((after["canUndo"].clone(), after["canRedo"].clone()), (before["canUndo"].clone(), before["canRedo"].clone()));

    // The same batch without `atomic` keeps its two successful steps, as documented.
    let r = json_of(&call(&client, "command_batch", json!({"steps": steps})).await);
    assert_eq!((r["completed"].as_u64(), r["failed"].as_u64()), (Some(2), Some(1)));
    assert_eq!(json_of(&call(&client, "doc_inspect", json!({})).await)["layers"].as_array().unwrap().len(), 3);

    // A good atomic batch applies everything and reports the new revision.
    let seen = revision(&client).await;
    let good = json!([{"id": "layer.newAdjustmentLayer.vibrance", "params": {"vibrance": 20}}, {"id": "layer.newAdjustmentLayer.photoFilter"}]);
    let r = json_of(&call(&client, "command_batch", json!({"steps": good, "atomic": true, "expect_revision": seen})).await);
    assert_eq!((r["completed"].as_u64(), r["failed"].as_u64()), (Some(2), Some(0)));
    assert_eq!(r["results"].as_array().unwrap().len(), 2);
    assert_eq!(r["outcome"], "applied");
    assert_eq!(r["revision"].as_u64(), Some(revision(&client).await));
    // The revision it was planned against is now stale.
    let r = json_of(&call(&client, "command_batch", json!({"steps": good, "atomic": true, "expect_revision": seen})).await);
    assert!(r["results"][0]["error"].as_str().unwrap().contains("Nothing was run"));
    assert_eq!((r["outcome"].as_str(), r["completed"].as_u64()), (Some("not_run"), Some(0)));
    // As many steps as a plain batch takes (256) are one transaction too.
    let many: Vec<Value> = (0..100).map(|_| json!({"id": "select.all"})).collect();
    let r = json_of(&call(&client, "command_batch", json!({"steps": many, "atomic": true})).await);
    assert_eq!((r["completed"].as_u64(), r["outcome"].as_str()), (Some(100), Some("applied")), "{r}");
    assert_eq!(json_of(&call(&client, "doc_inspect", json!({})).await)["layers"].as_array().unwrap().len(), 5);

    for bad in [
        json!({"steps": good, "expect_revision": 3}),
        json!({"steps": good, "atomic": true, "stop_on_error": false}),
        json!({"steps": [{"id": "filter.blur.gaussianBlur", "wait": false}], "atomic": true}),
    ] {
        error_of(&call(&client, "command_batch", bad).await);
    }
    // Steps that leave the document cannot be part of a transaction.
    let r = json_of(&call(&client, "command_batch", json!({"steps": [{"id": "layer.newAdjustmentLayer.invert"}, {"id": "file.close"}], "atomic": true})).await);
    assert!(r["results"][0]["error"].as_str().unwrap().contains("not something a transaction can take back"));
    assert_eq!(r["outcome"], "not_run");
    assert_eq!(json_of(&call(&client, "session_list", json!({})).await)["documents"].as_array().unwrap().len(), 1);
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn transactions_and_recipes_cannot_reach_files_outside_the_roots() {
    let ws = Workspace::new("gate");
    ws.photo("photo.png");
    let client = connect(ws.server()).await;
    json_of(&call(&client, "doc_open", json!({"path": "photo.png"})).await);
    let before = json_of(&call(&client, "doc_inspect", json!({})).await);
    // A step that names an ambient file is refused by the same gate a top-level command meets,
    // and the earlier step is rolled back.
    let lut = json!({"id": "layer.newAdjustmentLayer.colorLookup", "params": {"file": "/etc/hostname"}});
    let steps = json!([{"id": "layer.newAdjustmentLayer.invert"}, lut]);
    let r = json_of(&call(&client, "command_batch", json!({"steps": steps, "atomic": true})).await);
    assert_eq!(r["failed"], 1);
    assert!(r["results"][0]["error"].as_str().unwrap().contains("rolled back"), "{r}");
    assert_eq!(json_of(&call(&client, "doc_inspect", json!({})).await)["layers"], before["layers"]);
    // The same inside a recipe: the variant is discarded.
    let e = error_of(&call(&client, "command_run", json!({"id": "seeing.variant", "params": {"recipe": {"steps": [lut]}}})).await);
    assert!(e.contains("discarded"), "{e}");
    assert_eq!(json_of(&call(&client, "session_list", json!({})).await)["documents"].as_array().unwrap().len(), 1);
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_files_and_refused_writes_are_errors_that_keep_the_session() {
    let ws = Workspace::new("bad");
    ws.photo("photo.png");
    let good = std::fs::read(ws.0.join("photo.png")).unwrap();
    std::fs::write(ws.0.join("truncated.png"), &good[..good.len() / 3]).unwrap();
    std::fs::write(ws.0.join("text.jpg"), "this is not a picture").unwrap();
    std::fs::write(ws.0.join("empty.png"), "").unwrap();
    std::fs::write(ws.0.join("notes.txt"), "keep me").unwrap();
    let client = connect(ws.server()).await;
    for path in ["truncated.png", "text.jpg", "empty.png", "missing.png", "../photo.png", "/etc/hostname"] {
        error_of(&call(&client, "doc_open", json!({"path": path})).await);
    }
    assert_eq!(json_of(&call(&client, "session_list", json!({})).await)["documents"].as_array().unwrap().len(), 0, "a bad file left a document open");
    // The session still works.
    json_of(&call(&client, "doc_open", json!({"path": "photo.png"})).await);
    // Exports that may not be written say so and write nothing.
    for path in ["../out.png", "/tmp/pc-seeing-outside.png", "nodir/out.png", "notes.txt"] {
        let e = error_of(&call(&client, "doc_export", json!({"path": path})).await);
        assert!(!e.contains("verified"), "{path}: {e}");
    }
    assert_eq!(std::fs::read_to_string(ws.0.join("notes.txt")).unwrap(), "keep me");
    assert!(!std::path::Path::new("/tmp/pc-seeing-outside.png").exists());
    assert_eq!(json_of(&call(&client, "doc_export", json!({"path": "out.png"})).await)["verified"], true);
    client.cancel().await.unwrap();

    // A read-only launch cannot export at all.
    let read_only = MagicpicMcp::headless_with_workspace(AuthorizedWorkspace::new(Some(&ws.0), None).unwrap());
    let client = connect(read_only).await;
    json_of(&call(&client, "doc_open", json!({"path": "photo.png"})).await);
    assert!(error_of(&call(&client, "doc_export", json!({"path": "second.png"})).await).contains("write authority is absent"));
    assert!(!ws.0.join("second.png").exists());
    client.cancel().await.unwrap();
}

/// An app stand-in for bridge mode: the real handshake, then every control request answered by
/// a real headless engine, the way the desktop app answers with its own session.
async fn engine_app(mut backend: Headless) -> (String, tokio::task::JoinHandle<(Headless, Vec<String>)>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let app = tokio::spawn(async move {
        let mut seen = Vec::new();
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        let mut reader = BufReader::new(read);
        crate::bridge::serve_handshake(&mut reader, &mut write).await;
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
            let request: Value = serde_json::from_str(&line).unwrap();
            let method = request["method"].as_str().unwrap_or_default().to_owned();
            seen.push(match request["params"]["command"].as_str() {
                Some(command) => format!("{method} {command}"),
                None => method.clone(),
            });
            let reply = match backend.handle(&method, request["params"].clone()) {
                Ok(result) => json!({"id": request["id"], "ok": true, "result": result}),
                Err(error) => json!({"id": request["id"], "ok": false, "error": error.to_string()}),
            };
            write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
        }
        (backend, seen)
    });
    (addr, app)
}

#[tokio::test(flavor = "multi_thread")]
async fn bridged_views_render_the_document_and_edits_reach_the_app_session() {
    let mut backend = Headless::new();
    backend.handle("doc.new", json!({"width": 64, "height": 48, "name": "open in app"})).unwrap();
    let (addr, app) = engine_app(backend).await;
    let client = connect(MagicpicMcp::bridge(&addr, CONTROL_TOKEN).unwrap()).await;

    // A document view in bridge mode is the document, not the window.
    let (img, _, note) = image_of(&call(&client, "render_preview", json!({"view": "normal"})).await);
    assert_eq!(img.dimensions(), (64, 48));
    assert_eq!(note["document"]["name"], "open in app");
    let seen = note["document"]["revision"].as_u64().unwrap();
    let (img, _, _) = image_of(&call(&client, "render_preview", json!({"index": 0, "view": "lines", "max_side": 32})).await);
    assert_eq!(img.dimensions(), (32, 24));

    // A guarded edit reaches the app's own session…
    json_of(&call(&client, "command_run", json!({"id": "layer.newAdjustmentLayer.invert", "expect_revision": seen})).await);
    // …a stale one does not…
    let e = error_of(&call(&client, "command_run", json!({"id": "layer.flattenImage", "expect_revision": seen})).await);
    assert!(e.contains("Nothing was run"), "{e}");
    // …and a failing atomic batch leaves the app's document as it was.
    let steps = json!([{"id": "layer.newAdjustmentLayer.posterize"}, {"id": "no.such.command"}]);
    let r = json_of(&call(&client, "command_batch", json!({"steps": steps, "atomic": true})).await);
    assert!(r["results"][0]["error"].as_str().unwrap().contains("rolled back"), "{r}");
    // files_list belongs to headless mode and says what to do instead.
    assert!(error_of(&call(&client, "files_list", json!({})).await).contains("headless mode"));

    client.cancel().await.unwrap();
    let (backend, seen) = app.await.unwrap();
    let doc = backend.session.active().unwrap();
    assert_eq!(doc.doc.name, "open in app");
    assert_eq!(doc.doc.layer_count(), 2, "exactly the one guarded edit was applied");
    assert!(seen.iter().filter(|m| m.as_str() == "engine.execute seeing.view").count() == 2, "{seen:?}");
    assert!(seen.iter().all(|m| m != "ui.screenshot"), "a document view must not fall back to the window: {seen:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bridge_without_an_app_fails_instead_of_editing_another_session() {
    // Nothing listens here: the bridge must not quietly become a headless session.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    let client = connect(MagicpicMcp::bridge(&addr, CONTROL_TOKEN).unwrap()).await;
    for (tool, args) in [
        ("render_preview", json!({"view": "luminance"})),
        ("command_run", json!({"id": "layer.newAdjustmentLayer.invert", "expect_revision": 1})),
        ("command_batch", json!({"steps": [{"id": "layer.newAdjustmentLayer.invert"}], "atomic": true})),
        ("doc_new", json!({"width": 8, "height": 8})),
    ] {
        let r = call(&client, tool, args).await;
        let message = text(&r);
        assert!(message.contains("cannot connect") && message.contains("not sent"), "{tool}: {message}");
        if tool == "command_batch" {
            assert_eq!(json_of(&r)["outcome"], "not_run", "nothing was sent, so nothing ran");
        }
    }
    client.cancel().await.unwrap();
}

/// The app applies an atomic batch but its reply never arrives: the tool must say the outcome
/// is unknown, not that nothing ran, and must not send the batch again.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_reply_to_an_atomic_batch_is_reported_as_unknown_and_never_resent() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let app = tokio::spawn(async move {
        let mut backend = Headless::new();
        backend.handle("doc.new", json!({"width": 8, "height": 8})).unwrap();
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        let mut reader = BufReader::new(read);
        crate::bridge::serve_handshake(&mut reader, &mut write).await;
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        backend.handle(request["method"].as_str().unwrap(), request["params"].clone()).unwrap();
        // The edit is applied; the connection closes before the reply.
        drop((reader, write));
        // A second connection would be a resend: there must be none.
        let resent = tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept()).await.is_ok();
        (backend.session.active().unwrap().doc.layer_count(), resent)
    });
    let client = connect(MagicpicMcp::bridge(&addr, CONTROL_TOKEN).unwrap()).await;
    let steps = json!([{"id": "layer.newAdjustmentLayer.invert"}, {"id": "layer.newAdjustmentLayer.posterize"}]);
    let r = json_of(&call(&client, "command_batch", json!({"steps": steps, "atomic": true})).await);
    assert_eq!(r["outcome"], "unknown", "{r}");
    assert!(r["completed"].is_null(), "an unknown outcome must not claim that nothing ran: {r}");
    let error = r["results"][0]["error"].as_str().unwrap();
    assert!(error.contains("may have completed") && error.contains("inspect"), "{error}");
    assert!(!error.contains("rolled back"), "{error}");
    let (layers, resent) = app.await.unwrap();
    assert_eq!(layers, 3, "the app applied the whole batch exactly once");
    assert!(!resent, "the batch was sent again");
    client.cancel().await.unwrap();
}
