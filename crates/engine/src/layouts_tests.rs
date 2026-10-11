use std::sync::Arc;

use filmcraft_media::{Generator, generators::GeneratorSource};
use filmcraft_project::graphic::{LayerContent, LayerSpec};
use filmcraft_project::layoutpack::{LayoutPack, parse_hex_color};
use filmcraft_project::{ClipId, ItemId, Label, TrackItem};
use filmcraft_render::graphic_clip::item_layer_specs;
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use serde_json::json;

use crate::Session;
use crate::media_test_util::tmp_dir;

const R: FrameRate = FrameRate::FPS_24;

fn add(s: &mut Session, name: &str, color: [f32; 4]) -> ItemId {
    crate::demo::add_generator(
        Arc::make_mut(&mut s.project),
        &s.media,
        GeneratorSource::new(Generator::ColorMatte { color }, 320, 180, R, Tick(60 * TICKS_PER_SECOND)),
        name,
        Label::Iris,
        None,
    )
}

/// A 320×180 sequence: V1 holds two 2 s clips of "Interview" (red) end to end, V2 a 4 s
/// "Screen capture" (blue) over both. Two scenes.
fn session(dir: &str) -> Session {
    let mut s = Session { prefs_path: Some(tmp_dir(dir).join("prefs.json")), ..Default::default() };
    s.execute("file.newSequence", json!({"width": 320, "height": 180, "fps": 24})).unwrap();
    let cam = add(&mut s, "Interview", [1.0, 0.0, 0.0, 1.0]);
    let screen = add(&mut s, "Screen capture", [0.0, 0.0, 1.0, 1.0]);
    let two = 2 * TICKS_PER_SECOND;
    s.execute("timeline.place", json!({"item": cam.0, "track": "V1", "time": 0, "duration": two, "audio": false})).unwrap();
    s.execute("timeline.place", json!({"item": cam.0, "track": "V1", "time": two, "duration": two, "audio": false})).unwrap();
    s.execute("timeline.place", json!({"item": screen.0, "track": "V2", "time": 0, "duration": 2 * two, "audio": false})).unwrap();
    s
}

fn item(s: &Session, clip: u64) -> TrackItem {
    s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1.clone()
}

fn specs(s: &Session, clip: u64) -> Vec<LayerSpec> {
    let it = item(s, clip);
    item_layer_specs(&it, it.source_in, (320, 180)).into_iter().map(|(_, sp)| sp).collect()
}

fn layer<'a>(v: &'a [LayerSpec], name: &str) -> &'a LayerSpec {
    v.iter().find(|x| x.name == name).unwrap_or_else(|| panic!("no layer {name}"))
}

fn text(v: &[LayerSpec], name: &str) -> String {
    match &layer(v, name).content {
        LayerContent::Text(t) => t.text.clone(),
        _ => panic!("{name} is not text"),
    }
}

fn source(v: &[LayerSpec], name: &str) -> u32 {
    match &layer(v, name).content {
        LayerContent::Media(m) => m.source,
        _ => panic!("{name} is not a window"),
    }
}

/// The picture at frame `f`, RGBA8, and a pixel of it.
fn pixel(s: &Session, f: i64, x: usize, y: usize) -> [u8; 4] {
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let seq = s.state.active_sequence.unwrap();
    let img = filmcraft_render::render_sequence(&s.project, seq, R.tick_of(f) + Tick(1000), Default::default(), &provider).to_rgba8();
    let i = (y * 320 + x) * 4;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

#[test]
fn library_lists_the_builtin_packs_and_draws_thumbnails() {
    let mut s = session("lp-list");
    let v = s.execute("layout.pack.list", json!({})).unwrap();
    let packs = v["packs"].as_array().unwrap();
    assert_eq!(packs.iter().map(|p| p["id"].as_str().unwrap()).collect::<Vec<_>>(), ["builtin:studio", "builtin:paper", "builtin:harbor"]);
    assert!(packs.iter().all(|p| p["source"] == "builtin" && p["layouts"].as_array().unwrap().len() == 9));
    let split = packs[0]["layouts"].as_array().unwrap().iter().find(|l| l["id"] == "screen-speaker").unwrap();
    assert_eq!(split["windows"], json!(["screen", "camera"]));
    assert_eq!(s.execute("layout.pack.list", json!({"query": "editorial"})).unwrap()["packs"].as_array().unwrap().len(), 1);
    assert_eq!(s.execute("layout.pack.list", json!({"source": "user"})).unwrap()["packs"].as_array().unwrap().len(), 0);
    assert_eq!(s.execute("layout.pack.list", json!({"kind": "nope"})).unwrap()["packs"].as_array().unwrap().len(), 0);
    // a thumbnail in the sequence's shape, and in another
    let t = s.execute("layout.thumbnail", json!({"pack": "Studio", "layout": "title", "width": 160})).unwrap();
    assert_eq!((t["width"].as_u64(), t["height"].as_u64()), (Some(160), Some(90)));
    assert!(t["pngBase64"].as_str().unwrap().len() > 100);
    let t = s.execute("layout.thumbnail", json!({"pack": "builtin:paper", "layout": "Title Card", "width": 90, "shape": [1080, 1920]})).unwrap();
    assert_eq!((t["width"].as_u64(), t["height"].as_u64()), (Some(90), Some(160)));
    assert!(s.execute("layout.thumbnail", json!({"pack": "Studio", "layout": "nope"})).is_err());
    assert!(s.execute("layout.thumbnail", json!({"pack": "nope", "layout": "title"})).is_err());
}

#[test]
fn scenes_come_from_chapters_or_from_the_footage() {
    let mut s = session("lp-scenes");
    let v = s.execute("layout.scenes", json!({})).unwrap();
    let scenes = v["scenes"].as_array().unwrap();
    assert_eq!(scenes.len(), 2, "one per clip of the lowest footage track");
    assert_eq!((scenes[0]["frames"].as_i64(), scenes[1]["startSeconds"].as_f64()), (Some(48), Some(2.0)));
    assert!(scenes[0]["layout"].is_null() && v["layoutTrack"].is_null());
    // chapter markers take over
    Arc::make_mut(&mut s.project).sequence_mut(s.state.active_sequence.unwrap()).unwrap().markers.extend([(1.0, "Body"), (3.0, "Close")].map(|(sec, name)| {
        filmcraft_project::Marker {
            id: filmcraft_project::MarkerId(sec as u64 + 9000),
            start: Tick::from_seconds_f64(sec),
            duration: Tick::ZERO,
            name: name.into(),
            comment: String::new(),
            kind: filmcraft_project::MarkerKind::Chapter,
            color: Label::Iris,
        }
    }));
    let v = s.execute("layout.scenes", json!({})).unwrap();
    let got: Vec<(f64, f64, &str)> = v["scenes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["startSeconds"].as_f64().unwrap(), x["endSeconds"].as_f64().unwrap(), x["name"].as_str().unwrap()))
        .collect();
    assert_eq!(got, [(0.0, 1.0, ""), (1.0, 3.0, "Body"), (3.0, 4.0, "Close")]);
}

#[test]
fn apply_frames_the_scene_on_a_layout_track_and_points_windows_at_the_footage() {
    let mut s = session("lp-apply");
    let undo = s.history.undo.len();
    let v = s.execute("layout.apply", json!({"pack": "Studio", "layout": "screen-speaker", "scene": 0})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    assert_eq!((v["track"].as_u64(), v["trackAdded"].as_bool()), (Some(3), Some(true)), "a new track above the sequence's three");
    assert_eq!((v["start"].as_i64(), v["end"].as_i64()), (Some(0), Some(2 * TICKS_PER_SECOND)));
    assert_eq!(v["unfilled"], json!([]));
    let seq = s.active_sequence().unwrap();
    assert_eq!(seq.video_tracks[3].name, "Layouts");
    let clip = v["clip"].as_u64().unwrap();
    assert_eq!(s.state.selection, vec![ClipId(clip)]);
    // roles decide: "Screen capture" on V2 is two tracks down, "Interview" on V1 three
    let sp = specs(&s, clip);
    assert_eq!((source(&sp, "Screen"), source(&sp, "Speaker")), (2, 3));
    let meta = item(&s, clip).graphic.unwrap();
    let link = meta.layout.as_ref().unwrap();
    assert_eq!((link.pack.as_str(), link.layout.as_str()), ("builtin:studio", "screen-speaker"));
    assert!(meta.smart.as_ref().unwrap().active());
    // the picture: blue in the screen's window, red in the speaker's, the pack's background between
    assert_eq!(pixel(&s, 10, 100, 90), [0, 0, 255, 255]);
    assert_eq!(pixel(&s, 10, 280, 90), [255, 0, 0, 255]);
    let bg = pixel(&s, 10, 4, 4);
    assert!(bg[0] < 30 && bg[1] < 30 && bg[2] < 40, "{bg:?}");
    // the next scene goes on the same track, end to end with the first
    let v2 = s.execute("layout.apply", json!({"pack": "Studio", "layout": "speaker-framed", "scene": 1, "values": {"heading": "Second"}})).unwrap();
    assert_eq!((v2["track"].as_u64(), v2["trackAdded"].as_bool()), (Some(3), Some(false)));
    let seq = s.active_sequence().unwrap();
    assert_eq!(seq.video_tracks.len(), 4);
    assert_eq!(seq.video_tracks[3].items.len(), 2);
    assert_eq!(seq.video_tracks[3].items[0].end(), seq.video_tracks[3].items[1].start);
    let sp2 = specs(&s, v2["clip"].as_u64().unwrap());
    assert_eq!(text(&sp2, "Heading"), "Second");
    // one window, a camera: it takes the lowest footage track
    assert_eq!(source(&sp2, "Speaker"), 3);
    // scenes now report their layouts
    let sc = s.execute("layout.scenes", json!({})).unwrap();
    assert_eq!(sc["layoutTrack"], json!(3));
    assert_eq!(sc["scenes"][1]["layout"]["layout"], json!("speaker-framed"));
    // undo takes the second layout away, and the track with the first
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_sequence().unwrap().video_tracks.len(), 3);
}

#[test]
fn applying_again_replaces_the_layout_and_keeps_what_it_said() {
    let mut s = session("lp-swap");
    let a = s
        .execute(
            "layout.apply",
            json!({"pack": "Studio", "layout": "speaker-framed", "scene": 0, "values": {"heading": "Why it matters", "subheading": "Dana Fox"}}),
        )
        .unwrap();
    // the same scene, another layout that has `heading` and `subheading` too
    let b = s.execute("layout.apply", json!({"pack": "Studio", "layout": "speaker-title", "scene": 0})).unwrap();
    let track = &s.active_sequence().unwrap().video_tracks[3];
    assert_eq!(track.items.len(), 1);
    assert_ne!(a["clip"], b["clip"]);
    let sp = specs(&s, b["clip"].as_u64().unwrap());
    assert_eq!((text(&sp, "Heading"), text(&sp, "Subheading")), ("Why it matters".into(), "Dana Fox".into()));
    // `clip` replaces that clip in place; `keepText: false` starts from the layout's own words
    let c = s.execute("layout.apply", json!({"pack": "Paper", "layout": "title", "clip": b["clip"], "keepText": false})).unwrap();
    let sp = specs(&s, c["clip"].as_u64().unwrap());
    assert_eq!(text(&sp, "Heading"), "Getting started");
    assert_eq!(layer(&sp, "Background").appearance.fill, parse_hex_color("#f4f1ea"));
    assert_eq!(item(&s, c["clip"].as_u64().unwrap()).range(), item(&s, s.active_sequence().unwrap().video_tracks[0].items[0].id.0).range());
    // a range inside a layout clip cuts it in two around the new one
    let mid = s.execute("layout.apply", json!({"pack": "Studio", "layout": "chapter", "startSeconds": 0.5, "endSeconds": 1.0})).unwrap();
    let track = &s.active_sequence().unwrap().video_tracks[3];
    let spans: Vec<(i64, i64)> = track.items.iter().map(|it| (R.frame_at(it.start), R.frame_at(it.end()))).collect();
    assert_eq!(spans, [(0, 12), (12, 24), (24, 48)]);
    assert_eq!(track.items[1].id.0, mid["clip"].as_u64().unwrap());
    assert!(s.active_sequence().unwrap().check().is_ok());
}

#[test]
fn map_and_track_parameters_and_windows_without_footage() {
    let mut s = session("lp-map");
    // the other way round from what the names suggest
    let v = s.execute("layout.apply", json!({"pack": "Studio", "layout": "screen-speaker", "scene": 0, "map": {"screen": "V1", "Speaker": 2}})).unwrap();
    let sp = specs(&s, v["clip"].as_u64().unwrap());
    assert_eq!((source(&sp, "Screen"), source(&sp, "Speaker")), (3, 2));
    assert_eq!(pixel(&s, 10, 100, 90), [255, 0, 0, 255]);
    s.execute("edit.undo", json!({})).unwrap();
    // on V3 (empty) by name: one track above the screen recording
    let v = s.execute("layout.apply", json!({"pack": "Studio", "layout": "screen-bubble", "scene": 1, "track": "V3"})).unwrap();
    assert_eq!((v["track"].as_u64(), v["trackAdded"].as_bool()), (Some(2), Some(false)));
    let sp = specs(&s, v["clip"].as_u64().unwrap());
    assert_eq!((source(&sp, "Screen"), source(&sp, "Speaker")), (1, 2));
    s.execute("edit.undo", json!({})).unwrap();
    // with only one track of footage, the second window has nothing to show and says so
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[1].items.clear();
    let v = s.execute("layout.apply", json!({"pack": "Studio", "layout": "screen-speaker", "scene": 0})).unwrap();
    assert_eq!(v["unfilled"], json!(["Screen"]));
    let sp = specs(&s, v["clip"].as_u64().unwrap());
    assert_eq!(source(&sp, "Speaker"), 3);
    // the empty window shows its fill (the pack's surface colour), not the speaker again
    let px = pixel(&s, 10, 100, 90);
    assert!(px[0] < 60 && px[2] < 70, "{px:?}");
    assert_eq!(pixel(&s, 10, 280, 90), [255, 0, 0, 255]);
}

#[test]
fn hostile_parameters_are_errors_and_change_nothing() {
    let mut s = session("lp-hostile");
    let before = (s.project.to_json(), s.history.undo.len());
    for p in [
        json!({}),
        json!({"pack": "Studio"}),
        json!({"pack": "nope", "layout": "title"}),
        json!({"pack": "Studio", "layout": "nope"}),
        json!({"pack": "Studio", "layout": "title", "scene": 99}),
        json!({"pack": "Studio", "layout": "title", "scene": u64::MAX}),
        json!({"pack": "Studio", "layout": "title", "startSeconds": 3.0, "endSeconds": 1.0}),
        json!({"pack": "Studio", "layout": "title", "startSeconds": 1.0, "endSeconds": 1.0}),
        json!({"pack": "Studio", "layout": "title", "clip": 424242}),
        json!({"pack": "Studio", "layout": "title", "scene": 0, "track": 99}),
        json!({"pack": "Studio", "layout": "title", "scene": 0, "track": "V0"}),
        json!({"pack": "Studio", "layout": "title", "scene": 0, "track": "V1"}),
        json!({"pack": "Studio", "layout": "title", "scene": 0, "values": {"nope": "x"}}),
        json!({"pack": "Studio", "layout": "title", "scene": 0, "values": {"heading_size": "big"}}),
        json!({"pack": "Studio", "layout": "screen-speaker", "scene": 0, "map": {"screen": 99}}),
        json!({"pack": "Studio", "layout": "screen-speaker", "scene": 0, "map": {"nope": 1}}),
        json!({"pack": "Studio", "layout": "screen-speaker", "scene": 0, "map": {"screen": "V9"}}),
        json!({"pack": "Studio", "layout": "title", "scene": 0, "transition": "fast"}),
        json!({"pack": "Studio", "layout": "title", "scene": 0, "transition": {"easing": "bounce"}}),
        json!({"pack": "/nonexistent/x.fcpack", "layout": "title", "scene": 0}),
    ] {
        assert!(s.execute("layout.apply", p.clone()).is_err(), "{p}");
        assert_eq!((s.project.to_json(), s.history.undo.len()), before, "{p}");
    }
    // extreme but valid numbers are clamped, not refused
    let v = s.execute("layout.apply", json!({"pack": "Studio", "layout": "title", "scene": 0, "transition": {"seconds": 1e300, "fades": false}})).unwrap();
    let t = item(&s, v["clip"].as_u64().unwrap()).graphic.unwrap().smart.unwrap();
    assert_eq!((t.duration, t.fades), (Tick(600 * TICKS_PER_SECOND), false));
    for (cmd, p) in [
        ("layout.transition", json!({"clip": 424242})),
        ("layout.transition", json!({"all": true, "easing": "bounce"})),
        ("layout.retheme", json!({})),
        ("layout.retheme", json!({"colors": {"accent": "red"}})),
        ("layout.retheme", json!({"to": "nope"})),
        ("layout.pack.remix", json!({"pack": "Studio"})),
        ("layout.pack.remix", json!({"pack": "Studio", "name": "X", "colors": {"nope": "#ffffff"}})),
        ("layout.pack.remix", json!({"pack": "Studio", "name": "X", "fonts": {"heading": 5}})),
        ("layout.pack.install", json!({"path": "/nonexistent/x.fcpack"})),
        ("layout.pack.remove", json!({"pack": "Studio"})),
        ("layout.pack.export", json!({"pack": "Studio"})),
        ("layout.save", json!({"clip": 424242, "pack": "Mine"})),
    ] {
        assert!(s.execute(cmd, p.clone()).is_err(), "{cmd} {p}");
    }
}

#[test]
fn transitions_and_themes_change_the_clips_in_the_sequence() {
    let mut s = session("lp-theme");
    let a = s.execute("layout.apply", json!({"pack": "Studio", "layout": "speaker-full", "scene": 0})).unwrap();
    let b = s.execute("layout.apply", json!({"pack": "Studio", "layout": "speaker-framed", "scene": 1})).unwrap();
    let (a, b) = (a["clip"].as_u64().unwrap(), b["clip"].as_u64().unwrap());
    // the second layout opens with a smart transition: half way through its half second the
    // speaker's window is between the full frame and its framed box, so a point just inside the
    // frame's edge still shows the footage
    assert_eq!(pixel(&s, 46, 4, 90), [255, 0, 0, 255]);
    let early = pixel(&s, 51, 4, 90);
    let settled = pixel(&s, 80, 4, 90);
    assert!(settled[0] < 30, "background once settled: {settled:?}");
    assert!(early[0] > 100, "still mostly footage three frames in: {early:?}");
    // one clip
    let v = s.execute("layout.transition", json!({"clip": b, "seconds": 1.0, "easing": "linear", "fades": false})).unwrap();
    assert_eq!(v["clips"], json!([b]));
    let t = item(&s, b).graphic.unwrap().smart.unwrap();
    assert_eq!((t.duration, t.easing.name(), t.fades, t.enabled), (Tick(TICKS_PER_SECOND), "linear", false, true));
    // all of them, off: the layout cuts
    let v = s.execute("layout.transition", json!({"all": true, "enabled": false})).unwrap();
    assert_eq!(v["clips"].as_array().unwrap().len(), 2);
    assert!(pixel(&s, 51, 4, 90)[0] < 30);
    assert!(!item(&s, a).graphic.unwrap().smart.unwrap().enabled);
    // take another pack's theme, then one colour of our own
    let undo = s.history.undo.len();
    let v = s.execute("layout.retheme", json!({"to": "Paper"})).unwrap();
    assert_eq!(v["clips"].as_array().unwrap().len(), 2);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(layer(&specs(&s, b), "Background").appearance.fill, parse_hex_color("#f4f1ea"));
    assert_eq!(item(&s, b).graphic.unwrap().layout.unwrap().pack, "builtin:paper");
    let v = s.execute("layout.retheme", json!({"pack": "Paper", "colors": {"accent": "#00ff00"}})).unwrap();
    assert_eq!(v["clips"].as_array().unwrap().len(), 2);
    assert_eq!(layer(&specs(&s, a), "Accent").appearance.fill, Some([0.0, 1.0, 0.0, 1.0]));
    assert_eq!(layer(&specs(&s, b), "Background").appearance.fill, parse_hex_color("#f4f1ea"), "the rest of the theme stays");
    // nothing was placed from Harbor
    assert_eq!(s.execute("layout.retheme", json!({"pack": "Harbor", "colors": {"accent": "#0000ff"}})).unwrap()["clips"], json!([]));
}

#[test]
fn packs_are_remixed_exported_installed_saved_and_removed_as_files() {
    let mut s = session("lp-files");
    let dir = s.prefs_path.clone().unwrap().parent().unwrap().to_path_buf();
    // remix: the same layouts in our colours, as a user pack
    let v = s
        .execute(
            "layout.pack.remix",
            json!({"pack": "Studio", "name": "Lanzone Morgan", "colors": {"accent": "#c8102e", "background": "#101820"}, "fonts": {"heading": {"family": "Noto Serif", "style": "Regular"}}, "transition": {"seconds": 0.75}}),
        )
        .unwrap();
    assert_eq!(v["id"], json!("user:lanzone-morgan"));
    let path = v["path"].as_str().unwrap().to_string();
    assert!(path.ends_with("lanzone-morgan.fcpack"));
    let pack = LayoutPack::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!((pack.theme.colors["accent"].as_str(), pack.theme.colors["text"].as_str(), pack.layouts.len()), ("#c8102e", "#f5f6f8", 9));
    assert_eq!(pack.transition.duration, Tick::from_seconds_f64(0.75));
    assert_eq!(s.execute("layout.pack.list", json!({"source": "user"})).unwrap()["packs"].as_array().unwrap().len(), 1);
    // it applies like any pack, by name
    let a = s.execute("layout.apply", json!({"pack": "lanzone morgan", "layout": "title", "scene": 0})).unwrap();
    let sp = specs(&s, a["clip"].as_u64().unwrap());
    assert_eq!(layer(&sp, "Accent").appearance.fill, parse_hex_color("#c8102e"));
    assert!(matches!(&layer(&sp, "Heading").content, LayerContent::Text(t) if t.font == "Noto Serif"));
    // export a built-in pack to a file, install it from there (it becomes a user pack), remove it
    let out = dir.join("harbor-copy.fcpack").to_string_lossy().to_string();
    s.execute("layout.pack.export", json!({"pack": "Harbor", "path": out})).unwrap();
    let v = s.execute("layout.pack.install", json!({"path": out})).unwrap();
    assert_eq!((v["id"].as_str(), v["layouts"].as_u64()), (Some("user:harbor"), Some(9)));
    assert_eq!(s.execute("layout.pack.list", json!({"source": "user"})).unwrap()["packs"].as_array().unwrap().len(), 2);
    assert_eq!(s.execute("layout.pack.remove", json!({"pack": "user:harbor"})).unwrap()["removed"], json!("user:harbor"));
    // files that are not packs are refused
    let junk = dir.join("junk.fcpack");
    std::fs::write(&junk, b"PK\x03\x04 not ours").unwrap();
    assert!(s.execute("layout.pack.install", json!({"path": junk.to_string_lossy()})).unwrap_err().to_string().contains("ZIP"));
    std::fs::write(&junk, b"{\"format\":\"filmcraft.layoutPack\",\"version\":1,\"name\":\"x\"}").unwrap();
    assert!(s.execute("layout.pack.install", json!({"path": junk.to_string_lossy()})).is_err());
    // save a clip as a layout of our pack: move its accent, then save it under a new name
    let clip = a["clip"].as_u64().unwrap();
    s.execute("graphics.set", json!({"clip": clip, "layer": "Heading", "props": {"text": "Lanzone Morgan"}})).unwrap();
    let v = s.execute("layout.save", json!({"clip": clip, "pack": "Lanzone Morgan", "name": "Firm Title", "kind": "title"})).unwrap();
    assert_eq!((v["layout"].as_str(), v["layouts"].as_u64(), v["addedShape"].as_bool()), (Some("firm-title"), Some(10), Some(false)));
    let pack = LayoutPack::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
    let saved = pack.layout("firm-title").unwrap();
    assert_eq!(saved.designs[0].canvas, [320, 180]);
    assert!(!saved.bindings.is_empty() && !saved.controls.is_empty(), "theme bindings and properties come along");
    let b = s.execute("layout.apply", json!({"pack": "Lanzone Morgan", "layout": "Firm Title", "scene": 1, "keepText": false})).unwrap();
    assert_eq!(text(&specs(&s, b["clip"].as_u64().unwrap()), "Heading"), "Lanzone Morgan");
    // a new pack is made when the name is new; a plain graphic's text becomes its property
    let g = s.execute("graphics.newText", json!({"text": "Hello", "time": 10 * TICKS_PER_SECOND})).unwrap();
    s.execute("graphics.newMedia", json!({"clip": g["clip"], "source": 1, "size": [100, 60], "role": "camera", "fit": "fit"})).unwrap();
    let v = s.execute("layout.save", json!({"clip": g["clip"], "pack": "Scratch", "name": "Hello Card"})).unwrap();
    assert_eq!((v["pack"].as_str(), v["kind"].as_str()), (Some("user:scratch"), Some("media")));
    let scratch = LayoutPack::from_bytes(&std::fs::read(v["path"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(scratch.layouts[0].controls[0].param, "text");
    assert_eq!(scratch.layouts[0].media_roles(), ["camera"]);
}

#[test]
fn media_windows_are_made_and_edited_like_other_layers() {
    let mut s = session("lp-media");
    let v = s
        .execute(
            "graphics.newMedia",
            json!({"source": 2, "position": [160, 90], "size": [200, 100], "cornerRadius": 12, "name": "Cam", "time": 0, "seconds": 2}),
        )
        .unwrap();
    let clip = v["clip"].as_u64().unwrap();
    let l = s.execute("graphics.list", json!({"clip": clip})).unwrap();
    let m = &l["layers"][0];
    assert_eq!(
        (m["kind"].as_str(), m["name"].as_str(), m["media"]["source"].as_u64(), m["media"]["sourceName"].as_str()),
        (Some("media"), Some("Cam"), Some(2), Some("Track Below 2"))
    );
    assert_eq!(m["media"]["cornerRadius"], json!(12.0));
    // on V3, two tracks down is V1: the interview
    assert_eq!(pixel(&s, 10, 160, 90), [255, 0, 0, 255]);
    s.execute("graphics.set", json!({"clip": clip, "layer": 0, "props": {"source": "track below 1", "fit": "stretch", "zoom": 150}})).unwrap();
    assert_eq!(pixel(&s, 10, 160, 90), [0, 0, 255, 255]);
    let l = s.execute("graphics.list", json!({"clip": clip})).unwrap();
    assert_eq!((l["layers"][0]["media"]["fit"].as_str(), l["layers"][0]["media"]["zoom"].as_f64()), (Some("Stretch"), Some(150.0)));
    for p in [
        json!({"source": 99}),
        json!({"source": "sideways"}),
        json!({"size": [-1, 5]}),
        json!({"size": [f64::MAX, f64::MAX], "position": [1e308, 1e308], "zoom": -5}),
        json!({"fit": "squash"}),
    ] {
        let r = s.execute("graphics.newMedia", p.clone());
        if let Ok(v) = r {
            // accepted values must still draw
            let _ = pixel(&s, 10, 1, 1);
            assert!(v["clip"].as_u64().is_some());
        }
    }
}
