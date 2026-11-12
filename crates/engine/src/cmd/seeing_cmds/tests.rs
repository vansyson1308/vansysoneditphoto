use serde_json::{Value, json};

use super::*;

/// A session with one 96×64 picture at `depth` bits: a dark left half, a light right half and a
/// saturated red square, so every view has something to show.
fn picture(depth: u32) -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 96, "height": 64, "depth": depth, "name": "card"})).unwrap();
    s.edit("Test Picture", |doc, _| {
        let fmt = doc.pixel_format();
        let b = doc.bounds();
        let mut vals = Vec::new();
        for y in 0..64 {
            for x in 0..96 {
                let base = if x < 48 { 0.15 } else { 0.85 };
                let c = if (10..26).contains(&x) && (10..26).contains(&y) { [0.9, 0.1, 0.1, 1.0] } else { [base, base, base, 1.0] };
                vals.extend(magicpic_raster::from_rgba(&fmt, c));
            }
        }
        let layer = doc.layers.first_mut().ok_or(EngineError::NoDocument)?;
        crate::pixels_mut(layer)?.write_region(b, &vals);
        Ok(())
    })
    .unwrap();
    s
}

fn flat(s: &Session, index: usize) -> Vec<[f32; 4]> {
    magicpic_compose::flatten(&s.documents()[index].doc).px.clone()
}

/// What must not move when a command only looks: revision, history length, pixels, notes.
fn fingerprint(s: &Session, index: usize) -> (u64, usize, Vec<[f32; 4]>, usize) {
    let st = &s.documents()[index];
    (st.revision, st.history.past_len(), flat(s, index), st.doc.notes.len())
}

fn decode(v: &Value) -> magicpic_codecs::Image {
    let bytes = base64::engine::general_purpose::STANDARD.decode(v["base64"].as_str().unwrap()).unwrap();
    assert_eq!(v["bytes"], bytes.len());
    magicpic_codecs::decode(&bytes).unwrap()
}

#[test]
fn every_view_returns_an_image_and_changes_nothing() {
    for depth in [8, 16, 32] {
        let mut s = picture(depth);
        let before = fingerprint(&s, 0);
        let journal = s.journal.len();
        for view in ["normal", "luminance", "masses", "shapes", "lines", "color"] {
            let v = s.execute("seeing.view", json!({"view": view, "maxSide": 48})).unwrap();
            assert_eq!(v["mime"], "image/png");
            assert_eq!((v["width"].as_u64(), v["height"].as_u64()), (Some(48), Some(32)), "{view} at {depth} bit");
            assert_eq!(v["view"], view);
            assert_eq!(v["analysisOnly"], view != "normal");
            assert_eq!(v["document"]["revision"], before.0);
            let img = decode(&v);
            assert_eq!(img.dimensions(), (48, 32));
        }
        assert_eq!(fingerprint(&s, 0), before, "a view edited the {depth}-bit document");
        assert_eq!(s.documents().len(), 1);
        assert_eq!(s.journal.len(), journal, "views are queries, not journal entries");
    }
}

#[test]
fn normal_view_matches_the_composite_and_luminance_is_grey() {
    let mut s = picture(8);
    let v = s.execute("seeing.view", json!({"maxSide": 96})).unwrap();
    let img = decode(&v);
    let px = img.to_rgba8();
    // Left half dark, right half light, red square red: the picture as it is.
    let at = |x: usize, y: usize| -> [u8; 4] { [px[(y * 96 + x) * 4], px[(y * 96 + x) * 4 + 1], px[(y * 96 + x) * 4 + 2], px[(y * 96 + x) * 4 + 3]] };
    assert_eq!(at(40, 50), [38, 38, 38, 255]);
    assert_eq!(at(80, 50), [217, 217, 217, 255]);
    assert!(at(15, 15)[0] > 200 && at(15, 15)[1] < 50);
    let v = s.execute("seeing.view", json!({"view": "luminance", "maxSide": 96})).unwrap();
    let px = decode(&v).to_rgba8();
    assert!(px.chunks(4).all(|p| p[0] == p[1] && p[1] == p[2]));
}

#[test]
fn a_region_is_rendered_alone() {
    let mut s = picture(8);
    // The red square only.
    let v = s.execute("seeing.view", json!({"region": {"x": 10, "y": 10, "width": 16, "height": 16}, "maxSide": 64})).unwrap();
    assert_eq!((v["width"].as_u64(), v["height"].as_u64()), (Some(16), Some(16)), "regions are never enlarged");
    assert_eq!(v["region"], json!({"x": 10, "y": 10, "width": 16, "height": 16}));
    let px = decode(&v).to_rgba8();
    assert!(px.chunks(4).all(|p| p[0] > 200 && p[1] < 50), "only the red square");
    for region in [
        json!({"x": 90, "y": 0, "width": 16, "height": 16}),
        json!({"x": 0, "y": 0, "width": 0, "height": 5}),
        json!({"x": -1, "y": 0, "width": 5, "height": 5}),
        json!("all"),
        json!({"x": 1}),
    ] {
        assert!(s.execute("seeing.view", json!({"region": region})).is_err(), "{region}");
    }
}

#[test]
fn jpeg_views_are_opaque_and_another_document_can_be_viewed() {
    let mut s = picture(8);
    s.execute("file.new", json!({"width": 20, "height": 10, "name": "other", "background": "transparent"})).unwrap();
    let v = s.execute("seeing.view", json!({"format": "jpeg", "quality": 60})).unwrap();
    assert_eq!(v["mime"], "image/jpeg");
    let img = decode(&v);
    assert_eq!(img.dimensions(), (20, 10));
    // Transparency shows as white paper.
    let px = img.to_rgba8();
    assert!(px.chunks(4).all(|p| p[0] > 250 && p[3] == 255));
    // `document` looks at the first document without making it active.
    let v = s.execute("seeing.view", json!({"document": 0, "maxSide": 48})).unwrap();
    assert_eq!(v["document"]["name"], "card");
    assert_eq!(s.active_index(), Some(1));
    assert!(s.execute("seeing.view", json!({"document": 7})).is_err());
    assert!(s.execute("seeing.view", json!({"format": "gif"})).is_err());
}

#[test]
fn a_study_is_a_separate_document() {
    for depth in [8, 16, 32] {
        let mut s = picture(depth);
        let before = fingerprint(&s, 0);
        let v = s.execute("seeing.study", json!({"view": "shapes", "levels": 2})).unwrap();
        assert_eq!(v["document"], 1);
        assert_eq!(v["name"], "card (shapes study)");
        assert_eq!(fingerprint(&s, 0), before, "the source changed");
        let study = &s.documents()[1].doc;
        assert_eq!((study.size.width, study.size.height), (96, 64));
        assert_eq!(study.layers.len(), 1);
        // Two tones: black and white.
        assert!(flat(&s, 1).iter().all(|p| p[0] < 0.01 || p[0] > 0.99));
    }
    let mut s = picture(8);
    assert!(s.execute("seeing.study", json!({})).is_err(), "a study needs a view");
    assert!(s.execute("seeing.study", json!({"view": "normal"})).is_err());
    for id in ["seeing.study.luminance", "seeing.study.masses", "seeing.study.shapes", "seeing.study.lines", "seeing.study.color"] {
        s.set_active(0);
        s.execute(id, json!({})).unwrap();
    }
    assert_eq!(s.documents().len(), 6);
}

#[test]
fn frames_are_a_function_of_the_seed() {
    let mut s = picture(8);
    let before = fingerprint(&s, 0);
    let a = s.execute("seeing.frames", json!({"seed": 11, "count": 4})).unwrap();
    assert_eq!(a, s.execute("seeing.frames", json!({"seed": 11, "count": 4})).unwrap());
    assert_ne!(a, s.execute("seeing.frames", json!({"seed": 12, "count": 4})).unwrap());
    let frames = a["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 4);
    for f in frames {
        assert!(f["x"].as_u64().unwrap() + f["width"].as_u64().unwrap() <= 96);
        assert!(f["y"].as_u64().unwrap() + f["height"].as_u64().unwrap() <= 64);
        // Every frame can be looked at as it is.
        s.execute("seeing.view", json!({"region": f, "maxSide": 32})).unwrap();
    }
    assert_eq!(fingerprint(&s, 0), before);
}

#[test]
fn experiments_are_reproducible_recipes() {
    let mut s = picture(8);
    let before = fingerprint(&s, 0);
    let a = s.execute("seeing.experiments", json!({"seed": 5, "count": 4})).unwrap();
    assert_eq!(a, s.execute("seeing.experiments", json!({"seed": 5, "count": 4})).unwrap());
    assert_ne!(a["experiments"], s.execute("seeing.experiments", json!({"seed": 6, "count": 4})).unwrap()["experiments"]);
    assert_eq!(a["experiments"].as_array().unwrap().len(), 4);
    let named = s.execute("seeing.experiments", json!({"kinds": ["defocus", "overexpose"]})).unwrap();
    let kinds: Vec<&str> = named["experiments"].as_array().unwrap().iter().map(|r| r["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["defocus", "overexpose"]);
    assert_eq!(fingerprint(&s, 0), before, "proposing experiments changed the document");
    assert!(s.execute("seeing.experiments", json!({"kinds": ["sharpen"]})).is_err());
}

#[test]
fn every_kind_makes_a_variant_and_leaves_the_source_alone() {
    for depth in [8, 16, 32] {
        for (kind, _) in KINDS {
            let mut s = picture(depth);
            let before = fingerprint(&s, 0);
            let v = s.execute("seeing.variant", json!({"kind": kind, "seed": 3})).unwrap_or_else(|e| panic!("{kind} at {depth} bit: {e}"));
            assert_eq!(v["document"], 1, "{kind}");
            assert_eq!(v["sourceDocument"], 0);
            assert_eq!(fingerprint(&s, 0), before, "{kind} at {depth} bit changed the source");
            assert_eq!(s.active_index(), Some(1));
            assert_ne!(flat(&s, 1), flat(&s, 0), "{kind} at {depth} bit changed nothing");
            // The recipe travels with the variant.
            let stored = s.execute("seeing.recipe", json!({})).unwrap();
            assert_eq!(stored["recipe"], v["recipe"]);
            assert_eq!(stored["recipe"]["source"], json!({"name": "card", "width": 96, "height": 64}));
            assert_eq!(stored["recipe"]["seed"], 3);
        }
    }
}

#[test]
fn experiments_stay_editable() {
    // Exposure is an adjustment layer over the untouched pixels…
    let mut s = picture(8);
    s.execute("seeing.variant", json!({"kind": "overexpose", "seed": 1})).unwrap();
    let doc = &s.documents()[1].doc;
    assert_eq!(doc.layers.len(), 2);
    assert!(matches!(doc.layers[1].content, LayerContent::Adjustment(_)));
    // …and a blur is a smart filter on a stamp, with the original layer still underneath.
    let mut s = picture(8);
    s.execute("seeing.variant", json!({"kind": "defocus", "seed": 1})).unwrap();
    let doc = &s.documents()[1].doc;
    assert_eq!(doc.layers.len(), 2);
    assert!(matches!(doc.layers[0].content, LayerContent::Raster(_)));
    assert!(matches!(doc.layers[1].content, LayerContent::Smart(_)));
    assert_eq!(magicpic_compose::render_layer(&doc.layers[0], doc.bounds()).px, magicpic_compose::flatten(&s.documents()[0].doc).px);
}

#[test]
fn replaying_a_stored_recipe_gives_the_same_pixels() {
    for depth in [8, 16, 32] {
        for (kind, _) in KINDS {
            let mut s = picture(depth);
            s.execute("seeing.variant", json!({"kind": kind, "seed": 42})).unwrap();
            let recipe = s.execute("seeing.recipe", json!({})).unwrap()["recipe"].clone();
            // Through text and back, as a saved recipe would go.
            let recipe: Value = serde_json::from_str(&serde_json::to_string(&recipe).unwrap()).unwrap();
            s.set_active(0);
            let v = s.execute("seeing.variant", json!({"recipe": recipe, "name": "again"})).unwrap();
            assert_eq!(v["document"], 2);
            assert_eq!(v["name"], "again");
            assert_eq!(s.documents()[1].doc.size, s.documents()[2].doc.size, "{kind}");
            assert_eq!(flat(&s, 1), flat(&s, 2), "{kind} at {depth} bit did not replay exactly");
        }
    }
}

#[test]
fn a_frame_crops_the_variant_without_deleting_pixels() {
    let mut s = picture(8);
    let recipe = json!({"label": "red square", "frame": {"x": 10, "y": 10, "width": 16, "height": 16}, "steps": []});
    let v = s.execute("seeing.variant", json!({"recipe": recipe})).unwrap();
    assert_eq!((v["width"].as_u64(), v["height"].as_u64()), (Some(16), Some(16)));
    assert_eq!(v["name"], "card (red square)");
    assert!(flat(&s, 1).iter().all(|p| p[0] > 0.8 && p[1] < 0.2));
    // The rest of the picture is still in the layer, outside the canvas.
    let doc = &s.documents()[1].doc;
    let outside = magicpic_compose::render_layer(&doc.layers[0], Rect::new(60, 30, 61, 31)).px[0];
    assert!(outside[3] > 0.99 && outside[0] > 0.8, "{outside:?}");
    // A frame outside the picture is refused before anything is made.
    let bad = json!({"frame": {"x": 90, "y": 0, "width": 16, "height": 16}, "steps": []});
    assert!(s.execute("seeing.variant", json!({"recipe": bad})).is_err());
    assert_eq!(s.documents().len(), 2);
}

#[test]
fn a_failing_step_discards_the_variant() {
    let mut s = picture(8);
    let before = fingerprint(&s, 0);
    let recipe = json!({"label": "half done", "steps": [
        {"id": "layer.newAdjustmentLayer.exposure", "params": {"exposure": 1}},
        {"id": "filter.blur.gaussianBlur", "params": {"radius": "wide"}},
        {"id": "layer.newAdjustmentLayer.invert", "params": {}},
    ]});
    let e = s.execute("seeing.variant", json!({"recipe": recipe})).unwrap_err().to_string();
    assert!(e.contains("step 2 of 3") && e.contains("filter.blur.gaussianBlur") && e.contains("discarded"), "{e}");
    assert_eq!(s.documents().len(), 1, "the half-made variant stayed open");
    assert_eq!(s.active_index(), Some(0));
    assert_eq!(fingerprint(&s, 0), before, "the source changed");
    // Unknown commands fail the same way.
    let recipe = json!({"steps": [{"id": "no.such.command"}]});
    assert!(s.execute("seeing.variant", json!({"recipe": recipe})).is_err());
    assert_eq!(s.documents().len(), 1);
}

#[test]
fn recipes_cannot_leave_the_variant() {
    let mut s = picture(8);
    let before = fingerprint(&s, 0);
    for id in
        ["seeing.variant", "seeing.study.lines", "file.close", "file.new", "image.duplicate", "history.newDocument", "window.nextDocument", "actions.play"]
    {
        let recipe = json!({"steps": [{"id": id, "params": {}}]});
        let e = s.execute("seeing.variant", json!({"recipe": recipe})).unwrap_err().to_string();
        assert!(e.contains("not allowed in a recipe"), "{id}: {e}");
    }
    let many: Vec<Value> = (0..33).map(|_| json!({"id": "layer.newAdjustmentLayer.invert"})).collect();
    assert!(s.execute("seeing.variant", json!({"recipe": {"steps": many}})).is_err());
    assert!(s.execute("seeing.variant", json!({"recipe": {"magicpicSeeing": 2, "steps": []}})).is_err());
    assert!(s.execute("seeing.variant", json!({"recipe": {"steps": [], "surprise": 1}})).is_err());
    assert!(s.execute("seeing.variant", json!({"recipe": {"steps": [{"id": "layer.newAdjustmentLayer.invert", "params": 3}]}})).is_err());
    assert_eq!(s.documents().len(), 1);
    assert_eq!(fingerprint(&s, 0), before);
}

#[test]
fn the_menu_variant_records_its_seed() {
    let mut s = picture(8);
    let v = s.execute("seeing.variant.chance", json!({})).unwrap();
    let seed = v["recipe"]["seed"].as_u64().unwrap();
    assert!(seed < 1 << 53);
    assert_eq!(v["recipe"]["kind"], "chance");
    // The same seed by hand makes the same variant.
    s.set_active(0);
    s.execute("seeing.variant", json!({"kind": "chance", "seed": seed})).unwrap();
    assert_eq!(flat(&s, 1), flat(&s, 2));
}

#[test]
fn a_reaction_is_a_note_that_undo_takes_back() {
    let mut s = picture(8);
    s.execute("seeing.variant", json!({"kind": "underexpose", "seed": 2})).unwrap();
    let v = s.execute("seeing.note", json!({"reaction": "I like the heavy shadows", "reason": "they hide the clutter"})).unwrap();
    assert_eq!(v["reactions"], json!(["I like the heavy shadows (why: they hide the clutter)"]));
    s.execute("seeing.note", json!({"reaction": "too dark for the face"})).unwrap();
    let stored = s.execute("seeing.recipe", json!({})).unwrap();
    assert_eq!(stored["reactions"].as_array().unwrap().len(), 2);
    assert!(stored["recipe"].is_object(), "the recipe is still there");
    assert!(s.undo());
    assert_eq!(s.execute("seeing.recipe", json!({})).unwrap()["reactions"].as_array().unwrap().len(), 1);
    for p in [
        json!({}),
        json!({"reaction": ""}),
        json!({"reaction": "  "}),
        json!({"reaction": 5}),
        json!({"reaction": "x".repeat(201)}),
        json!({"reaction": "ok", "reason": 1}),
    ] {
        assert!(s.execute("seeing.note", p.clone()).is_err(), "{p}");
    }
}

#[test]
fn recipe_of_a_plain_document_is_null() {
    let mut s = picture(8);
    let v = s.execute("seeing.recipe", json!({})).unwrap();
    assert!(v["recipe"].is_null());
    assert_eq!(v["reactions"], json!([]));
    // Somebody else's note that looks like a recipe is not one.
    s.execute("notes.add", json!({"x": 1, "y": 1, "text": "recipe: {\"steps\":[]}", "author": "someone"})).unwrap();
    assert!(s.execute("seeing.recipe", json!({})).unwrap()["recipe"].is_null());
}

/// A variant saved before the product was renamed carries the earlier note author and recipe key.
/// Its recipe is still found, replays, and a new variant of it does not inherit the old notes.
#[test]
fn a_recipe_saved_under_the_earlier_product_name_is_still_read() {
    let mut s = picture(8);
    let text = r#"recipe: {"photocraftSeeing":1,"label":"exposure -2 EV","kind":"underexpose","seed":3,"steps":[{"id":"layer.newAdjustmentLayer.exposure","params":{"exposure":-2}}]}"#;
    s.execute("notes.add", json!({"x": 1, "y": 1, "text": text, "author": "PhotoCraft Seeing"})).unwrap();
    let stored = s.execute("seeing.recipe", json!({})).unwrap();
    assert_eq!(stored["recipe"]["label"], "exposure -2 EV", "{stored}");
    assert_eq!(stored["recipe"]["magicpicSeeing"], 1, "written back under the current key");
    let recipe: Value = serde_json::from_str(text.trim_start_matches("recipe: ")).unwrap();
    s.execute("seeing.variant", json!({"recipe": recipe})).unwrap();
    let doc = &s.active().unwrap().doc;
    assert!(doc.notes.iter().all(|n| n.author == NOTE_AUTHOR), "the source's old note is not carried into the variant");
    assert_eq!(doc.notes.len(), 1);
}

#[test]
fn nothing_runs_without_a_document_and_bad_params_are_errors() {
    let mut s = Session::new();
    for spec in specs() {
        assert!(s.execute(spec.id, json!({})).is_err(), "{} ran without a document", spec.id);
    }
    let mut s = picture(8);
    let before = fingerprint(&s, 0);
    let bad: &[(&str, Value)] = &[
        ("seeing.view", json!({"view": "pretty"})),
        ("seeing.view", json!({"view": 3})),
        ("seeing.view", json!({"maxSide": 0})),
        ("seeing.view", json!({"maxSide": 100000})),
        ("seeing.view", json!({"maxSide": -5})),
        ("seeing.view", json!({"strength": 2})),
        ("seeing.view", json!({"strength": "a lot"})),
        ("seeing.view", json!({"levels": 1})),
        ("seeing.view", json!({"levels": 2.5})),
        ("seeing.view", json!({"quality": 0})),
        ("seeing.view", json!({"document": -1})),
        ("seeing.study", json!({"view": "lines", "maxSide": 4096})),
        ("seeing.frames", json!({"count": 0})),
        ("seeing.frames", json!({"count": 1000})),
        ("seeing.frames", json!({"seed": -1})),
        ("seeing.frames", json!({"seed": 1.5})),
        ("seeing.frames", json!({"minFraction": 0.9, "maxFraction": 0.1})),
        ("seeing.frames", json!({"aspect": 0})),
        ("seeing.frames", json!({"aspect": "wide"})),
        ("seeing.experiments", json!({"count": 13})),
        ("seeing.experiments", json!({"kinds": "defocus"})),
        ("seeing.experiments", json!({"kinds": [1]})),
        ("seeing.variant", json!({})),
        ("seeing.variant", json!({"kind": "sharpen"})),
        ("seeing.variant", json!({"kind": "defocus", "seed": "x"})),
        ("seeing.variant", json!({"kind": "defocus", "recipe": {"steps": []}})),
        ("seeing.variant", json!({"recipe": []})),
        ("seeing.variant", json!({"recipe": {"steps": "blur"}})),
        ("seeing.variant", json!({"kind": "defocus", "name": ""})),
    ];
    for (id, p) in bad {
        assert!(s.execute(id, p.clone()).is_err(), "{id} {p} was accepted");
    }
    assert_eq!(s.documents().len(), 1);
    assert_eq!(fingerprint(&s, 0), before);
}

#[test]
fn the_view_menu_lists_the_studies() {
    let specs = specs();
    let menu: Vec<&str> = specs.iter().filter(|c| c.menu == ["View", "Seeing"]).map(|c| c.label).collect();
    assert_eq!(menu, ["Tones Study", "Color Masses Study", "Shapes Study", "Lines Study", "Color Without Tone Study", "New Chance Variant"]);
    // The queries stay out of the journal (and so out of recorded actions).
    for id in ["seeing.view", "seeing.frames", "seeing.experiments", "seeing.recipe"] {
        assert!(!specs.iter().any(|c| c.id == id && c.journal), "{id}");
    }
}

#[test]
fn tiny_documents_do_not_break_chance() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 1, "height": 1})).unwrap();
    for (kind, _) in KINDS {
        // Either a variant or a clean refusal; never a panic or a stray document.
        let n = s.documents().len();
        s.set_active(0);
        match s.execute("seeing.variant", json!({"kind": kind, "seed": 9})) {
            Ok(_) => assert_eq!(s.documents().len(), n + 1),
            Err(_) => assert_eq!(s.documents().len(), n),
        }
    }
    s.set_active(0);
    s.execute("seeing.view", json!({"view": "lines"})).unwrap();
    s.execute("seeing.frames", json!({})).unwrap();
}

#[test]
fn looking_keeps_a_floating_selection_floating() {
    let mut s = picture(8);
    s.execute("select.rect", json!({"x": 10, "y": 10, "width": 16, "height": 16})).unwrap();
    s.execute("select.float", json!({"dx": 30, "dy": 0})).unwrap();
    assert!(s.active().unwrap().floating.is_some());
    let before = fingerprint(&s, 0);
    for (id, p) in [
        ("seeing.view", json!({"view": "lines"})),
        ("seeing.view", json!({})),
        ("seeing.frames", json!({})),
        ("seeing.experiments", json!({})),
        ("seeing.recipe", json!({})),
    ] {
        s.execute(id, p).unwrap();
        assert!(s.active().unwrap().floating.is_some(), "{id} dropped the floating piece");
        assert_eq!(fingerprint(&s, 0), before, "{id} edited the document");
    }
}

/// Fill the 32-bit document with linear 0.214 (mid grey on screen) and tag it linear sRGB, as an
/// opened EXR or HDR file is: the canvas then shows it through the sRGB curve.
fn linear_grey(s: &mut Session) {
    s.edit("Grey", |doc, _| {
        doc.icc_profile = Some(magicpic_cms::Builtin::LinearSrgb.profile().to_bytes());
        let fmt = doc.pixel_format();
        let b = doc.bounds();
        let vals: Vec<f32> = (0..256).flat_map(|_| magicpic_raster::from_rgba(&fmt, [0.2140, 0.2140, 0.2140, 1.0])).collect();
        let layer = doc.layers.first_mut().ok_or(EngineError::NoDocument)?;
        crate::pixels_mut(layer)?.write_region(b, &vals);
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_linear_document_is_viewed_as_it_looks_not_as_its_numbers() {
    // Mid grey in linear light is 0.214; on screen (and in a view) it is 0.5.
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 16, "height": 16, "depth": 32})).unwrap();
    linear_grey(&mut s);
    assert!(is_linear(&s, &s.active().unwrap().doc), "the canvas shows a linear-profile document through the sRGB curve");
    for view in ["normal", "luminance"] {
        let v = s.execute("seeing.view", json!({"view": view})).unwrap();
        let px = decode(&v).to_rgba8();
        assert!((126..=129).contains(&px[0]), "{view}: linear 0.214 shown as {} instead of mid grey", px[0]);
    }
    // An 8-bit document's numbers are already encoded and are left alone.
    let mut s = picture(8);
    assert!(!is_linear(&s, &s.active().unwrap().doc));
    let v = s.execute("seeing.view", json!({})).unwrap();
    assert_eq!(decode(&v).to_rgba8()[0], 38);
}

#[test]
fn a_study_of_a_linear_document_is_not_darkened() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 16, "height": 16, "depth": 32})).unwrap();
    linear_grey(&mut s);
    s.execute("seeing.study.luminance", json!({})).unwrap();
    let g = flat(&s, 1)[0][0];
    assert!((g - 0.5).abs() < 0.02, "study of a mid-grey 32-bit document is {g}");
}

#[test]
fn a_variant_starts_its_own_record() {
    let mut s = picture(8);
    // A reaction on the source belongs to the source…
    s.execute("seeing.note", json!({"reaction": "the original is calm"})).unwrap();
    s.execute("seeing.variant", json!({"kind": "overexpose", "seed": 1})).unwrap();
    let v = s.execute("seeing.recipe", json!({})).unwrap();
    assert_eq!(v["reactions"], json!([]), "the variant inherited the source's reactions");
    assert_eq!(v["recipe"]["kind"], "overexpose");
    // …and a variant of a variant has its own recipe, not its parent's as well.
    s.execute("seeing.variant", json!({"kind": "underexpose", "seed": 1})).unwrap();
    let doc = &s.active().unwrap().doc;
    assert_eq!(doc.notes.iter().filter(|n| n.author == NOTE_AUTHOR).count(), 1);
    assert_eq!(s.execute("seeing.recipe", json!({})).unwrap()["recipe"]["kind"], "underexpose");
    s.set_active(0);
    assert_eq!(s.execute("seeing.recipe", json!({})).unwrap()["reactions"].as_array().unwrap().len(), 1);
}

#[test]
fn a_step_that_opens_another_document_discards_the_variant_and_the_stray() {
    let mut s = picture(8);
    let before = fingerprint(&s, 0);
    let journal = s.journal.len();
    // Duplicate Layer to a new document is an ordinary layer command by name.
    let recipe = json!({"steps": [{"id": "layer.newAdjustmentLayer.invert"}, {"id": "layer.duplicate", "params": {"document": "new"}}]});
    let e = s.execute("seeing.variant", json!({"recipe": recipe})).unwrap_err().to_string();
    assert!(e.contains("step 2 of 2") && e.contains("discarded"), "{e}");
    assert_eq!(s.documents().len(), 1, "a stray document stayed open");
    assert_eq!(s.active_index(), Some(0));
    assert_eq!(fingerprint(&s, 0), before);
    assert_eq!(s.journal.len(), journal, "the journal kept the discarded variant's commands");
    // The same step in a transaction is rolled back the same way.
    let e = s.execute("document.transaction", json!({"steps": recipe["steps"]})).unwrap_err().to_string();
    assert!(e.contains("another document") && e.contains("rolled back"), "{e}");
    assert_eq!(s.documents().len(), 1);
    // The document is as it was; only its revision moved on, so every view of it redraws.
    let after = fingerprint(&s, 0);
    assert!(after.0 > before.0);
    assert_eq!((after.1, after.2, after.3), (before.1, before.2.clone(), before.3));
}

#[test]
fn misspelt_parameters_are_errors() {
    let mut s = picture(8);
    for (id, p) in [
        ("seeing.view", json!({"veiw": "lines"})),
        ("seeing.view", json!({"max_side": 100})),
        ("seeing.study", json!({"view": "lines", "region": {"x": 0, "y": 0, "width": 4, "height": 4}})),
        ("seeing.study.lines", json!({"view": "color"})),
        ("seeing.frames", json!({"number": 3})),
        ("seeing.experiments", json!({"kind": "defocus"})),
        ("seeing.variant", json!({"kind": "defocus", "sed": 3})),
        ("seeing.variant.chance", json!({"seed": 3})),
        ("seeing.recipe", json!({"document": 0})),
        ("seeing.note", json!({"reaction": "ok", "why": "because"})),
    ] {
        let e = s.execute(id, p.clone()).unwrap_err().to_string();
        assert!(e.contains("unknown parameter"), "{id} {p}: {e}");
    }
    assert_eq!(s.documents().len(), 1);
}
