//! Layout packs: themed scene layouts that frame the footage under them, and the smart
//! transitions between them ([`filmcraft_project::layoutpack`]).
//!
//! - **Library** (`layout.pack.*`): built-in packs are original designs in code; user packs live
//!   in `<data dir>/Layout Packs/*.fcpack`. Install copies a pack file there, Export writes any
//!   pack (also a built-in one) as a file, Remix saves a pack's layouts under new colours and
//!   fonts, and `layout.save` adds a graphic clip to a user pack as a layout.
//! - **Scenes** (`layout.scenes`): the stretches of the sequence a layout is applied to: between
//!   chapter markers, or clip by clip on the lowest track with footage when there are none.
//! - **Apply** (`layout.apply`): places a layout clip over a scene, on the track that holds the
//!   sequence's layouts (above everything, added when needed), replacing what that track had
//!   there. The layout's media windows are pointed at the tracks under it that hold the scene's
//!   footage, by role (`camera`, `screen`): a `map` parameter, then track and clip names, then
//!   track order decide. Text the replaced layout showed is kept where the new one has the same
//!   property.
//! - **Smart transitions** (`layout.transition`): how a layout clip opens when it follows
//!   another one.
//! - **Retheme** (`layout.retheme`): restyles the layout clips already in the sequence.

use std::collections::BTreeMap;

use filmcraft_geom::Affine;
use filmcraft_project::graphic::{self, MEDIA_LAYER, MEDIA_SOURCE_OPTS, TEXT_LAYER};
use filmcraft_project::gtemplate::{ControlKind, TemplateControl, base64_encode, ensure_uids, layer_uid, slug};
use filmcraft_project::layoutpack::{
    Design, Easing, LAYOUT_KINDS, Layout, LayoutLink, LayoutPack, PACK_EXTENSION, SmartTransition, Theme, ThemeBinding, ThemeFont, apply_theme, builtin_packs,
    parse_hex_color,
};
use filmcraft_project::{ClipId, EffectInstance, ItemKind, MarkerKind, ParamValue, Project, Sequence, Track, TrackId, TrackItem, TrackKind};
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, has_seq, str_p, time_p, u64_p};
use crate::graphics::{apply_props, graphic_source, target_clip};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

fn spec(id: &'static str, label: &'static str, menu: &'static [&'static str], params: &'static str, enabled: Enabled, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut: None, params, enabled, run, journal: true }
}
fn query(id: &'static str, label: &'static str, params: &'static str, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false }
}

const MENU: &[&str] = &["Graphics and Titles", "Layouts"];
/// The name given to the track that is added for layout clips.
pub const LAYOUT_TRACK: &str = "Layouts";
/// A pack file larger than this is not read.
const MAX_PACK_BYTES: u64 = 64 << 20;

// ------------------------------------------------------------------------------------------------
// Library
// ------------------------------------------------------------------------------------------------

/// Where user layout packs live: `<data dir>/Layout Packs` (None without a data directory). The
/// data directory is the one the preferences are in; a headless session without preferences (the
/// CLI, the MCP server) uses the one its user libraries were pointed at, so packs installed in
/// the app are there for agents too.
pub fn user_packs_dir(s: &Session) -> Option<std::path::PathBuf> {
    let data = s.prefs_path.as_ref().and_then(|p| p.parent()).or_else(|| s.export_presets.data_dir())?;
    Some(data.join("Layout Packs"))
}

/// A pack from the library: built in, or a user file.
#[derive(Clone, Debug)]
pub struct PackEntry {
    pub pack: LayoutPack,
    /// None for built-in packs.
    pub path: Option<String>,
}

fn is_pack_path(key: &str) -> bool {
    key.to_ascii_lowercase().ends_with(&format!(".{PACK_EXTENSION}")) || key.contains('/') || key.contains('\\')
}

fn read_pack(s: &Session, path: &str) -> Result<LayoutPack> {
    let err = |e: String| EngineError::Other(format!("{path}: {e}"));
    if s.services.file_size(path).map_err(|e| err(e.to_string()))? > MAX_PACK_BYTES {
        return Err(err("too large to be a layout pack".into()));
    }
    let b = s.services.read_file(path).map_err(|e| err(e.to_string()))?;
    LayoutPack::from_bytes(&b).map_err(|e| err(e.to_string()))
}

/// Built-in and user packs (user files that fail to read are skipped).
pub fn library(s: &Session) -> Vec<PackEntry> {
    let mut v: Vec<PackEntry> = builtin_packs().into_iter().map(|pack| PackEntry { pack, path: None }).collect();
    if let Some(dir) = user_packs_dir(s)
        && let Some(Ok(mut names)) = s.services.list_dir(&dir.to_string_lossy())
    {
        names.sort();
        for n in names.into_iter().filter(|n| n.to_ascii_lowercase().ends_with(&format!(".{PACK_EXTENSION}"))) {
            let path = dir.join(&n).to_string_lossy().to_string();
            if let Ok(pack) = read_pack(s, &path) {
                v.push(PackEntry { pack, path: Some(path) });
            }
        }
    }
    v
}

/// Find a pack by id, name (ignoring case) or `.fcpack` path.
pub fn find_pack(s: &Session, key: &str) -> Result<PackEntry> {
    if is_pack_path(key) {
        return Ok(PackEntry { pack: read_pack(s, key)?, path: Some(key.to_string()) });
    }
    let lib = library(s);
    lib.iter()
        .find(|e| e.pack.id == key)
        .or_else(|| lib.iter().find(|e| e.pack.name.eq_ignore_ascii_case(key)))
        .cloned()
        .ok_or_else(|| bad("layout", format!("no layout pack `{key}`")))
}

fn find_layout<'a>(pack: &'a LayoutPack, key: &str) -> Result<&'a Layout> {
    pack.layout(key).ok_or_else(|| bad("layout", format!("the pack `{}` has no layout `{key}`", pack.name)))
}

fn control_json(c: &TemplateControl) -> Value {
    json!({"id": c.id, "name": c.name, "kind": c.kind.name(), "layer": c.layer, "param": c.param, "min": c.min, "max": c.max})
}

fn layout_json(l: &Layout) -> Value {
    json!({
        "id": l.id,
        "name": l.name,
        "kind": l.kind,
        "description": l.description,
        "duration": l.duration.0,
        "windows": l.media_roles(),
        "controls": l.controls.iter().map(control_json).collect::<Vec<_>>(),
        "shapes": l.designs.iter().map(|d| d.canvas).collect::<Vec<_>>(),
    })
}

fn transition_json(t: &SmartTransition) -> Value {
    json!({"enabled": t.enabled, "duration": t.duration.0, "seconds": t.duration.seconds(), "easing": t.easing.name(), "fades": t.fades})
}

fn pack_json(e: &PackEntry) -> Value {
    let p = &e.pack;
    json!({
        "id": p.id,
        "name": p.name,
        "description": p.description,
        "author": p.author,
        "license": p.license,
        "tags": p.tags,
        "source": if e.path.is_some() { "user" } else { "builtin" },
        "path": e.path,
        "theme": p.theme,
        "transition": transition_json(&p.transition),
        "layouts": p.layouts.iter().map(layout_json).collect::<Vec<_>>(),
    })
}

fn list_packs(s: &mut Session, p: &Value) -> Result<Value> {
    let q = str_p(p, "query").unwrap_or("").to_ascii_lowercase();
    let kind = str_p(p, "kind").map(str::to_ascii_lowercase);
    let src = str_p(p, "source");
    let hit = |e: &PackEntry| {
        let p = &e.pack;
        q.is_empty()
            || [&p.name, &p.description, &p.id].iter().any(|x| x.to_ascii_lowercase().contains(&q))
            || p.tags.iter().any(|x| x.to_ascii_lowercase().contains(&q))
            || p.layouts.iter().any(|l| l.name.to_ascii_lowercase().contains(&q) || l.kind.to_ascii_lowercase().contains(&q))
    };
    let packs: Vec<Value> = library(s)
        .iter()
        .filter(|e| match src {
            Some("builtin") => e.path.is_none(),
            Some("user") => e.path.is_some(),
            _ => true,
        })
        .filter(|e| hit(e))
        .filter(|e| kind.as_ref().is_none_or(|k| e.pack.layouts.iter().any(|l| l.kind.eq_ignore_ascii_case(k))))
        .map(pack_json)
        .collect();
    Ok(json!({"packs": packs, "kinds": LAYOUT_KINDS, "folder": user_packs_dir(s).map(|d| d.to_string_lossy().to_string())}))
}

/// Write `pack` into the user folder (as `<slug of its name>.fcpack`); returns the path.
fn save_user_pack(s: &Session, cmd: &str, pack: &LayoutPack) -> Result<String> {
    pack.validate().map_err(|e| EngineError::Other(e.to_string()))?;
    let dir = user_packs_dir(s).ok_or_else(|| bad(cmd, "no data directory for layout packs"))?;
    let _ = std::fs::create_dir_all(&dir);
    let dest = dir.join(format!("{}.{PACK_EXTENSION}", slug(&pack.name))).to_string_lossy().to_string();
    s.services.write_file(&dest, pack.to_json().as_bytes()).map_err(|e| EngineError::Other(format!("{dest}: {e}")))?;
    Ok(dest)
}

fn user_id(name: &str) -> String {
    format!("user:{}", slug(name))
}

fn install_pack(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "layout.pack.install";
    let path = str_p(p, "path").ok_or_else(|| bad(ID, "need `path`"))?.to_string();
    let mut pack = read_pack(s, &path)?;
    if !pack.id.starts_with("user:") {
        pack.id = user_id(&pack.name);
    }
    let dest = save_user_pack(s, ID, &pack)?;
    let fonts = crate::graphic_templates::register_font_resources(&pack.resources);
    s.toast(format!("Installed layout pack \"{}\"", pack.name));
    Ok(json!({"id": pack.id, "name": pack.name, "path": dest, "layouts": pack.layouts.len(), "fontsRegistered": fonts}))
}

fn remove_pack(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "layout.pack.remove";
    let key = str_p(p, "pack").ok_or_else(|| bad(ID, "need `pack`"))?;
    let e = find_pack(s, key)?;
    let path = e.path.ok_or_else(|| bad(ID, "built-in layout packs cannot be removed"))?;
    std::fs::remove_file(&path).map_err(|x| EngineError::Other(format!("{path}: {x}")))?;
    Ok(json!({"removed": e.pack.id}))
}

fn export_pack(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "layout.pack.export";
    let key = str_p(p, "pack").ok_or_else(|| bad(ID, "need `pack`"))?;
    let path = str_p(p, "path").ok_or_else(|| bad(ID, "need `path`"))?.to_string();
    let pack = find_pack(s, key)?.pack;
    s.services.write_file(&path, pack.to_json().as_bytes()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    Ok(json!({"id": pack.id, "name": pack.name, "path": path, "layouts": pack.layouts.len()}))
}

/// `colors` ({name: "#rrggbb"}) and `fonts` ({name: {family, style} | "Family"}) of a command as
/// a theme to lay over another.
fn theme_p(cmd: &str, p: &Value) -> Result<Theme> {
    let mut t = Theme::default();
    if let Some(o) = p.get("colors").and_then(Value::as_object) {
        for (k, v) in o {
            let hex = v.as_str().filter(|x| parse_hex_color(x).is_some()).ok_or_else(|| bad(cmd, format!("colour `{k}` must be \"#rrggbb\"")))?;
            t.colors.insert(k.clone(), hex.to_string());
        }
    }
    if let Some(o) = p.get("fonts").and_then(Value::as_object) {
        for (k, v) in o {
            let (family, style) = match v {
                Value::String(f) => (f.clone(), "Regular".to_string()),
                Value::Object(f) => {
                    (f.get("family").and_then(Value::as_str).unwrap_or("").to_string(), f.get("style").and_then(Value::as_str).unwrap_or("Regular").to_string())
                }
                _ => (String::new(), String::new()),
            };
            if family.trim().is_empty() {
                return Err(bad(cmd, format!("font `{k}` needs a family")));
            }
            t.fonts.insert(k.clone(), ThemeFont { family, style });
        }
    }
    Ok(t)
}

fn remix_pack(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "layout.pack.remix";
    let key = str_p(p, "pack").ok_or_else(|| bad(ID, "need `pack`"))?;
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(ID, "need `name` for the new pack"))?;
    let base = find_pack(s, key)?.pack;
    let over = theme_p(ID, p)?;
    let known = |have: Vec<&String>| have.into_iter().cloned().collect::<Vec<_>>().join(", ");
    if let Some(k) = over.colors.keys().find(|k| !base.theme.colors.contains_key(*k)) {
        return Err(bad(ID, format!("the pack has no colour `{k}` (it has: {})", known(base.theme.colors.keys().collect()))));
    }
    if let Some(k) = over.fonts.keys().find(|k| !base.theme.fonts.contains_key(*k)) {
        return Err(bad(ID, format!("the pack has no font `{k}` (it has: {})", known(base.theme.fonts.keys().collect()))));
    }
    let mut pack = base.remix(&user_id(name), name, &over);
    if let Some(d) = str_p(p, "description") {
        pack.description = d.to_string();
    }
    pack.author = str_p(p, "author").unwrap_or("").to_string();
    if let Some(t) = p.get("transition") {
        pack.transition = transition_p(ID, s, t, &pack.transition)?;
    }
    let dest = save_user_pack(s, ID, &pack)?;
    s.toast(format!("Saved layout pack \"{}\"", pack.name));
    Ok(json!({"id": pack.id, "name": pack.name, "path": dest, "theme": pack.theme}))
}

// ------------------------------------------------------------------------------------------------
// Thumbnails
// ------------------------------------------------------------------------------------------------

/// A layout drawn `w` pixels wide on a `shape`-proportioned frame (its media windows show their
/// fill): `(width, height, RGBA8)`.
pub fn thumbnail(pack: &LayoutPack, layout: &Layout, shape: (u32, u32), w: u32) -> (u32, u32, Vec<u8>) {
    let w = w.clamp(16, 1920);
    let design = layout.design_for(shape).map_or([1920, 1080], |d| d.canvas);
    let canvas = (design[0].max(1), design[1].max(1));
    let h = ((w as f64 * canvas.1 as f64 / canvas.0 as f64).round() as u32).clamp(1, 8192);
    let blank = || (w, h, vec![0; w as usize * h as usize * 4]);
    crate::graphic_templates::register_font_resources(&pack.resources);
    let Some((mut layers, _)) = pack.instantiate(layout, canvas) else { return blank() };
    // a window has no picture here: a soft ramp over its fill says "footage goes here"
    for e in layers.iter_mut().filter(|e| e.effect == MEDIA_LAYER) {
        let base = e.params.get("fill_color").and_then(|p| p.value.as_color()).unwrap_or([0.2, 0.2, 0.22, 1.0]);
        let toward = |c: f32, k: f32| c + (0.5 - c) * k;
        let light = [toward(base[0], 0.45), toward(base[1], 0.45), toward(base[2], 0.45), 1.0];
        for (k, v) in [
            ("fill_kind", ParamValue::Choice(1)),
            ("gradient_start", ParamValue::Color(light)),
            ("gradient_end", ParamValue::Color(base)),
            ("gradient_angle", ParamValue::Float(55.0)),
        ] {
            e.params.insert(k.into(), filmcraft_project::Param::new(v));
        }
    }
    let mut img = filmcraft_render::image::Image::new(w as usize, h as usize);
    let k = w as f64 / canvas.0 as f64;
    filmcraft_render::graphic_clip::render_graphic_layers(&layers, Tick::ZERO, canvas, &Affine::scale(k, k), &mut img);
    (w, h, img.to_rgba8())
}

/// [`thumbnail`] through a cache keyed by the pack's and the layout's content, the shape and the
/// width (the UI asks every frame).
pub fn thumbnail_cached(pack: &LayoutPack, layout: &Layout, shape: (u32, u32), w: u32) -> std::sync::Arc<(u32, u32, Vec<u8>)> {
    type Key = (String, String, u64, (u32, u32), u32);
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<Key, std::sync::Arc<(u32, u32, Vec<u8>)>>>> = std::sync::OnceLock::new();
    let fnv = |h: u64, text: &str| text.bytes().fold(h, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    let content = fnv(fnv(0xcbf2_9ce4_8422_2325, &serde_json::to_string(&pack.theme).unwrap_or_default()), &serde_json::to_string(layout).unwrap_or_default());
    let key = (pack.id.clone(), layout.id.clone(), content, shape, w);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return v.clone();
    }
    let v = std::sync::Arc::new(thumbnail(pack, layout, shape, w));
    let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
    if c.len() > 512 {
        c.clear();
    }
    c.insert(key, v.clone());
    v
}

fn shape_p(s: &Session, p: &Value) -> (u32, u32) {
    let seq = s.active_sequence().map(|q| (q.settings.width, q.settings.height));
    match p.get("shape").and_then(Value::as_array).map(|a| (a.first().and_then(Value::as_u64), a.get(1).and_then(Value::as_u64))) {
        Some((Some(w), Some(h))) if (1..=16384).contains(&w) && (1..=16384).contains(&h) => (w as u32, h as u32),
        _ => seq.unwrap_or((1920, 1080)),
    }
}

fn layout_thumbnail(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "layout.thumbnail";
    let pack = find_pack(s, str_p(p, "pack").ok_or_else(|| bad(ID, "need `pack`"))?)?.pack;
    let layout = find_layout(&pack, str_p(p, "layout").ok_or_else(|| bad(ID, "need `layout`"))?)?;
    let (w, h, rgba) = thumbnail(&pack, layout, shape_p(s, p), u64_p(p, "width").unwrap_or(480).min(1920) as u32);
    let png = filmcraft_export::encode_png(rgba, w, h).map_err(|x| EngineError::Other(x.to_string()))?;
    if let Some(path) = str_p(p, "path") {
        s.services.write_file(path, &png).map_err(|x| EngineError::Other(format!("{path}: {x}")))?;
        return Ok(json!({"width": w, "height": h, "path": path}));
    }
    Ok(json!({"width": w, "height": h, "pngBase64": base64_encode(&png)}))
}

// ------------------------------------------------------------------------------------------------
// Scenes
// ------------------------------------------------------------------------------------------------

/// A stretch of a sequence that takes one layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub start: Tick,
    pub end: Tick,
    pub name: String,
}

fn is_layout_clip(it: &TrackItem) -> bool {
    it.graphic.as_deref().is_some_and(|g| g.layout.is_some())
}

fn is_graphic_item(p: &Project, it: &TrackItem) -> bool {
    p.item(it.item).is_some_and(|i| matches!(i.kind, ItemKind::Graphic { .. }))
}

/// Whether a clip is footage a layout can frame: media, a subclip or a nested sequence.
fn is_footage(p: &Project, it: &TrackItem) -> bool {
    it.enabled && p.item(it.item).is_some_and(|i| matches!(i.kind, ItemKind::Media(_) | ItemKind::Subclip { .. } | ItemKind::Sequence(_)))
}

/// The scenes of a sequence: the stretches between its chapter markers (from the sequence start
/// to its end); without chapter markers, the clips of the lowest video track that holds footage,
/// one scene each; without footage, the whole sequence.
pub fn scenes(p: &Project, seq: &Sequence) -> Vec<Scene> {
    let end = seq.duration();
    if end.0 <= 0 {
        return Vec::new();
    }
    let mut chapters: Vec<(Tick, &str)> =
        seq.markers.iter().filter(|m| m.kind == MarkerKind::Chapter && m.start.0 > 0 && m.start < end).map(|m| (m.start, m.name.as_str())).collect();
    chapters.sort_by_key(|c| c.0);
    chapters.dedup_by_key(|c| c.0);
    if !chapters.is_empty() {
        let first_name = seq.markers.iter().find(|m| m.kind == MarkerKind::Chapter && m.start.0 <= 0).map_or("", |m| m.name.as_str());
        let mut out = Vec::with_capacity(chapters.len() + 1);
        let mut at = (Tick::ZERO, first_name);
        for c in chapters {
            out.push(Scene { start: at.0, end: c.0, name: at.1.to_string() });
            at = c;
        }
        out.push(Scene { start: at.0, end, name: at.1.to_string() });
        return out;
    }
    if let Some(track) = seq.video_tracks.iter().find(|tr| tr.items.iter().any(|it| is_footage(p, it))) {
        return track
            .items
            .iter()
            .filter(|it| is_footage(p, it) && it.duration.0 > 0)
            .map(|it| Scene { start: it.start, end: it.end(), name: it.name.clone() })
            .collect();
    }
    vec![Scene { start: Tick::ZERO, end, name: String::new() }]
}

/// The layout clip covering `t`, on the topmost track that has one.
fn layout_clip_at(seq: &Sequence, t: Tick) -> Option<(usize, &TrackItem)> {
    seq.video_tracks.iter().enumerate().rev().find_map(|(i, tr)| tr.item_at(t).filter(|it| is_layout_clip(it)).map(|it| (i, it)))
}

fn list_scenes(s: &mut Session, _: &Value) -> Result<Value> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = seq.settings.frame_rate;
    let list: Vec<Value> = scenes(&s.project, seq)
        .iter()
        .enumerate()
        .map(|(i, sc)| {
            let layout = layout_clip_at(seq, sc.start).map(|(ti, it)| {
                let link = it.graphic.as_deref().and_then(|g| g.layout.as_ref());
                json!({"clip": it.id.0, "track": ti, "pack": link.map(|l| l.pack.clone()), "layout": link.map(|l| l.layout.clone())})
            });
            json!({
                "scene": i,
                "name": sc.name,
                "start": sc.start.0,
                "end": sc.end.0,
                "startSeconds": sc.start.seconds(),
                "endSeconds": sc.end.seconds(),
                "frames": rate.frame_at(sc.end) - rate.frame_at(sc.start),
                "layout": layout,
            })
        })
        .collect();
    Ok(json!({"scenes": list, "layoutTrack": layout_track(seq)}))
}

// ------------------------------------------------------------------------------------------------
// Apply
// ------------------------------------------------------------------------------------------------

/// The video track that holds the sequence's layout clips (the topmost one with any).
fn layout_track(seq: &Sequence) -> Option<usize> {
    seq.video_tracks.iter().rposition(|tr| tr.items.iter().any(is_layout_clip))
}

const SCREEN_WORDS: &[&str] = &["screen", "capture", "desktop", "slides", "display", "demo"];
const CAMERA_WORDS: &[&str] = &["cam", "face", "speaker", "talking", "interview", "host", "presenter"];

/// The role a name suggests (`screen`, `camera`), if any.
fn role_of_name(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    if SCREEN_WORDS.iter().any(|w| n.contains(w)) {
        Some("screen")
    } else if CAMERA_WORDS.iter().any(|w| n.contains(w)) {
        Some("camera")
    } else {
        None
    }
}

/// The role of the footage on a track during `range`: from the track's name, else the name of
/// its first footage clip there, else that clip's project item.
fn track_role(p: &Project, track: &Track, range: &TimeRange) -> Option<&'static str> {
    role_of_name(&track.name).or_else(|| {
        let it = track.items.iter().find(|it| is_footage(p, it) && it.range().overlaps(range))?;
        role_of_name(&it.name).or_else(|| p.item(it.item).and_then(|i| role_of_name(&i.name)))
    })
}

/// A track named in a `map` value: a 1-based video track number, `"V2"`, or a track name.
fn track_p(seq: &Sequence, v: &Value) -> Option<usize> {
    let by_number = |n: u64| n.checked_sub(1).map(|i| i as usize).filter(|i| *i < seq.video_tracks.len());
    match v {
        Value::Number(n) => n.as_u64().and_then(by_number),
        Value::String(x) => {
            let t = x.trim();
            t.strip_prefix(['V', 'v'])
                .and_then(|d| d.parse::<u64>().ok())
                .or_else(|| t.parse::<u64>().ok())
                .and_then(by_number)
                .or_else(|| seq.video_tracks.iter().position(|tr| tr.name.eq_ignore_ascii_case(t)))
        }
        _ => None,
    }
}

fn layer_text(e: &EffectInstance, id: &str) -> String {
    match e.params.get(id).map(|p| &p.value) {
        Some(ParamValue::Text(s)) => s.clone(),
        _ => String::new(),
    }
}

/// One media window of a layout being placed and the track it was given.
struct Window {
    /// Index into the layers.
    layer: usize,
    name: String,
    role: String,
    track: Option<usize>,
}

/// Point the media windows of `layers` (a layout going onto video track `on`) at the tracks under
/// it that hold footage during `range`. Each window gets a different track: first those the
/// `map` names (by the window's role or layer name), then tracks whose role matches the
/// window's, then what is left: camera windows take the lowest free track and the others the
/// highest (footage is usually laid down with the camera first and the screen above it). A
/// window that gets none shows its fill. Returns what each window was given.
fn assign_windows(
    p: &Project,
    seq: &Sequence,
    on: usize,
    range: &TimeRange,
    layers: &mut [EffectInstance],
    map: Option<&serde_json::Map<String, Value>>,
    cmd: &str,
) -> Result<Vec<Window>> {
    let mut windows: Vec<Window> = layers
        .iter()
        .enumerate()
        .filter(|(_, e)| e.effect == MEDIA_LAYER)
        .map(|(i, e)| Window { layer: i, name: layer_text(e, "name"), role: layer_text(e, "role"), track: None })
        .collect();
    let footage = |i: usize| seq.video_tracks.get(i).is_some_and(|tr| tr.enabled && tr.items.iter().any(|it| is_footage(p, it) && it.range().overlaps(range)));
    let mut free: Vec<usize> = (0..on.min(seq.video_tracks.len())).filter(|&i| footage(i)).collect();
    if let Some(map) = map {
        for (k, v) in map {
            let track = track_p(seq, v).ok_or_else(|| bad(cmd, format!("`map`: `{k}` must name a video track (a number from 1, \"V2\", or a track name)")))?;
            if track >= on {
                return Err(bad(cmd, format!("`map`: `{k}` points at V{}, which is not under the layout (V{})", track + 1, on + 1)));
            }
            let w = windows
                .iter_mut()
                .find(|w| w.track.is_none() && (w.role.eq_ignore_ascii_case(k) || w.name.eq_ignore_ascii_case(k)))
                .ok_or_else(|| bad(cmd, format!("`map`: the layout has no window `{k}`")))?;
            w.track = Some(track);
            free.retain(|i| *i != track);
        }
    }
    // tracks whose name says what they hold
    for w in windows.iter_mut().filter(|w| w.track.is_none() && !w.role.is_empty()) {
        if let Some(pos) = free.iter().position(|&i| seq.video_tracks.get(i).and_then(|tr| track_role(p, tr, range)).is_some_and(|r| r == w.role)) {
            w.track = Some(free.remove(pos));
        }
    }
    // the rest by order, the camera first; a track with a role of its own is not handed to a
    // window that asks for another while unlabelled tracks remain
    let labelled = |i: usize| seq.video_tracks.get(i).and_then(|tr| track_role(p, tr, range)).is_some();
    let mut order: Vec<usize> = (0..windows.len()).filter(|&i| windows[i].track.is_none()).collect();
    order.sort_by_key(|&i| windows[i].role != "camera");
    for i in order {
        let camera = windows[i].role == "camera";
        let pick = |pool: &[usize]| if camera { pool.first().copied() } else { pool.last().copied() };
        let plain: Vec<usize> = free.iter().copied().filter(|&t| !labelled(t)).collect();
        if let Some(t) = pick(&plain).or_else(|| pick(&free)) {
            windows[i].track = Some(t);
            free.retain(|x| *x != t);
        }
    }
    for w in &windows {
        let Some(e) = layers.get_mut(w.layer) else { continue };
        match w.track {
            Some(t) => {
                let n = on - t;
                if n >= MEDIA_SOURCE_OPTS.len() {
                    return Err(bad(
                        cmd,
                        format!("V{} is {n} tracks under the layout: a window reaches at most {} tracks down", t + 1, MEDIA_SOURCE_OPTS.len() - 1),
                    ));
                }
                e.params.insert("source".into(), filmcraft_project::Param::new(ParamValue::Choice(n as u32)));
            }
            // No footage for this window: aim it under the bottom track, where there is nothing,
            // so that it shows its fill (or hide it when the stack is too tall to aim past).
            None => match on + 1 {
                n if n < MEDIA_SOURCE_OPTS.len() => {
                    e.params.insert("source".into(), filmcraft_project::Param::new(ParamValue::Choice(n as u32)));
                }
                _ => e.enabled = false,
            },
        }
    }
    Ok(windows)
}

/// A control's value on placed layers (text, or the layer's visibility), for carrying what a
/// replaced layout said into the new one.
fn text_control_value(layers: &[EffectInstance], c: &TemplateControl) -> Option<Value> {
    let e = layers.iter().find(|e| graphic::is_layer(e) && layer_uid(e) == c.layer)?;
    match c.kind {
        ControlKind::Text => match e.params.get(&c.param).map(|p| &p.value) {
            Some(ParamValue::Text(t)) => Some(json!(t)),
            _ => None,
        },
        ControlKind::Checkbox if c.param == "enabled" => Some(json!(e.enabled)),
        _ => None,
    }
}

/// Set control `values` ({control id or name: value}) on `layers`.
fn set_control_values(cmd: &str, layers: &mut [EffectInstance], controls: &[TemplateControl], values: &serde_json::Map<String, Value>) -> Result<usize> {
    let mut n = 0;
    for (k, v) in values {
        let c = controls.iter().find(|c| c.id == *k).or_else(|| controls.iter().find(|c| c.name.eq_ignore_ascii_case(k))).ok_or_else(|| {
            bad(cmd, format!("the layout has no property `{k}` (it has: {})", controls.iter().map(|c| c.id.as_str()).collect::<Vec<_>>().join(", ")))
        })?;
        let Some(e) = layers.iter_mut().find(|e| graphic::is_layer(e) && layer_uid(e) == c.layer) else { continue };
        let mut props = serde_json::Map::new();
        match (c.kind, v) {
            (ControlKind::Font, Value::Object(o)) => {
                if let Some(f) = o.get("family") {
                    props.insert("font".into(), f.clone());
                }
                if let Some(st) = o.get("style") {
                    props.insert("font_style".into(), st.clone());
                }
            }
            (ControlKind::Slider, Value::Number(x)) => {
                let (lo, hi) = (c.min.unwrap_or(f64::MIN), c.max.unwrap_or(f64::MAX));
                let x = x.as_f64().unwrap_or(0.0);
                props.insert(c.param.clone(), json!(if lo <= hi { x.clamp(lo, hi) } else { x }));
            }
            _ => {
                props.insert(c.param.clone(), v.clone());
            }
        }
        apply_props(e, &props, Tick::ZERO).map_err(|e| bad(cmd, format!("`{k}`: {e}")))?;
        n += 1;
    }
    Ok(n)
}

/// A smart transition from command parameters laid over `base`: `false` / `true`, or
/// `{enabled, seconds | frames | duration (ticks), easing, fades}`.
fn transition_p(cmd: &str, s: &Session, v: &Value, base: &SmartTransition) -> Result<SmartTransition> {
    let mut t = base.clone();
    match v {
        Value::Null => {}
        Value::Bool(on) => t.enabled = *on,
        Value::Object(_) => {
            if let Some(on) = bool_p(v, "enabled") {
                t.enabled = on;
            }
            let dur = if let Some(f) = f64_p(v, "seconds") {
                Some(if f.is_finite() { Tick::from_seconds_f64(f.clamp(0.0, 600.0)) } else { return Err(bad(cmd, "`seconds` must be a number")) })
            } else if let Some(n) = v.get("frames").and_then(Value::as_i64) {
                Some(s.sequence_rate().tick_of(n.clamp(0, 100_000)))
            } else {
                v.get("duration").and_then(Value::as_i64).map(Tick)
            };
            if let Some(d) = dur {
                t.duration = Tick(d.0.clamp(0, 600 * filmcraft_time::TICKS_PER_SECOND));
            }
            if let Some(e) = str_p(v, "easing") {
                t.easing = Easing::from_name(e).ok_or_else(|| {
                    bad(cmd, format!("unknown easing `{e}` (one of: {})", Easing::ALL.iter().map(|e| e.name()).collect::<Vec<_>>().join(", ")))
                })?;
            }
            if let Some(f) = bool_p(v, "fades") {
                t.fades = f;
            }
        }
        _ => return Err(bad(cmd, "`transition` is true, false or {enabled, seconds, easing, fades}")),
    }
    Ok(t)
}

/// Clear `range` on a layout track: graphic clips wholly inside it go, those that reach into it
/// are trimmed (one that spans it is cut in two), and transitions touching it go. Anything else
/// in the way is an error.
fn clear_range(p: &mut Project, track: &mut Track, range: &TimeRange, cmd: &str, on: usize) -> Result<()> {
    let (t0, t1) = (range.start, range.end());
    if let Some(it) = track.items.iter().find(|it| it.range().overlaps(range) && !is_graphic_item(p, it)) {
        return Err(bad(cmd, format!("\"{}\" is in the way on V{}: layouts go on a track of their own (pass `track`)", it.name, on + 1)));
    }
    let mut kept = Vec::with_capacity(track.items.len() + 1);
    for mut it in std::mem::take(&mut track.items) {
        let (a, b) = (it.start, it.end());
        if b <= t0 || a >= t1 {
            kept.push(it);
        } else if a < t0 && b > t1 {
            // the range falls inside this clip: keep both ends
            let mut tail = it.clone();
            tail.id = ClipId(p.alloc_id());
            tail.source_in = it.source_time_at(t1);
            tail.start = t1;
            tail.duration = b - t1;
            it.duration = t0 - a;
            kept.push(it);
            kept.push(tail);
        } else if a < t0 {
            it.duration = t0 - a;
            kept.push(it);
        } else if b > t1 {
            it.source_in = it.source_time_at(t1);
            it.start = t1;
            it.duration = b - t1;
            kept.push(it);
        }
    }
    track.items = kept;
    let ids: Vec<ClipId> = track.items.iter().map(|it| it.id).collect();
    track.transitions.retain(|x| !x.range().overlaps(range) && x.from.is_none_or(|c| ids.contains(&c)) && x.to.is_none_or(|c| ids.contains(&c)));
    track.sort();
    Ok(())
}

/// Where a layout goes: the range of an existing layout `clip`, a `scene` number, `start…` /
/// `end…` times, the In/Out points, or the scene under the playhead.
fn range_p(s: &Session, seq: &Sequence, p: &Value, cmd: &str) -> Result<(TimeRange, Option<(usize, ClipId)>)> {
    let rate = seq.settings.frame_rate;
    if let Some(c) = u64_p(p, "clip").map(ClipId) {
        let (ti, it) = seq
            .video_tracks
            .iter()
            .enumerate()
            .find_map(|(i, tr)| tr.item(c).map(|it| (i, it)))
            .filter(|(_, it)| is_graphic_item(&s.project, it))
            .ok_or_else(|| bad(cmd, "`clip` is not a graphic clip of this sequence"))?;
        return Ok((it.range(), Some((ti, c))));
    }
    let list = scenes(&s.project, seq);
    let range = if let Some(n) = u64_p(p, "scene") {
        let sc = list.get(n as usize).ok_or_else(|| bad(cmd, format!("there is no scene {n} (the sequence has {})", list.len())))?;
        (sc.start, sc.end)
    } else if let (Some(a), Some(b)) = (time_p(s, p, "start"), time_p(s, p, "end")) {
        (a, b)
    } else if let (Some(a), Some(b)) = (seq.mark_in, seq.mark_out) {
        (a, b)
    } else {
        let t = s.playhead();
        let sc = list
            .iter()
            .find(|sc| sc.start <= t && t < sc.end)
            .or_else(|| list.last().filter(|sc| t >= sc.end))
            .ok_or_else(|| bad(cmd, "there is nothing at the playhead to lay out: pass `scene`, or `start…` and `end…` times"))?;
        (sc.start, sc.end)
    };
    let (a, b) = (rate.snap(range.0.max(Tick::ZERO)), rate.snap(range.1));
    if b <= a {
        return Err(bad(cmd, "the range is empty"));
    }
    Ok((TimeRange::new(a, b - a), None))
}

fn apply_layout(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "layout.apply";
    let pack = find_pack(s, str_p(p, "pack").ok_or_else(|| bad(ID, "need `pack`"))?)?.pack;
    let layout = find_layout(&pack, str_p(p, "layout").ok_or_else(|| bad(ID, "need `layout`"))?)?.clone();
    crate::graphic_templates::register_font_resources(&pack.resources);
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (range, replacing) = range_p(s, seq, p, ID)?;
    let (w, h, rate) = (seq.settings.width, seq.settings.height, seq.settings.frame_rate);
    let want_track = match p.get("track").filter(|v| !v.is_null()) {
        Some(v) => Some(track_p(seq, v).ok_or_else(|| bad(ID, "`track` must name a video track (a number from 1, \"V2\", or a track name)"))?),
        None => None,
    };
    let (mut layers, mut meta) = pack.instantiate(&layout, (w, h)).ok_or_else(|| bad(ID, "the layout has no design"))?;
    if let Some(t) = p.get("transition") {
        meta.smart = Some(transition_p(ID, s, t, &pack.transition)?);
    }
    let values = p.get("values").and_then(Value::as_object).cloned();
    let map = p.get("map").and_then(Value::as_object).cloned();
    let keep_text = bool_p(p, "keepText").unwrap_or(true);
    let name = layout.name.clone();
    let controls = layout.controls.clone();
    let (clip, on, windows, added) = s.edit("Apply Layout", move |pr, st| {
        let src = graphic_source(pr, w, h, rate);
        let new_track = TrackId(pr.alloc_id());
        let q = pr.sequence(seq_id).ok_or(EngineError::NoSequence)?;
        // the track: the replaced clip's, the one asked for, the one that already holds layouts,
        // or a new one on top
        let (on, added) = match replacing.map(|(ti, _)| ti).or(want_track).or_else(|| layout_track(q)) {
            Some(i) => (i, false),
            None => (q.video_tracks.len(), true),
        };
        if !added && q.video_tracks.get(on).is_none_or(|tr| tr.locked) {
            return Err(bad(ID, format!("V{} is locked or does not exist", on + 1)));
        }
        // what the layout it replaces said, for the properties the new one shares
        let mut carried = serde_json::Map::new();
        if keep_text
            && let Some(old) = q.video_tracks.get(on).and_then(|tr| tr.item_at(range.start)).filter(|it| is_layout_clip(it))
            && let Some(old_controls) = old.graphic.as_deref().and_then(|g| g.template.as_ref()).map(|t| &t.controls)
        {
            for c in old_controls.iter().filter(|c| controls.iter().any(|n| n.id == c.id && n.kind == c.kind)) {
                if let Some(v) = text_control_value(&old.effects, c) {
                    carried.insert(c.id.clone(), v);
                }
            }
        }
        let windows = assign_windows(pr, q, on, &range, &mut layers, map.as_ref(), ID)?;
        set_control_values(ID, &mut layers, &controls, &carried)?;
        if let Some(v) = &values {
            set_control_values(ID, &mut layers, &controls, v)?;
        }
        let mut ti =
            pr.make_track_item(src, TrackKind::Video, range.start, TimeRange::new(Tick::ZERO, range.duration), rate).ok_or_else(|| bad(ID, "bad item"))?;
        ti.name = name;
        ti.effects.extend(layers);
        for e in &mut ti.effects {
            filmcraft_project::resolve_auto_points(e, (w, h), (w, h));
        }
        ti.graphic = Some(Box::new(meta));
        let id = ti.id;
        // a track is taken out of the project while it is edited (clearing needs fresh clip ids)
        let mut track = if added {
            Track::new(new_track, TrackKind::Video, LAYOUT_TRACK.into())
        } else {
            let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
            let tr = q.video_tracks.get_mut(on).ok_or(EngineError::NoSequence)?;
            std::mem::replace(tr, Track::new(TrackId(0), TrackKind::Video, String::new()))
        };
        let cleared = clear_range(pr, &mut track, &range, ID, on);
        if cleared.is_ok() {
            track.items.push(ti);
            track.sort();
        }
        let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        if added {
            q.video_tracks.push(track);
        } else if let Some(slot) = q.video_tracks.get_mut(on) {
            *slot = track;
        }
        cleared?;
        q.check().map_err(EngineError::Other)?;
        st.selection = vec![id];
        st.graphic_layers = vec![0];
        Ok((id, on, windows, added))
    })?;
    s.state.graphic_layers.clear();
    let unfilled: Vec<&str> = windows.iter().filter(|x| x.track.is_none()).map(|x| x.name.as_str()).collect();
    Ok(json!({
        "clip": clip.0,
        "pack": pack.id,
        "layout": layout.id,
        "name": layout.name,
        "track": on,
        "trackAdded": added,
        "start": range.start.0,
        "end": range.end().0,
        "windows": windows.iter().map(|x| json!({"layer": x.name, "role": x.role, "track": x.track, "source": x.track.map(|t| on - t)})).collect::<Vec<_>>(),
        "unfilled": unfilled,
        "controls": layout.controls.iter().map(control_json).collect::<Vec<_>>(),
    }))
}

// ------------------------------------------------------------------------------------------------
// Smart transitions and themes on placed clips
// ------------------------------------------------------------------------------------------------

/// Every graphic clip of the active sequence that `pick` selects, edited by `f` in one undo
/// step. Returns the ids of the clips `f` changed.
fn edit_graphic_clips(s: &mut Session, label: &str, pick: impl Fn(&TrackItem) -> bool, mut f: impl FnMut(&mut TrackItem) -> bool) -> Result<Vec<u64>> {
    s.edit_sequence(label, |q, _, _| {
        let mut changed = Vec::new();
        for tr in &mut q.video_tracks {
            for it in tr.items.iter_mut().filter(|it| pick(it)) {
                if f(it) {
                    changed.push(it.id.0);
                }
            }
        }
        Ok(changed)
    })
}

fn set_transition(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "layout.transition";
    let all = bool_p(p, "all").unwrap_or(false);
    let one = if all { None } else { Some(target_clip(s, p).ok_or_else(|| bad(ID, "no graphic clip: select one, pass `clip`, or pass `all: true`"))?) };
    // validate the parameters once, against the default
    transition_p(ID, s, p, &SmartTransition::default())?;
    let settings = p.clone();
    let rate = s.sequence_rate();
    let mut after = None;
    let changed = edit_graphic_clips(
        s,
        "Smart Transition",
        |it| match one {
            Some(c) => it.id == c,
            None => is_layout_clip(it),
        },
        |it| {
            let meta = it.graphic.get_or_insert_with(Default::default);
            let base = meta.smart.clone().unwrap_or_default();
            // parsed above: the only failure left would be the same one
            let Ok(t) = parse_transition(&settings, &base, rate) else { return false };
            after = Some(t.clone());
            let was = meta.smart.replace(t);
            was != meta.smart
        },
    )?;
    Ok(json!({"clips": changed, "transition": after.as_ref().map(transition_json)}))
}

/// [`transition_p`] without a session (inside an edit): the same parameters, read with the
/// sequence's frame rate.
fn parse_transition(v: &Value, base: &SmartTransition, rate: filmcraft_time::FrameRate) -> std::result::Result<SmartTransition, ()> {
    let mut t = base.clone();
    if let Some(on) = bool_p(v, "enabled") {
        t.enabled = on;
    }
    if let Some(f) = f64_p(v, "seconds").filter(|f| f.is_finite()) {
        t.duration = Tick::from_seconds_f64(f.clamp(0.0, 600.0));
    } else if let Some(n) = v.get("frames").and_then(Value::as_i64) {
        t.duration = rate.tick_of(n.clamp(0, 100_000));
    } else if let Some(d) = v.get("duration").and_then(Value::as_i64) {
        t.duration = Tick(d.clamp(0, 600 * filmcraft_time::TICKS_PER_SECOND));
    }
    if let Some(e) = str_p(v, "easing") {
        t.easing = Easing::from_name(e).ok_or(())?;
    }
    if let Some(f) = bool_p(v, "fades") {
        t.fades = f;
    }
    Ok(t)
}

fn retheme(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "layout.retheme";
    let from = str_p(p, "pack").map(|k| find_pack(s, k)).transpose()?.map(|e| e.pack.id);
    let to = str_p(p, "to").map(|k| find_pack(s, k)).transpose()?.map(|e| e.pack);
    let over = theme_p(ID, p)?;
    if to.is_none() && over.colors.is_empty() && over.fonts.is_empty() {
        return Err(bad(ID, "need `to` (a pack whose theme to take), `colors` or `fonts`"));
    }
    // the theme each clip takes: the target pack's, or its own pack's, with the overrides on top
    let lib: BTreeMap<String, Theme> = library(s).into_iter().map(|e| (e.pack.id.clone(), e.pack.theme)).collect();
    if let Some(t) = &to {
        crate::graphic_templates::register_font_resources(&t.resources);
    }
    let mut params = 0usize;
    let changed = edit_graphic_clips(
        s,
        "Change Layout Theme",
        |it| it.graphic.as_deref().and_then(|g| g.layout.as_ref()).is_some_and(|l| from.as_ref().is_none_or(|f| *f == l.pack)),
        |it| {
            let Some(link) = it.graphic.as_deref_mut().and_then(|g| g.layout.as_mut()) else { return false };
            let base = match &to {
                Some(t) => t.theme.clone(),
                None => lib.get(&link.pack).cloned().unwrap_or_default(),
            };
            let theme = base.merged(&over);
            if let Some(t) = &to {
                link.pack = t.id.clone();
            }
            let bindings: Vec<ThemeBinding> = link.bindings.clone();
            let n = apply_theme(&mut it.effects, &bindings, &theme);
            params += n;
            n > 0
        },
    )?;
    Ok(json!({"clips": changed, "parameters": params, "pack": to.map(|t| t.id)}))
}

// ------------------------------------------------------------------------------------------------
// Saving a clip as a layout
// ------------------------------------------------------------------------------------------------

fn save_layout(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "layout.save";
    let clip = target_clip(s, p).ok_or_else(|| bad(ID, "no graphic clip: select one or pass `clip`"))?;
    let pack_name = str_p(p, "pack")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| bad(ID, "need `pack` (a user pack's name; it is made if it does not exist)"))?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, it) = seq.find_item(clip).ok_or_else(|| bad(ID, "no such clip"))?;
    let canvas = match s.project.item(it.item).map(|i| &i.kind) {
        Some(ItemKind::Graphic { width, height, .. }) => [*width, *height],
        _ => [seq.settings.width, seq.settings.height],
    };
    let link: Option<LayoutLink> = it.graphic.as_deref().and_then(|g| g.layout.clone());
    let old_controls: Vec<TemplateControl> = it.graphic.as_deref().and_then(|g| g.template.as_ref()).map(|t| t.controls.clone()).unwrap_or_default();
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| it.name.clone());
    if name.is_empty() {
        return Err(bad(ID, "need `name` for the layout"));
    }
    let mut layers: Vec<EffectInstance> = it.graphic_layers().cloned().collect();
    if layers.is_empty() {
        return Err(bad(ID, "the clip has no graphic layers"));
    }
    ensure_uids(&mut layers);
    let mut graphic = it.graphic.as_deref().cloned().unwrap_or_default();
    graphic.template = None;
    graphic.layout = None;
    graphic.smart = None;
    // the pack: an existing user pack, else a new one that starts with the theme of the pack the
    // clip came from (so its theme bindings keep working)
    let lib = library(s);
    let existing = lib.iter().find(|e| e.path.is_some() && (e.pack.id == pack_name || e.pack.name.eq_ignore_ascii_case(pack_name))).map(|e| e.pack.clone());
    let mut pack = existing.unwrap_or_else(|| LayoutPack {
        id: user_id(pack_name),
        name: pack_name.to_string(),
        theme: link.as_ref().and_then(|l| lib.iter().find(|e| e.pack.id == l.pack)).map(|e| e.pack.theme.clone()).unwrap_or_default(),
        ..Default::default()
    });
    // bindings the pack's theme can resolve; controls: the clip's own, else each text layer's text
    let bindings: Vec<ThemeBinding> = link
        .iter()
        .flat_map(|l| l.bindings.iter())
        .filter(|b| {
            layers.iter().any(|e| layer_uid(e) == b.layer)
                && match b.token.split_once('.') {
                    Some(("color", n)) => pack.theme.colors.contains_key(n),
                    Some(("font", n)) => pack.theme.fonts.contains_key(n),
                    _ => false,
                }
        })
        .cloned()
        .collect();
    let mut controls: Vec<TemplateControl> = old_controls
        .into_iter()
        .filter(|c| layers.iter().any(|e| layer_uid(e) == c.layer && (c.param == "enabled" || e.params.contains_key(&c.param))))
        .collect();
    if controls.is_empty() {
        for (i, e) in layers.iter().enumerate().filter(|(_, e)| e.effect == TEXT_LAYER) {
            let label = graphic::layer_display_name(e, i);
            let mut id = slug(&label).replace('-', "_");
            while controls.iter().any(|c| c.id == id) {
                id.push('_');
            }
            controls.push(TemplateControl { id, name: label, kind: ControlKind::Text, layer: layer_uid(e), param: "text".into(), min: None, max: None });
        }
    }
    let kind = str_p(p, "kind")
        .map(str::to_ascii_lowercase)
        .or_else(|| link.as_ref().map(|l| l.kind.clone()))
        .unwrap_or_else(|| if layers.iter().any(|e| e.effect == MEDIA_LAYER) { "media".into() } else { "title".into() });
    let id = slug(&name);
    let design = Design { canvas, layers };
    let added_shape = match pack.layouts.iter_mut().find(|l| l.id == id) {
        // the same layout again: this drawing replaces the one for its frame shape, or joins the
        // others as the drawing for a new shape
        Some(l) => {
            let same = |d: &Design| (d.canvas[0] as u64 * canvas[1] as u64) == (d.canvas[1] as u64 * canvas[0] as u64);
            let new_shape = !l.designs.iter().any(same);
            l.designs.retain(|d| !same(d));
            l.designs.push(design);
            l.name = name.clone();
            l.kind = kind.clone();
            if !new_shape || l.designs.len() == 1 {
                l.controls = controls;
                l.bindings = bindings;
                l.graphic = graphic;
            }
            new_shape
        }
        None => {
            pack.layouts.push(Layout {
                id: id.clone(),
                name: name.clone(),
                kind: kind.clone(),
                description: str_p(p, "description").unwrap_or("").to_string(),
                duration: Tick::ZERO,
                graphic,
                controls,
                bindings,
                designs: vec![design],
            });
            false
        }
    };
    let dest = save_user_pack(s, ID, &pack)?;
    s.toast(format!("Saved layout \"{name}\" to the pack \"{}\"", pack.name));
    Ok(
        json!({"pack": pack.id, "layout": id, "name": name, "kind": kind, "path": dest, "shape": canvas, "addedShape": added_shape, "layouts": pack.layouts.len()}),
    )
}

fn has_graphic(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    target_clip(s, &Value::Null).map(|_| ()).ok_or_else(|| "select a graphic clip".into())
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        query(
            "layout.pack.list",
            "List Layout Packs",
            r#"{"query":str? (search pack and layout names, descriptions, tags),"kind":"camera|media|split|title|chapter|quote|outro|overlay"?,"source":"builtin|user"?}"#,
            list_packs,
        ),
        spec("layout.pack.install", "Install Layout Pack…", MENU, r#"{"path":str (a .fcpack file)}"#, always, install_pack),
        spec("layout.pack.remove", "Remove Layout Pack", &[], r#"{"pack":id|name (a user pack)}"#, always, remove_pack),
        spec("layout.pack.export", "Export Layout Pack…", MENU, r#"{"pack":id|name,"path":str (the .fcpack file to write)}"#, always, export_pack),
        spec(
            "layout.pack.remix",
            "Remix Layout Pack…",
            MENU,
            r##"{"pack":id|name|path,"name":str (the new pack),"colors":{theme colour: "#rrggbb"}?,"fonts":{theme font: {"family":str,"style":str}|"Family"}?,"description":str?,"author":str?,"transition":{"enabled":bool,"seconds":f64,"easing":"linear|easeIn|easeOut|easeInOut|overshoot","fades":bool}?}"##,
            always,
            remix_pack,
        ),
        query(
            "layout.thumbnail",
            "Layout Thumbnail",
            r#"{"pack":id|name|path,"layout":id|name,"width":px=480,"shape":[w,h]? (default: the active sequence's frame),"path":str? (write a PNG there; else returned as pngBase64)}"#,
            layout_thumbnail,
        ),
        query("layout.scenes", "List Scenes", r#"{}"#, list_scenes),
        spec(
            "layout.apply",
            "Apply Layout",
            &[],
            r#"{"pack":id|name|path,"layout":id|name,"scene":n? | "startTime|startFrame|startSeconds":…,"endTime|endFrame|endSeconds":…? | "clip":id? (a graphic clip to replace) (default: the In/Out points, else the scene under the playhead),"values":{property id or name: value}?,"map":{window role or layer name: video track number|"V2"|track name}?,"track":number|"V3"|name? (default: the track that holds layouts, else a new top track),"transition":bool|{"enabled":bool,"seconds":f64,"easing":str,"fades":bool}?,"keepText":bool=true}"#,
            has_seq,
            apply_layout,
        ),
        spec(
            "layout.transition",
            "Smart Transition",
            &[],
            r#"{"clip":id? | "all":bool? (every layout clip of the sequence),"enabled":bool?,"seconds":f64? | "frames":n? | "duration":ticks?,"easing":"linear|easeIn|easeOut|easeInOut|overshoot"?,"fades":bool?}"#,
            has_seq,
            set_transition,
        ),
        spec(
            "layout.retheme",
            "Change Layout Theme",
            &[],
            r##"{"pack":id|name? (only clips placed from this pack),"to":id|name|path? (take this pack's theme),"colors":{theme colour: "#rrggbb"}?,"fonts":{theme font: {"family":str,"style":str}|"Family"}?}"##,
            has_seq,
            retheme,
        ),
        spec(
            "layout.save",
            "Save As Layout…",
            MENU,
            r#"{"clip":id?,"pack":str (a user pack's name or id; made if missing),"name":str? (default: the clip's name),"kind":"camera|media|split|title|chapter|quote|outro|overlay"?,"description":str?}"#,
            has_graphic,
            save_layout,
        ),
    ]
}

#[cfg(test)]
#[path = "layouts_tests.rs"]
mod tests;
