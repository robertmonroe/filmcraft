//! Media layers (windows onto the tracks below a graphic clip) and smart transitions between
//! layout clips.

use super::*;
use filmcraft_geom::Vec2;
use filmcraft_media::generators::GeneratorSource;
use filmcraft_media::{Generator, MediaSource};
use filmcraft_project::graphic::{new_media_layer, new_shape_layer};
use filmcraft_project::layoutpack::{Easing, SmartTransition, builtin_packs};
use filmcraft_project::{ClipId, EffectInstance, GraphicMeta, Label, MediaClip, MediaRef, SequenceSettings, TrackKind};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, TimeRange};

const R: FrameRate = FrameRate::FPS_24;
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

struct Scene {
    p: Project,
    seq: ItemId,
    map: SourceMap,
    graphic: ItemId,
}

/// A 320×180 sequence with `tracks` video tracks and a graphic source the size of its frame.
fn scene(tracks: usize) -> Scene {
    let mut p = Project::new("layout");
    let seq = p.new_sequence("s", SequenceSettings { width: 320, height: 180, frame_rate: R, ..Default::default() }, tracks, 1, None);
    let graphic = p.add_item("G", Label::Rose, ItemKind::Graphic { width: 320, height: 180, rate: R }, None);
    Scene { p, seq, map: SourceMap::default(), graphic }
}

impl Scene {
    /// A colour matte on `track` for frames `start..start + len`.
    fn matte(&mut self, track: usize, color: [f32; 4], start: i64, len: i64) -> ClipId {
        let g = GeneratorSource::new(Generator::ColorMatte { color }, 320, 180, R, Tick(60 * TICKS_PER_SECOND));
        let info = g.info().clone();
        let id = self.p.add_item(
            "matte",
            Label::Iris,
            ItemKind::Media(MediaClip {
                media: MediaRef::Generator(g.generator.clone()),
                info,
                interpret: Default::default(),
                mark_in: None,
                mark_out: None,
                markers: vec![],
                offline: false,
                proxy: None,
                identity: None,
            }),
            None,
        );
        self.map.0.insert(id, Arc::new(g) as SharedSource);
        self.push(track, id, start, len, vec![], None)
    }

    /// A graphic clip of `layers` on `track`.
    fn layout(&mut self, track: usize, start: i64, len: i64, layers: Vec<EffectInstance>, meta: Option<GraphicMeta>) -> ClipId {
        self.push(track, self.graphic, start, len, layers, meta)
    }

    fn push(&mut self, track: usize, item: ItemId, start: i64, len: i64, layers: Vec<EffectInstance>, meta: Option<GraphicMeta>) -> ClipId {
        let mut ti = self.p.make_track_item(item, TrackKind::Video, R.tick_of(start), TimeRange::new(Tick::ZERO, R.tick_of(len)), R).unwrap();
        ti.effects.extend(layers);
        ti.graphic = meta.map(Box::new);
        let id = ti.id;
        let s = self.p.sequence_mut(self.seq).unwrap();
        s.video_tracks[track].items.push(ti);
        s.video_tracks[track].sort();
        id
    }

    fn render(&self, frame: i64) -> Image {
        render_sequence(&self.p, self.seq, R.tick_of(frame) + Tick(1000), RenderOptions::default(), &self.map)
    }
}

fn set(e: &mut EffectInstance, k: &str, v: ParamValue) {
    e.params.get_mut(k).unwrap_or_else(|| panic!("{k}")).value = v;
}

fn named(mut e: EffectInstance, name: &str) -> EffectInstance {
    set(&mut e, "name", ParamValue::Text(name.into()));
    e
}

/// A square-cornered rectangle with corner (`x`, `y`).
fn rect(x: f64, y: f64, w: f64, h: f64, color: [f32; 4]) -> EffectInstance {
    let mut e = new_shape_layer(0, Vec2::new(x + w / 2.0, y + h / 2.0), Vec2::new(w, h), vec![]);
    set(&mut e, "fill_color", ParamValue::Color(color));
    e
}

/// A media window with corner (`x`, `y`).
fn window(source: u32, x: f64, y: f64, w: f64, h: f64) -> EffectInstance {
    new_media_layer(source, Vec2::new(x + w / 2.0, y + h / 2.0), Vec2::new(w, h))
}

fn near(c: [f32; 4], want: [f32; 4]) -> bool {
    c.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.02)
}

#[test]
fn windows_show_the_tracks_below() {
    let mut s = scene(3);
    s.matte(0, RED, 0, 48);
    s.matte(1, BLUE, 0, 48);
    // background, the track directly below (blue) on the left, the one under it (red) on the right
    s.layout(2, 0, 48, vec![rect(0.0, 0.0, 320.0, 180.0, GREEN), window(1, 20.0, 40.0, 120.0, 100.0), window(2, 180.0, 40.0, 120.0, 100.0)], None);
    let img = s.render(10);
    assert!(near(img.get(80, 90), BLUE), "left window: {:?}", img.get(80, 90));
    assert!(near(img.get(240, 90), RED), "right window: {:?}", img.get(240, 90));
    assert!(near(img.get(160, 90), GREEN), "between the windows: {:?}", img.get(160, 90));
    assert!(near(img.get(5, 5), GREEN) && near(img.get(315, 175), GREEN));
    // box edges are where they were drawn
    assert!(near(img.get(22, 90), BLUE) && near(img.get(17, 90), GREEN));
    assert!(near(img.get(137, 42), BLUE) && near(img.get(143, 42), GREEN));
}

#[test]
fn all_tracks_below_is_their_composite_and_lower_tracks_still_show_around() {
    let mut s = scene(3);
    s.matte(0, RED, 0, 48);
    // the upper track only has a clip later on
    s.matte(1, BLUE, 24, 24);
    s.layout(2, 0, 48, vec![window(0, 100.0, 50.0, 120.0, 80.0)], None);
    // no background: the tracks below show around the window as usual
    let early = s.render(10);
    assert!(near(early.get(160, 90), RED) && near(early.get(10, 10), RED));
    let late = s.render(30);
    assert!(near(late.get(160, 90), BLUE) && near(late.get(10, 10), BLUE));
    // on the bottom track there is nothing below: the window shows its own fill
    let mut alone = scene(1);
    let mut w = window(0, 100.0, 50.0, 120.0, 80.0);
    set(&mut w, "fill_color", ParamValue::Color(GREEN));
    alone.layout(0, 0, 48, vec![w], None);
    let img = alone.render(10);
    assert!(near(img.get(160, 90), GREEN) && img.get(10, 10)[3] < 0.01);
}

#[test]
fn an_opaque_background_skips_the_hidden_tracks_without_changing_the_picture() {
    let build = |alpha: f32| {
        let mut s = scene(3);
        s.matte(0, RED, 0, 48);
        s.matte(1, BLUE, 0, 48);
        s.layout(
            2,
            0,
            48,
            vec![rect(0.0, 0.0, 320.0, 180.0, [0.0, 1.0, 0.0, alpha]), window(2, 40.0, 30.0, 100.0, 60.0), window(0, 180.0, 90.0, 100.0, 60.0)],
            None,
        );
        s
    };
    let (opaque, nearly) = (build(1.0), build(0.9999));
    let (seq_o, seq_n) = (opaque.p.sequence(opaque.seq).unwrap(), nearly.p.sequence(nearly.seq).unwrap());
    let t = R.tick_of(10);
    assert_eq!(first_visible_track(&opaque.p, seq_o, t, RenderOptions::default(), 3), 2);
    assert_eq!(first_visible_track(&nearly.p, seq_n, t, RenderOptions::default(), 3), 0);
    let (a, b) = (opaque.render(10), nearly.render(10));
    for (x, y) in [(90, 60), (230, 120), (160, 20), (5, 170)] {
        let (ca, cb) = (a.get(x, y), b.get(x, y));
        assert!(ca.iter().zip(cb).all(|(p, q)| (p - q).abs() < 0.01), "({x},{y}): {ca:?} vs {cb:?}");
    }
    assert!(near(a.get(90, 60), RED) && near(a.get(230, 120), BLUE) && near(a.get(160, 20), GREEN));
    // a rounded background does not count as covering
    let mut s = build(1.0);
    let it = s.p.sequence_mut(s.seq).unwrap().video_tracks[2].items.first_mut().unwrap();
    it.effects.iter_mut().find(|e| e.effect == "graphic_shape").unwrap().params.get_mut("corner_radius").unwrap().value = ParamValue::Float(8.0);
    assert_eq!(first_visible_track(&s.p, s.p.sequence(s.seq).unwrap(), t, RenderOptions::default(), 3), 0);
}

#[test]
fn fit_shows_the_whole_frame_with_the_fill_around_it() {
    let mut s = scene(2);
    s.matte(0, RED, 0, 48);
    // a 100×100 box: a 16:9 frame fitted in it is 100×56.25, centred
    let mut w = window(1, 110.0, 40.0, 100.0, 100.0);
    set(&mut w, "fit", ParamValue::Choice(1));
    set(&mut w, "fill_color", ParamValue::Color(GREEN));
    s.layout(1, 0, 48, vec![rect(0.0, 0.0, 320.0, 180.0, BLUE), w], None);
    let img = s.render(10);
    assert!(near(img.get(160, 90), RED), "{:?}", img.get(160, 90));
    assert!(near(img.get(160, 50), GREEN) && near(img.get(160, 130), GREEN), "bars: {:?}", img.get(160, 50));
    assert!(near(img.get(160, 70), RED) && near(img.get(160, 110), RED));
    assert!(near(img.get(100, 90), BLUE));
}

#[test]
fn rounded_corners_borders_and_opacity() {
    let mut s = scene(2);
    s.matte(0, RED, 0, 48);
    let mut w = window(1, 60.0, 30.0, 200.0, 120.0);
    set(&mut w, "corner_radius", ParamValue::Float(40.0));
    set(&mut w, "stroke", ParamValue::Bool(true));
    set(&mut w, "stroke_color", ParamValue::Color([1.0, 1.0, 1.0, 1.0]));
    set(&mut w, "stroke_width", ParamValue::Float(6.0));
    s.layout(1, 0, 48, vec![rect(0.0, 0.0, 320.0, 180.0, BLUE), w], None);
    let img = s.render(10);
    assert!(near(img.get(160, 90), RED));
    // the corner of the box is outside the rounded window
    assert!(near(img.get(62, 32), BLUE), "{:?}", img.get(62, 32));
    // the outer stroke runs along the edge
    assert!(near(img.get(160, 27), [1.0, 1.0, 1.0, 1.0]), "{:?}", img.get(160, 27));
    // half opacity mixes the window with what is behind it
    let it = s.p.sequence_mut(s.seq).unwrap().video_tracks[1].items.first_mut().unwrap();
    it.effects.iter_mut().find(|e| e.effect == "graphic_media").unwrap().params.get_mut("opacity").unwrap().value = ParamValue::Float(50.0);
    let c = s.render(10).get(160, 90);
    assert!((c[0] - 0.5).abs() < 0.03 && (c[2] - 0.5).abs() < 0.03, "{c:?}");
}

#[test]
fn half_resolution_and_the_gpu_plan_agree_with_the_reference() {
    let mut s = scene(3);
    s.matte(0, RED, 0, 48);
    s.matte(1, BLUE, 0, 48);
    s.layout(2, 0, 48, vec![rect(0.0, 0.0, 320.0, 180.0, GREEN), window(1, 20.0, 40.0, 120.0, 100.0), window(2, 180.0, 40.0, 120.0, 100.0)], None);
    let t = R.tick_of(10);
    let half = render_sequence(&s.p, s.seq, t, RenderOptions { scale: 0.5, ..Default::default() }, &s.map);
    assert_eq!((half.w, half.h), (160, 90));
    assert!(near(half.get(40, 45), BLUE) && near(half.get(120, 45), RED) && near(half.get(80, 45), GREEN));
    // the GPU compositor is handed the CPU's picture for such a frame
    let full = render_sequence(&s.p, s.seq, t, RenderOptions::default(), &s.map);
    match plan::plan_frame(&s.p, s.seq, t, RenderOptions::default(), &s.map) {
        plan::FramePlan::Image(img) => assert_eq!(img.px, full.px),
        _ => panic!("a frame with media layers must be composited by the CPU"),
    }
    // a frame without them still plans as layers
    assert!(matches!(plan::plan_frame(&s.p, s.seq, R.tick_of(60), RenderOptions::default(), &s.map), plan::FramePlan::Layers { .. }));
}

#[test]
fn a_track_window_shows_the_clip_itself_not_its_place_in_the_frame() {
    // 16:9 footage in a tall sequence sits letterboxed in the frame...
    let mut p = Project::new("tall");
    let seq = p.new_sequence("s", SequenceSettings { width: 180, height: 320, frame_rate: R, ..Default::default() }, 2, 1, None);
    let graphic = p.add_item("G", Label::Rose, ItemKind::Graphic { width: 180, height: 320, rate: R }, None);
    let mut s = Scene { p, seq, map: SourceMap::default(), graphic };
    let clip = s.matte(0, RED, 0, 48);
    // (scaled to fit the frame's width, as Scale to Frame Size would)
    let q = s.p.sequence_mut(s.seq).unwrap();
    q.find_item_mut(clip).unwrap().1.effect_mut("motion").unwrap().params.get_mut("scale").unwrap().value = ParamValue::Float(56.25);
    let mut all = window(0, 0.0, 0.0, 180.0, 320.0);
    set(&mut all, "fill_color", ParamValue::Color(GREEN));
    let below = s.layout(1, 0, 48, vec![all], None);
    // ...which is what "all tracks below" shows: the frame as it is, bars and all
    let img = s.render(10);
    assert!(near(img.get(90, 160), RED) && near(img.get(90, 20), GREEN), "{:?}", img.get(90, 20));
    // ...while "track below 1" shows the clip's own picture, which then fills the window
    let q = s.p.sequence_mut(s.seq).unwrap();
    q.find_item_mut(below).unwrap().1.effects.iter_mut().find(|e| e.effect == "graphic_media").unwrap().params.get_mut("source").unwrap().value =
        ParamValue::Choice(1);
    let img = s.render(10);
    assert!(near(img.get(90, 160), RED) && near(img.get(90, 20), RED) && near(img.get(5, 315), RED), "{:?}", img.get(90, 20));
    // the clip's opacity still applies to its picture
    q_set_opacity(&mut s, clip, 50.0);
    let c = s.render(10).get(90, 20);
    assert!((c[0] - 0.5).abs() < 0.03 && (c[1] - 0.5).abs() < 0.03, "{c:?}");
}

fn q_set_opacity(s: &mut Scene, clip: ClipId, v: f64) {
    let q = s.p.sequence_mut(s.seq).unwrap();
    q.find_item_mut(clip).unwrap().1.effect_mut("opacity").unwrap().params.get_mut("opacity").unwrap().value = ParamValue::Float(v);
}

fn smart(seconds: f64, easing: Easing, fades: bool) -> Option<GraphicMeta> {
    Some(GraphicMeta { smart: Some(SmartTransition { enabled: true, duration: Tick::from_seconds_f64(seconds), easing, fades }), ..Default::default() })
}

#[test]
fn smart_transition_moves_the_window_and_fades_the_rest() {
    let mut s = scene(2);
    s.matte(0, RED, 0, 96);
    // A: the window fills the frame. B: a blue background and the window in a 160×90 box at the centre.
    s.layout(1, 0, 24, vec![named(window(1, 0.0, 0.0, 320.0, 180.0), "Speaker")], None);
    let b = s.layout(
        1,
        24,
        48,
        vec![named(rect(0.0, 0.0, 320.0, 180.0, BLUE), "Background"), named(window(1, 80.0, 45.0, 160.0, 90.0), "Speaker")],
        smart(1.0, Easing::Linear, true),
    );
    // before the cut and after the transition: the plain layouts
    assert!(near(s.render(20).get(10, 10), RED));
    let settled = s.render(60);
    assert!(near(settled.get(10, 10), BLUE) && near(settled.get(160, 90), RED) && near(settled.get(70, 90), BLUE));
    // half way (frame 36 of a 24-frame transition from 24): the window is 240×135 at the centre
    let mid = s.render(36);
    assert!(near(mid.get(160, 90), RED));
    assert!(near(mid.get(50, 90), RED), "inside the half-way box: {:?}", mid.get(50, 90));
    // outside it the background is half faded in over the track below
    let c = mid.get(20, 90);
    assert!(c[0] > 0.3 && c[0] < 0.7 && c[2] > 0.3 && c[2] < 0.7, "{c:?}");
    // the first frame of B looks like the last frame of A
    let first = s.render(24);
    assert!(first.get(20, 90)[2] < 0.05, "{:?}", first.get(20, 90));
    // with the transition off the layout cuts
    let q = s.p.sequence_mut(s.seq).unwrap();
    q.find_item_mut(b).unwrap().1.graphic.as_mut().unwrap().smart.as_mut().unwrap().enabled = false;
    assert!(near(s.render(24).get(20, 90), BLUE));
}

#[test]
fn smart_transition_needs_an_adjacent_layout_clip_and_changes_the_preview_hash() {
    let mut s = scene(2);
    s.matte(0, RED, 0, 96);
    // a gap before B: nothing to come from
    let a = s.layout(1, 0, 20, vec![named(window(1, 0.0, 0.0, 320.0, 180.0), "Speaker")], None);
    s.layout(1, 24, 48, vec![named(rect(0.0, 0.0, 320.0, 180.0, BLUE), "Background")], smart(1.0, Easing::Linear, true));
    assert!(near(s.render(24).get(20, 90), BLUE));
    // closing the gap starts the transition, and the cached preview of B's segment is no longer valid
    let before = preview::video_segments(&s.p, s.seq);
    s.p.sequence_mut(s.seq).unwrap().find_item_mut(a).unwrap().1.duration = R.tick_of(24);
    assert!(s.render(24).get(20, 90)[2] < 0.05);
    let after = preview::video_segments(&s.p, s.seq);
    let seg = |v: &[preview::Segment]| preview::segment_at(v, 30).map(|x| x.hash.clone()).unwrap();
    assert_ne!(seg(&before), seg(&after));
    // editing the outgoing clip changes B's segment too
    let q = s.p.sequence_mut(s.seq).unwrap();
    q.find_item_mut(a).unwrap().1.effects.iter_mut().find(|e| e.effect == "graphic_media").unwrap().params.get_mut("corner_radius").unwrap().value =
        ParamValue::Float(30.0);
    assert_ne!(seg(&after), seg(&preview::video_segments(&s.p, s.seq)));
}

#[test]
fn hostile_layers_draw_something_and_never_panic() {
    let mut s = scene(2);
    s.matte(0, RED, 0, 48);
    let mut layers = vec![rect(0.0, 0.0, 320.0, 180.0, BLUE)];
    for (source, size, radius, zoom, focus) in [
        (8u32, (100.0, 60.0), 0.0, 100.0, (50.0, 50.0)),
        (1, (0.0, 0.0), 0.0, 100.0, (50.0, 50.0)),
        (1, (1e9, 1e9), 1e9, 1.0, (-1e9, 1e9)),
        (1, (f64::NAN, 50.0), f64::NAN, f64::NAN, (f64::NAN, f64::NAN)),
        (1, (50.0, 50.0), 25.0, 10_000.0, (0.0, 100.0)),
        (0, (-50.0, -50.0), -5.0, -100.0, (50.0, 50.0)),
    ] {
        let mut w = window(source, 60.0, 30.0, 100.0, 60.0);
        set(&mut w, "size", ParamValue::Vec2(Vec2::new(size.0, size.1)));
        set(&mut w, "corner_radius", ParamValue::Float(radius));
        set(&mut w, "zoom", ParamValue::Float(zoom));
        set(&mut w, "focus", ParamValue::Vec2(Vec2::new(focus.0, focus.1)));
        layers.push(w);
    }
    // a source index far outside the choice list
    let mut w = window(1, 10.0, 10.0, 40.0, 40.0);
    set(&mut w, "source", ParamValue::Choice(u32::MAX));
    layers.push(w);
    s.layout(1, 0, 48, layers, smart(f64::MAX, Easing::Overshoot, true));
    let img = s.render(10);
    assert_eq!((img.w, img.h), (320, 180));
    assert!(img.px.iter().all(|v| v.is_finite()));
}

#[test]
fn layouts_stacked_on_layouts_stop_at_a_fixed_depth() {
    // every track above the footage holds a full-frame window onto all the tracks below it
    let mut s = scene(12);
    s.matte(0, RED, 0, 48);
    for track in 1..12 {
        // no fill: past the depth limit a window shows nothing instead of its placeholder colour
        let clear = |mut w: EffectInstance| {
            set(&mut w, "fill", ParamValue::Bool(false));
            w
        };
        s.layout(track, 0, 48, vec![clear(window(0, 0.0, 0.0, 320.0, 180.0)), clear(window(1, 0.0, 0.0, 160.0, 90.0))], None);
    }
    let started = std::time::Instant::now();
    let img = s.render(10);
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "took {:?}", started.elapsed());
    // the footage shows through every level that was followed, and under the ones that were not
    assert!(near(img.get(300, 170), RED), "{:?}", img.get(300, 170));
}

#[test]
fn builtin_layouts_frame_footage_at_every_shape() {
    let packs = builtin_packs();
    for (w, h) in [(320u32, 180u32), (180, 320), (240, 240), (192, 240), (400, 100)] {
        for pack in &packs {
            for layout in &pack.layouts {
                let mut p = Project::new("b");
                let seq = p.new_sequence("s", SequenceSettings { width: w, height: h, frame_rate: R, ..Default::default() }, 3, 1, None);
                let g = p.add_item("G", Label::Rose, ItemKind::Graphic { width: w, height: h, rate: R }, None);
                let (layers, meta) = pack.instantiate(layout, (w, h)).unwrap();
                let mut ti = p.make_track_item(g, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, R.tick_of(48)), R).unwrap();
                ti.effects.extend(layers);
                ti.graphic = Some(Box::new(meta));
                p.sequence_mut(seq).unwrap().video_tracks[2].items.push(ti);
                let img = render_sequence(&p, seq, R.tick_of(30), RenderOptions::default(), &SourceMap::default());
                assert_eq!((img.w, img.h), (w as usize, h as usize));
                assert!(img.px.iter().all(|v| v.is_finite()), "{} / {} at {w}×{h}", pack.id, layout.id);
                // every layout draws something
                assert!(img.px.as_chunks::<4>().0.iter().any(|p| p[3] > 0.5), "{} / {} at {w}×{h} is empty", pack.id, layout.id);
            }
        }
    }
}

/// A `w`×`h` sequence with demo footage on V1 and V2 for `frames` frames, and its sources.
fn demo_scene(w: u32, h: u32, frames: i64) -> (Project, ItemId, ItemId, SourceMap) {
    let mut p = Project::new("b");
    let mut map = SourceMap::default();
    let seq = p.new_sequence("s", SequenceSettings { width: w, height: h, frame_rate: R, ..Default::default() }, 3, 1, None);
    for (track, scene) in [(0usize, filmcraft_media::DemoScene::OceanSunset), (1, filmcraft_media::DemoScene::CityNight)] {
        let g = GeneratorSource::demo(scene);
        let info = g.info().clone();
        let id = p.add_item(
            "demo",
            Label::Iris,
            ItemKind::Media(MediaClip {
                media: MediaRef::Generator(g.generator.clone()),
                info,
                interpret: Default::default(),
                mark_in: None,
                mark_out: None,
                markers: vec![],
                offline: false,
                proxy: None,
                identity: None,
            }),
            None,
        );
        map.0.insert(id, Arc::new(g) as SharedSource);
        let mut ti = p.make_track_item(id, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, R.tick_of(frames)), R).unwrap();
        ti.scale_to_frame = true;
        p.sequence_mut(seq).unwrap().video_tracks[track].items.push(ti);
    }
    let g = p.add_item("G", Label::Rose, ItemKind::Graphic { width: w, height: h, rate: R }, None);
    (p, seq, g, map)
}

fn write_ppm(name: &str, img: &Image) {
    let rgba = img.to_rgba8();
    let mut ppm = format!("P6\n{} {}\n255\n", img.w, img.h).into_bytes();
    ppm.extend(rgba.chunks_exact(4).flat_map(|c| [c[0], c[1], c[2]]));
    std::fs::write(name, ppm).unwrap_or_else(|e| panic!("{name}: {e}"));
}

/// Writes every built-in layout over demo footage, and the frames of a smart transition, as PPM
/// files into `FILMCRAFT_LAYOUT_PREVIEWS` (for looking at the designs:
/// `cargo test -p filmcraft-render layout_previews -- --ignored`).
#[test]
#[ignore = "writes pictures for review"]
fn layout_previews() {
    let Ok(dir) = std::env::var("FILMCRAFT_LAYOUT_PREVIEWS") else { return };
    let shapes: &[(u32, u32)] = &[(1920, 1080), (1080, 1920), (1080, 1080)];
    let packs = builtin_packs();
    for pack in &packs {
        for &(w, h) in shapes {
            for layout in &pack.layouts {
                let (mut p, seq, g, map) = demo_scene(w, h, 48);
                let (layers, meta) = pack.instantiate(layout, (w, h)).unwrap();
                let mut ti = p.make_track_item(g, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, R.tick_of(48)), R).unwrap();
                ti.effects.extend(layers);
                ti.graphic = Some(Box::new(meta));
                p.sequence_mut(seq).unwrap().video_tracks[2].items.push(ti);
                let img = render_sequence(&p, seq, R.tick_of(30), RenderOptions::default(), &map);
                write_ppm(&format!("{dir}/{}-{}-{w}x{h}.ppm", pack.id.replace(':', "_"), layout.id), &img);
            }
        }
    }
    // smart transitions: each layout into the next, five frames across the half second
    let studio = &packs[0];
    for (from, to) in [
        ("speaker-full", "speaker-framed"),
        ("speaker-framed", "speaker-title"),
        ("speaker-title", "screen-speaker"),
        ("screen-speaker", "screen-bubble"),
        ("screen-bubble", "quote"),
    ] {
        let (w, h) = (1280, 720);
        let (mut p, seq, g, map) = demo_scene(w, h, 96);
        for (i, id) in [from, to].into_iter().enumerate() {
            let (layers, meta) = studio.instantiate(studio.layout(id).unwrap(), (w, h)).unwrap();
            let mut ti = p.make_track_item(g, TrackKind::Video, R.tick_of(48 * i as i64), TimeRange::new(Tick::ZERO, R.tick_of(48)), R).unwrap();
            ti.effects.extend(layers);
            ti.graphic = Some(Box::new(meta));
            p.sequence_mut(seq).unwrap().video_tracks[2].items.push(ti);
        }
        for (n, frame) in [47, 50, 53, 56, 60].into_iter().enumerate() {
            let img = render_sequence(&p, seq, R.tick_of(frame), RenderOptions::default(), &map);
            write_ppm(&format!("{dir}/transition-{from}-to-{to}-{n}.ppm"), &img);
        }
    }
}
