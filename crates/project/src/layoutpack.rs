//! Layout packs (`.fcpack`): themed sets of scene layouts that frame live video, and the smart
//! transitions between them.
//!
//! A **layout** is a graphic whose layers include media windows
//! ([`MEDIA_LAYER`](crate::graphic::MEDIA_LAYER)): rounded boxes that show the video tracks under
//! the clip. Placing a layout over a scene frames the scene's footage (speaker, screen
//! recording…) with the pack's background, type and accents; nothing is baked into the footage,
//! which stays ordinary clips on the tracks below.
//!
//! A **pack** groups layouts that share one [`Theme`]: named colours and fonts that the layouts'
//! layers are bound to ([`ThemeBinding`]). Changing the theme restyles every layout
//! ([`LayoutPack::remix`]). Each layout carries one design per frame shape it was drawn for
//! (16:9, 9:16, 1:1…): the design nearest the sequence's aspect ratio is used, scaled to the
//! frame, so one pack serves every delivery format.
//!
//! A pack file is UTF-8 JSON ([`LayoutPack`]), documented in `docs/layout-packs.md`:
//!
//! ```json
//! {
//!   "format": "filmcraft.layoutPack",
//!   "version": 1,
//!   "id": "user:my-pack", "name": "My Pack",
//!   "description": "", "author": "", "license": "CC0-1.0", "tags": [],
//!   "theme": {
//!     "colors": { "background": "#0f1115", "accent": "#ff5a36" },
//!     "fonts": { "heading": { "family": "Inter", "style": "Bold" } }
//!   },
//!   "transition": { "enabled": true, "duration": 127008000000, "easing": "easeInOut", "fades": true },
//!   "layouts": [ {
//!     "id": "speaker-framed", "name": "Speaker – Framed", "kind": "camera",
//!     "description": "", "duration": 0,
//!     "controls": [ { "id": "heading", "name": "Heading", "kind": "text", "layer": 5, "param": "text" } ],
//!     "bindings": [ { "layer": 1, "param": "fill_color", "token": "color.background" } ],
//!     "designs": [ { "canvas": [1920, 1080], "layers": [ /* graphic layers, as in a project file */ ] } ]
//!   } ]
//! }
//! ```
//!
//! **Smart transitions.** When a layout clip follows another on the same track, the incoming
//! clip can open with a smart transition ([`SmartTransition`], stored on the clip): for its
//! duration the picture is drawn from layers interpolated between the outgoing layout's last
//! frame and the incoming one ([`tween_layers`]). Layers that are the same thing in both layouts
//! (same name, or media windows with the same role) move, resize and recolour into place; the
//! others fade. It is computed when the frame is drawn, so it never goes stale when either
//! layout is edited, and it writes no keyframes.
//!
//! FilmCraft reads only its own pack format. The built-in packs ([`builtin_packs`]) are original
//! designs made for FilmCraft in code, under the project licence.

use std::collections::BTreeMap;

use filmcraft_geom::Vec2;
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};

use crate::effect::EffectInstance;
use crate::graphic::{self, Appearance, LayerContent, LayerSpec, MEDIA_LAYER, Shadow, new_media_layer, new_shape_layer, new_text_layer};
use crate::graphic_design::{GraphicMeta, LayerExtra, Pin, PinTarget};
use crate::gtemplate::{ControlKind, Resources, TemplateControl, TemplateLink, base64_decode, layer_uid};
use crate::keyframe::{Param, ParamValue};

pub const PACK_FORMAT: &str = "filmcraft.layoutPack";
pub const PACK_VERSION: u32 = 1;
pub const PACK_EXTENSION: &str = "fcpack";

/// Limits on what a pack file may hold (a pack is untrusted input).
pub const MAX_LAYOUTS: usize = 256;
pub const MAX_DESIGNS: usize = 8;
pub const MAX_LAYERS: usize = 256;
pub const MAX_THEME_ENTRIES: usize = 64;
const MAX_NAME: usize = 200;
/// Longest smart transition (and longest default layout duration): ten minutes.
const MAX_DURATION: i64 = 600 * TICKS_PER_SECOND;

/// What a layout is for (free text in the file; these are the kinds the built-in packs use and
/// the Browse filter lists).
pub const LAYOUT_KINDS: &[&str] = &["camera", "media", "split", "title", "chapter", "quote", "outro", "overlay"];

// ------------------------------------------------------------------------------------------------
// Smart transitions
// ------------------------------------------------------------------------------------------------

/// How a smart transition progresses over its duration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    #[default]
    EaseInOut,
    /// Runs slightly past the end and settles back.
    Overshoot,
}

impl Easing {
    pub const ALL: [Easing; 5] = [Easing::Linear, Easing::EaseIn, Easing::EaseOut, Easing::EaseInOut, Easing::Overshoot];

    pub fn name(self) -> &'static str {
        match self {
            Easing::Linear => "linear",
            Easing::EaseIn => "easeIn",
            Easing::EaseOut => "easeOut",
            Easing::EaseInOut => "easeInOut",
            Easing::Overshoot => "overshoot",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Easing::Linear => "Linear",
            Easing::EaseIn => "Ease In",
            Easing::EaseOut => "Ease Out",
            Easing::EaseInOut => "Ease In and Out",
            Easing::Overshoot => "Overshoot",
        }
    }

    pub fn from_name(s: &str) -> Option<Easing> {
        let k: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        Self::ALL.into_iter().find(|e| e.name().to_ascii_lowercase() == k || e.label().replace(' ', "").to_ascii_lowercase() == k)
    }

    /// Eased progress for linear progress `p` (clamped to 0..1). 0 → 0 and 1 → 1 for every
    /// curve; only [`Easing::Overshoot`] leaves 0..1 in between.
    pub fn apply(self, p: f64) -> f64 {
        let p = if p.is_finite() { p.clamp(0.0, 1.0) } else { 1.0 };
        match self {
            Easing::Linear => p,
            Easing::EaseIn => p * p * p,
            Easing::EaseOut => 1.0 - (1.0 - p).powi(3),
            Easing::EaseInOut => {
                if p < 0.5 {
                    4.0 * p * p * p
                } else {
                    1.0 - (-2.0 * p + 2.0).powi(3) / 2.0
                }
            }
            Easing::Overshoot => {
                const C1: f64 = 1.2;
                const C3: f64 = C1 + 1.0;
                1.0 + C3 * (p - 1.0).powi(3) + C1 * (p - 1.0).powi(2)
            }
        }
    }
}

/// A smart transition into a layout clip from the layout clip before it on the same track
/// (`GraphicMeta::smart`), and a pack's default for the clips it places.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SmartTransition {
    pub enabled: bool,
    /// How long the incoming clip takes to settle, from its first frame.
    pub duration: Tick,
    pub easing: Easing,
    /// Fade layers that exist in only one of the two layouts (off: they cut).
    pub fades: bool,
}

impl Default for SmartTransition {
    fn default() -> Self {
        SmartTransition { enabled: true, duration: Tick(TICKS_PER_SECOND / 2), easing: Easing::EaseInOut, fades: true }
    }
}

impl SmartTransition {
    /// Whether it does anything.
    pub fn active(&self) -> bool {
        self.enabled && self.duration.0 > 0
    }

    /// Eased progress at `u` after the incoming clip's first frame; `None` once it has settled
    /// (or when it is off).
    pub fn progress(&self, u: Tick) -> Option<f64> {
        if !self.active() || u.0 < 0 || u >= self.duration {
            return None;
        }
        Some(self.easing.apply(u.0 as f64 / self.duration.0 as f64))
    }

    fn sanitized(mut self) -> Self {
        self.duration = Tick(self.duration.0.clamp(0, MAX_DURATION));
        self
    }
}

fn lerp(a: f64, b: f64, k: f64) -> f64 {
    a + (b - a) * k
}
fn lerp32(a: f32, b: f32, k: f64) -> f32 {
    (a as f64 + (b as f64 - a as f64) * k) as f32
}
fn lerp_v(a: Vec2, b: Vec2, k: f64) -> Vec2 {
    Vec2::new(lerp(a.x, b.x, k), lerp(a.y, b.y, k))
}
fn lerp_c(a: [f32; 4], b: [f32; 4], k: f64) -> [f32; 4] {
    let mut c = [0.0; 4];
    for i in 0..4 {
        c[i] = lerp32(a[i], b[i], k).clamp(0.0, 1.0);
    }
    c
}
/// A colour and its absence meet at the same colour with no alpha.
fn lerp_opt_c(a: Option<[f32; 4]>, b: Option<[f32; 4]>, k: f64) -> Option<[f32; 4]> {
    let clear = |c: [f32; 4]| [c[0], c[1], c[2], 0.0];
    match (a, b) {
        (None, None) => None,
        (Some(a), Some(b)) => Some(lerp_c(a, b, k)),
        (Some(a), None) => Some(lerp_c(a, clear(a), k)),
        (None, Some(b)) => Some(lerp_c(clear(b), b, k)),
    }
}

fn tween_appearance(a: &Appearance, b: &Appearance, k: f64) -> Appearance {
    let strokes = if a.strokes.len() == b.strokes.len() {
        a.strokes.iter().zip(&b.strokes).map(|(x, y)| (lerp_c(x.0, y.0, k), lerp32(x.1, y.1, k).max(0.0), y.2)).collect()
    } else {
        // a stroke that comes or goes grows from / shrinks to nothing
        let n = a.strokes.len().max(b.strokes.len());
        (0..n)
            .filter_map(|i| match (a.strokes.get(i), b.strokes.get(i)) {
                (Some(x), Some(y)) => Some((lerp_c(x.0, y.0, k), lerp32(x.1, y.1, k).max(0.0), y.2)),
                (Some(x), None) => Some((x.0, lerp32(x.1, 0.0, k).max(0.0), x.2)),
                (None, Some(y)) => Some((y.0, lerp32(0.0, y.1, k).max(0.0), y.2)),
                (None, None) => None,
            })
            .filter(|s| s.1 > 0.0)
            .collect()
    };
    let background = match (a.background, b.background) {
        (None, None) => None,
        (x, y) => {
            let (xa, ya) = (x.or(y), y.or(x));
            xa.zip(ya).map(|(xa, ya)| {
                let c = lerp_opt_c(x.map(|v| v.0), y.map(|v| v.0), k).unwrap_or(ya.0);
                (c, lerp32(xa.1, ya.1, k).max(0.0), lerp32(xa.2, ya.2, k).max(0.0))
            })
        }
    };
    let shadow = match (&a.shadow, &b.shadow) {
        (None, None) => None,
        (x, y) => {
            let (xa, ya) = (x.as_ref().or(y.as_ref()), y.as_ref().or(x.as_ref()));
            xa.zip(ya).map(|(xa, ya)| Shadow {
                color: lerp_opt_c(x.as_ref().map(|v| v.color), y.as_ref().map(|v| v.color), k).unwrap_or(ya.color),
                offset: (lerp32(xa.offset.0, ya.offset.0, k), lerp32(xa.offset.1, ya.offset.1, k)),
                size: lerp32(xa.size, ya.size, k).max(0.0),
                blur: lerp32(xa.blur, ya.blur, k).max(0.0),
            })
        }
    };
    Appearance {
        fill: lerp_opt_c(a.fill, b.fill, k),
        // a gradient cannot be mixed with a solid: it switches half way
        gradient: if k < 0.5 { a.gradient.clone() } else { b.gradient.clone() },
        strokes,
        background,
        shadow,
    }
}

/// `b` as it looks `k` of the way from `a` (the two are the same thing: see [`same_thing`]).
fn tween_pair(a: &LayerSpec, b: &LayerSpec, k: f64) -> LayerSpec {
    let mut out = b.clone();
    out.pin = None;
    let (ta, tb) = (&a.transform, &b.transform);
    out.transform.position = lerp_v(ta.position, tb.position, k);
    out.transform.anchor = lerp_v(ta.anchor, tb.anchor, k);
    out.transform.scale = lerp_v(ta.scale, tb.scale, k);
    out.transform.rotation = lerp(ta.rotation, tb.rotation, k);
    out.transform.opacity = lerp32(ta.opacity, tb.opacity, k).clamp(0.0, 1.0);
    out.appearance = tween_appearance(&a.appearance, &b.appearance, k);
    let size = |x: (f32, f32), y: (f32, f32)| (lerp32(x.0, y.0, k).max(0.0), lerp32(x.1, y.1, k).max(0.0));
    match (&a.content, &mut out.content) {
        (LayerContent::Shape(x), LayerContent::Shape(y)) => {
            y.size = size(x.size, y.size);
            y.corner_radius = lerp32(x.corner_radius, y.corner_radius, k).max(0.0);
        }
        (LayerContent::Media(x), LayerContent::Media(y)) => {
            y.shape.size = size(x.shape.size, y.shape.size);
            y.shape.corner_radius = lerp32(x.shape.corner_radius, y.shape.corner_radius, k).max(0.0);
            y.focus = (lerp32(x.focus.0, y.focus.0, k).clamp(0.0, 1.0), lerp32(x.focus.1, y.focus.1, k).clamp(0.0, 1.0));
            y.zoom = lerp32(x.zoom, y.zoom, k).clamp(0.01, 100.0);
            // Fill and Fit place the picture differently: the fit changes half way, as the
            // source does, while the box itself moves continuously.
            if k < 0.5 {
                y.fit = x.fit;
                y.source = x.source;
            }
        }
        (LayerContent::Text(x), LayerContent::Text(y)) => {
            y.size = lerp32(x.size, y.size, k).max(0.1);
            y.tracking = lerp32(x.tracking, y.tracking, k);
        }
        _ => {}
    }
    out
}

/// Whether two layers are the same thing in two layouts, so that one becomes the other.
fn same_thing(a: &LayerSpec, b: &LayerSpec) -> bool {
    match (&a.content, &b.content) {
        (LayerContent::Media(x), LayerContent::Media(y)) => {
            if !a.name.is_empty() && a.name == b.name {
                return true;
            }
            if !x.role.is_empty() || !y.role.is_empty() { x.role == y.role } else { a.name.is_empty() && b.name.is_empty() && x.source == y.source }
        }
        // a different shape kind cannot become another by interpolation
        (LayerContent::Shape(x), LayerContent::Shape(y)) => !a.name.is_empty() && a.name == b.name && x.shape == y.shape && x.shape != 3,
        // text that reads differently is a different thing, even under the same name
        (LayerContent::Text(x), LayerContent::Text(y)) => {
            x.text == y.text && x.vertical == y.vertical && (a.name == b.name || a.name.is_empty() || b.name.is_empty())
        }
        _ => false,
    }
}

/// The layers to draw `k` of the way (eased, 0 = `from`, 1 = `to`) through a smart transition
/// from the outgoing layout's layers to the incoming one's, both as shown (pins resolved), in
/// paint order.
///
/// Layers that are the same thing in both ([`same_thing`]: same name and kind; media windows with
/// the same role; text with the same words) are interpolated: position, size, corner radius,
/// scale, rotation, opacity, colours, strokes and shadow. The rest belong to one layout only:
/// with `fades` they fade out (outgoing) or in (incoming); without, the incoming ones are there
/// from the first frame and the outgoing ones are gone. Hidden layers take no part. The result
/// keeps the incoming layout's order, with outgoing-only layers kept just above the layer they
/// sat above before.
pub fn tween_layers(from: &[LayerSpec], to: &[LayerSpec], k: f64, fades: bool) -> Vec<LayerSpec> {
    let k = if k.is_finite() { k } else { 1.0 };
    // opacity never overshoots
    let fade = k.clamp(0.0, 1.0);
    let mut partner: Vec<Option<usize>> = vec![None; to.len()];
    let mut taken = vec![false; from.len()];
    for (bi, b) in to.iter().enumerate().filter(|(_, b)| b.enabled) {
        if let Some(ai) = (0..from.len()).find(|&ai| !taken[ai] && from[ai].enabled && same_thing(&from[ai], b)) {
            taken[ai] = true;
            partner[bi] = Some(ai);
        }
    }
    // outgoing-only layers, each after the incoming layer its nearest matched predecessor became
    let mut leaving: Vec<(Option<usize>, &LayerSpec)> = Vec::new();
    if fades && fade < 1.0 {
        let mut after: Option<usize> = None;
        for (ai, a) in from.iter().enumerate() {
            if taken[ai] {
                after = partner.iter().position(|p| *p == Some(ai)).or(after);
            } else if a.enabled {
                leaving.push((after, a));
            }
        }
    }
    let mut out = Vec::with_capacity(to.len() + leaving.len());
    let push_leaving = |out: &mut Vec<LayerSpec>, at: Option<usize>| {
        for (_, a) in leaving.iter().filter(|(after, _)| *after == at) {
            let mut l = (*a).clone();
            l.pin = None;
            l.transform.opacity *= (1.0 - fade) as f32;
            out.push(l);
        }
    };
    push_leaving(&mut out, None);
    for (bi, b) in to.iter().enumerate() {
        match partner[bi].and_then(|ai| from.get(ai)) {
            Some(a) => out.push(tween_pair(a, b, k)),
            None => {
                let mut l = b.clone();
                l.pin = None;
                if fades && l.enabled {
                    l.transform.opacity *= fade as f32;
                }
                out.push(l);
            }
        }
        push_leaving(&mut out, Some(bi));
    }
    out
}

// ------------------------------------------------------------------------------------------------
// Theme
// ------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeFont {
    pub family: String,
    pub style: String,
}

/// A pack's named colours (`#rrggbb` or `#rrggbbaa`, sRGB) and fonts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub colors: BTreeMap<String, String>,
    pub fonts: BTreeMap<String, ThemeFont>,
}

/// Binds one layer parameter to a theme entry: `token` is `color.<name>` (for a colour
/// parameter) or `font.<name>` (with `param: "font"`; sets the family and the style).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeBinding {
    pub layer: u64,
    pub param: String,
    pub token: String,
}

/// `#rrggbb` / `#rrggbbaa` → sRGB-encoded straight RGBA.
pub fn parse_hex_color(s: &str) -> Option<[f32; 4]> {
    let h = s.trim().strip_prefix('#')?;
    if !(h.len() == 6 || h.len() == 8) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok()).map(|v| v as f32 / 255.0);
    Some([byte(0)?, byte(2)?, byte(4)?, if h.len() == 8 { byte(6)? } else { 1.0 }])
}

/// sRGB-encoded straight RGBA → `#rrggbb` (`#rrggbbaa` when not opaque).
pub fn hex_color(c: [f32; 4]) -> String {
    let b = |v: f32| (if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 } * 255.0).round() as u8;
    if b(c[3]) == 255 {
        format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]), b(c[3]))
    }
}

impl Theme {
    pub fn color(&self, name: &str) -> Option<[f32; 4]> {
        self.colors.get(name).and_then(|s| parse_hex_color(s))
    }

    /// The value a token stands for: a colour, or a font (family, style).
    fn resolve(&self, token: &str) -> Option<Resolved> {
        if let Some(n) = token.strip_prefix("color.") {
            return self.color(n).map(Resolved::Color);
        }
        token.strip_prefix("font.").and_then(|n| self.fonts.get(n)).map(|f| Resolved::Font(f.family.clone(), f.style.clone()))
    }

    /// Entries of `over` replace or add to this theme's.
    pub fn merged(&self, over: &Theme) -> Theme {
        let mut t = self.clone();
        t.colors.extend(over.colors.iter().map(|(k, v)| (k.clone(), v.clone())));
        t.fonts.extend(over.fonts.iter().map(|(k, v)| (k.clone(), v.clone())));
        t
    }

    fn validate(&self) -> Result<(), PackError> {
        if self.colors.len() > MAX_THEME_ENTRIES || self.fonts.len() > MAX_THEME_ENTRIES {
            return Err(PackError::Invalid("the theme has too many entries".into()));
        }
        for (k, v) in &self.colors {
            if parse_hex_color(v).is_none() {
                return Err(PackError::Invalid(format!("theme colour `{k}` is `{v}`, not #rrggbb")));
            }
        }
        for (k, f) in &self.fonts {
            if f.family.trim().is_empty() {
                return Err(PackError::Invalid(format!("theme font `{k}` has no family")));
            }
        }
        Ok(())
    }
}

enum Resolved {
    Color([f32; 4]),
    Font(String, String),
}

fn set_all(e: &mut EffectInstance, id: &str, v: ParamValue) {
    match e.params.get_mut(id) {
        // keyframed parameters keep their animation shape; every key takes the themed value
        Some(p) => p.map_values(|_| v.clone()),
        None => {
            e.params.insert(id.to_string(), Param::new(v));
        }
    }
}

/// Write the theme's values into the bound parameters of `layers`. Bindings whose layer or token
/// is missing are skipped. Returns how many parameters were set.
pub fn apply_theme(layers: &mut [EffectInstance], bindings: &[ThemeBinding], theme: &Theme) -> usize {
    let mut n = 0;
    for b in bindings {
        let Some(l) = layers.iter_mut().find(|l| graphic::is_layer(l) && layer_uid(l) == b.layer && b.layer != 0) else { continue };
        match theme.resolve(&b.token) {
            Some(Resolved::Color(mut c)) => {
                // a themed colour keeps the alpha the layout gave the parameter
                if let Some(old) = l.params.get(&b.param).and_then(|p| p.value.as_color()) {
                    c[3] = old[3];
                }
                set_all(l, &b.param, ParamValue::Color(c));
                n += 1;
            }
            Some(Resolved::Font(family, style)) => {
                set_all(l, "font", ParamValue::Text(family));
                set_all(l, "font_style", ParamValue::Text(style));
                n += 1;
            }
            None => {}
        }
    }
    n
}

// ------------------------------------------------------------------------------------------------
// Pack
// ------------------------------------------------------------------------------------------------

/// One drawing of a layout for a frame shape.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Design {
    /// Frame size it was drawn for.
    pub canvas: [u32; 2],
    /// Graphic layers, each with a `uid`. The same layer has the same uid in every design of a
    /// layout (controls and theme bindings refer to it).
    pub layers: Vec<EffectInstance>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    /// Unique in the pack (`[a-z0-9_-]`).
    pub id: String,
    pub name: String,
    /// See [`LAYOUT_KINDS`].
    pub kind: String,
    pub description: String,
    /// Clip length when the layout is placed without a range (0: five seconds). A layout applied
    /// to a scene always takes the scene's length.
    pub duration: Tick,
    /// Roll and responsive time of the placed clip (its `template`, `layout` and `smart` are set
    /// when the layout is placed).
    #[serde(skip_serializing_if = "GraphicMeta::is_empty")]
    pub graphic: GraphicMeta,
    /// The properties a user of the layout edits (headings, names…).
    pub controls: Vec<TemplateControl>,
    pub bindings: Vec<ThemeBinding>,
    /// At least one.
    pub designs: Vec<Design>,
}

impl Layout {
    /// The design drawn for the frame shape nearest `canvas` (by aspect ratio).
    pub fn design_for(&self, canvas: (u32, u32)) -> Option<&Design> {
        let aspect = |w: u32, h: u32| (w.max(1) as f64 / h.max(1) as f64).ln();
        let want = aspect(canvas.0, canvas.1);
        self.designs.iter().min_by(|a, b| {
            let (da, db) = ((aspect(a.canvas[0], a.canvas[1]) - want).abs(), (aspect(b.canvas[0], b.canvas[1]) - want).abs());
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Roles its media windows ask for, in paint order (empty string: no role).
    pub fn media_roles(&self) -> Vec<String> {
        self.designs.first().map_or_else(Vec::new, |d| {
            d.layers
                .iter()
                .filter(|l| l.effect == MEDIA_LAYER)
                .map(|l| match l.params.get("role").map(|p| &p.value) {
                    Some(ParamValue::Text(s)) => s.clone(),
                    _ => String::new(),
                })
                .collect()
        })
    }
}

/// A layout pack (`.fcpack`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutPack {
    pub format: String,
    pub version: u32,
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub license: String,
    pub tags: Vec<String>,
    pub theme: Theme,
    /// The smart transition given to the clips this pack places.
    pub transition: SmartTransition,
    pub layouts: Vec<Layout>,
    #[serde(skip_serializing_if = "Resources::is_empty")]
    pub resources: Resources,
}

impl Default for LayoutPack {
    fn default() -> Self {
        LayoutPack {
            format: PACK_FORMAT.into(),
            version: PACK_VERSION,
            id: String::new(),
            name: String::new(),
            description: String::new(),
            author: String::new(),
            license: String::new(),
            tags: Vec::new(),
            theme: Theme::default(),
            transition: SmartTransition::default(),
            layouts: Vec::new(),
            resources: Resources::default(),
        }
    }
}

/// The layout a placed clip came from (`GraphicMeta::layout`): enough to restyle the clip when
/// the pack's theme changes and to swap it for another layout of the pack.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutLink {
    pub pack: String,
    pub layout: String,
    pub kind: String,
    pub bindings: Vec<ThemeBinding>,
}

/// Problems reading or validating a pack.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PackError {
    #[error("this file is a ZIP archive (another application's package?): FilmCraft reads only its own .fcpack layout packs")]
    ForeignPackage,
    #[error("not a FilmCraft layout pack: {0}")]
    NotPack(String),
    #[error("this layout pack was made by a newer FilmCraft (format version {0}; this build reads up to {PACK_VERSION})")]
    TooNew(u32),
    #[error("invalid layout pack: {0}")]
    Invalid(String),
}

fn id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_NAME && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Drop the parameters of `layers` that still have their effect's default value (and no
/// keyframes). A layer has some fifty parameters and a design changes a handful, so this is most
/// of a pack file. [`fill_defaults`] puts them back.
fn strip_defaults(layers: &mut [EffectInstance]) {
    for l in layers {
        let Some(def) = crate::effect::find_effect(&l.effect) else { continue };
        l.params.retain(|k, p| p.is_animated() || def.param(k).is_none_or(|d| d.default != p.value));
    }
}

/// Give `layers` every parameter of their effect that they lack, at its default value.
fn fill_defaults(layers: &mut [EffectInstance]) {
    for l in layers {
        let Some(def) = crate::effect::find_effect(&l.effect) else { continue };
        for d in &def.params {
            l.params.entry(d.id.to_string()).or_insert_with(|| Param::new(d.default.clone()));
        }
    }
}

impl LayoutPack {
    /// Serialize (pretty JSON). Layer parameters left at their default value are not written.
    pub fn to_json(&self) -> String {
        let mut compact = self.clone();
        for d in compact.layouts.iter_mut().flat_map(|l| l.designs.iter_mut()) {
            strip_defaults(&mut d.layers);
        }
        serde_json::to_string_pretty(&compact).unwrap_or_default()
    }

    /// Read a pack file. ZIP archives are refused without being inspected.
    pub fn from_bytes(bytes: &[u8]) -> Result<LayoutPack, PackError> {
        if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
            return Err(PackError::ForeignPackage);
        }
        let text = std::str::from_utf8(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)).map_err(|_| PackError::NotPack("not UTF-8 text".into()))?;
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| PackError::NotPack(e.to_string()))?;
        if v.get("format").and_then(|f| f.as_str()) != Some(PACK_FORMAT) {
            return Err(PackError::NotPack(format!("`format` is not \"{PACK_FORMAT}\"")));
        }
        let ver = v.get("version").and_then(|x| x.as_u64()).unwrap_or(0).min(u32::MAX as u64) as u32;
        if ver > PACK_VERSION {
            return Err(PackError::TooNew(ver));
        }
        if ver == 0 {
            return Err(PackError::Invalid("missing `version`".into()));
        }
        let mut p: LayoutPack = serde_json::from_value(v).map_err(|e| PackError::Invalid(e.to_string()))?;
        p.transition = p.transition.sanitized();
        if p.layouts.len() > MAX_LAYOUTS || p.layouts.iter().any(|l| l.designs.len() > MAX_DESIGNS || l.designs.iter().any(|d| d.layers.len() > MAX_LAYERS)) {
            return Err(PackError::Invalid("the pack has too many layouts, designs or layers".into()));
        }
        for l in &mut p.layouts {
            l.duration = Tick(l.duration.0.clamp(0, MAX_DURATION));
            for d in &mut l.designs {
                fill_defaults(&mut d.layers);
            }
        }
        p.validate()?;
        Ok(p)
    }

    /// Check the pack's structure: names and unique ids, sane sizes and counts, layers that are
    /// graphic layers with unique uids, controls and bindings that point at existing layers,
    /// parameters and theme entries.
    pub fn validate(&self) -> Result<(), PackError> {
        let bad = |m: String| Err(PackError::Invalid(m));
        if self.name.trim().is_empty() || self.name.len() > MAX_NAME {
            return bad("the pack has no name".into());
        }
        if self.layouts.is_empty() {
            return bad("the pack has no layouts".into());
        }
        if self.layouts.len() > MAX_LAYOUTS {
            return bad(format!("the pack has more than {MAX_LAYOUTS} layouts"));
        }
        self.theme.validate()?;
        let mut ids: Vec<&str> = Vec::new();
        for l in &self.layouts {
            let what = if l.name.is_empty() { &l.id } else { &l.name };
            if !id_ok(&l.id) || ids.contains(&l.id.as_str()) {
                return bad(format!("layout `{what}` has no unique id (lower-case letters, digits, - and _)"));
            }
            ids.push(&l.id);
            if l.name.trim().is_empty() || l.name.len() > MAX_NAME || l.kind.len() > MAX_NAME {
                return bad(format!("layout `{}` has no name", l.id));
            }
            if l.designs.is_empty() || l.designs.len() > MAX_DESIGNS {
                return bad(format!("layout `{what}` needs 1 to {MAX_DESIGNS} designs"));
            }
            for (di, d) in l.designs.iter().enumerate() {
                if d.canvas[0] == 0 || d.canvas[1] == 0 || d.canvas[0] > 16384 || d.canvas[1] > 16384 {
                    return bad(format!("layout `{what}`: bad canvas size {:?}", d.canvas));
                }
                if d.layers.is_empty() || d.layers.len() > MAX_LAYERS {
                    return bad(format!("layout `{what}`: a design needs 1 to {MAX_LAYERS} layers"));
                }
                let mut uids = Vec::new();
                for (i, e) in d.layers.iter().enumerate() {
                    if !graphic::is_layer(e) {
                        return bad(format!("layout `{what}`: layer {i} is `{}`, not a graphic layer", e.effect));
                    }
                    let uid = layer_uid(e);
                    if uid == 0 || uids.contains(&uid) {
                        return bad(format!("layout `{what}`: layer {i} of design {di} has no unique uid"));
                    }
                    uids.push(uid);
                }
            }
            // controls and bindings are checked against the first design; another design may leave
            // a layer out (the control or binding then does nothing there)
            let Some(first) = l.designs.first() else { continue };
            let mut cids: Vec<&str> = Vec::new();
            for c in &l.controls {
                if c.id.is_empty() || cids.contains(&c.id.as_str()) {
                    return bad(format!("layout `{what}`: control `{}` has no unique id", c.name));
                }
                cids.push(&c.id);
                let Some(e) = first.layers.iter().find(|e| layer_uid(e) == c.layer) else {
                    return bad(format!("layout `{what}`: control `{}` points at a missing layer", c.name));
                };
                if c.param != "enabled" && !e.params.contains_key(&c.param) {
                    return bad(format!("layout `{what}`: control `{}`: the layer has no parameter `{}`", c.name, c.param));
                }
            }
            if l.controls.len() > MAX_LAYERS * 4 || l.bindings.len() > MAX_LAYERS * 8 {
                return bad(format!("layout `{what}` has too many controls or bindings"));
            }
            for b in &l.bindings {
                if !first.layers.iter().any(|e| layer_uid(e) == b.layer) {
                    return bad(format!("layout `{what}`: a theme binding points at a missing layer"));
                }
                if self.theme.resolve(&b.token).is_none() {
                    return bad(format!("layout `{what}`: the theme has no `{}`", b.token));
                }
                if b.token.starts_with("font.") && b.param != "font" {
                    return bad(format!("layout `{what}`: a font binding must have `param: \"font\"`"));
                }
            }
        }
        for f in &self.resources.fonts {
            if base64_decode(&f.data).is_none() {
                return bad(format!("font `{}` is not valid base64", f.family));
            }
        }
        Ok(())
    }

    /// Find a layout by id, or by name (ignoring case).
    pub fn layout(&self, key: &str) -> Option<&Layout> {
        self.layouts.iter().find(|l| l.id == key).or_else(|| self.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(key)))
    }

    /// The layers and settings of a clip showing `layout` on a `canvas`-sized frame: the design
    /// nearest that shape, scaled to fit and centred (pin distances scale too; layers pinned to
    /// the frame then reach its edges), with the theme written into the bound parameters. The
    /// result carries the layout's controls (`GraphicMeta::template`, so they are edited like a
    /// graphics template's), its link to the pack and the pack's smart transition.
    pub fn instantiate(&self, layout: &Layout, canvas: (u32, u32)) -> Option<(Vec<EffectInstance>, GraphicMeta)> {
        let d = layout.design_for(canvas)?;
        let mut layers = d.layers.clone();
        crate::gtemplate::fit_layers(&mut layers, (d.canvas[0], d.canvas[1]), canvas);
        apply_theme(&mut layers, &layout.bindings, &self.theme);
        let mut meta = layout.graphic.clone();
        meta.template = Some(TemplateLink { id: format!("{}/{}", self.id, layout.id), name: layout.name.clone(), controls: layout.controls.clone() });
        meta.layout = Some(LayoutLink { pack: self.id.clone(), layout: layout.id.clone(), kind: layout.kind.clone(), bindings: layout.bindings.clone() });
        meta.smart = Some(self.transition.clone());
        Some((layers, meta))
    }

    /// A copy of the pack under a new id and name with `theme`'s entries over its own: the same
    /// layouts in other colours and fonts.
    pub fn remix(&self, id: &str, name: &str, theme: &Theme) -> LayoutPack {
        let mut p = self.clone();
        p.id = id.to_string();
        p.name = name.to_string();
        p.theme = self.theme.merged(theme);
        p
    }
}

// ------------------------------------------------------------------------------------------------
// Built-in packs (original designs)
// ------------------------------------------------------------------------------------------------

// Layer uids: the same thing has the same uid and name in every layout, which is what lets a
// smart transition carry it from one layout to the next.
const BG: u64 = 1;
const SPEAKER: u64 = 2;
const SCREEN: u64 = 3;
const ACCENT: u64 = 4;
const HEADING: u64 = 5;
const SUB: u64 = 6;
const PLATE: u64 = 7;
const NUMBER: u64 = 8;
const KICKER: u64 = 9;
const MARK: u64 = 10;
const QUOTE: u64 = 11;

fn set(e: &mut EffectInstance, id: &str, v: ParamValue) {
    e.params.insert(id.to_string(), Param::new(v));
}

fn named(mut e: EffectInstance, uid: u64, name: &str) -> EffectInstance {
    set(&mut e, "name", ParamValue::Text(name.into()));
    e.layer = Some(Box::new(LayerExtra { uid, ..Default::default() }));
    e
}

fn pin(e: &mut EffectInstance, to: u64, edges: [bool; 4], offsets: [f64; 4]) {
    let p =
        Pin { to: if to == 0 { PinTarget::Frame } else { PinTarget::Layer(to) }, left: edges[0], top: edges[1], right: edges[2], bottom: edges[3], offsets };
    e.layer.get_or_insert_with(Default::default).pin = Some(p);
}

/// One frame shape a built-in layout is drawn for: its size and the unit all measures scale by.
#[derive(Clone, Copy)]
struct Frame {
    w: f64,
    h: f64,
    /// 1 on a 1080-pixel short side.
    u: f64,
}

impl Frame {
    fn new(w: u32, h: u32) -> Frame {
        Frame { w: w as f64, h: h as f64, u: w.min(h) as f64 / 1080.0 }
    }
    fn landscape(&self) -> bool {
        self.w > self.h * 1.2
    }
    fn portrait(&self) -> bool {
        self.h > self.w * 1.2
    }
    /// The outer margin.
    fn m(&self) -> f64 {
        64.0 * self.u
    }
    /// A rectangle (corner + size) as a shape layer.
    #[allow(clippy::too_many_arguments)]
    fn rect(&self, uid: u64, name: &str, x: f64, y: f64, w: f64, h: f64, radius: f64) -> EffectInstance {
        let mut e = named(new_shape_layer(0, Vec2::new(x + w / 2.0, y + h / 2.0), Vec2::new(w.max(0.0), h.max(0.0)), vec![]), uid, name);
        set(&mut e, "corner_radius", ParamValue::Float(radius.max(0.0)));
        e
    }
    /// The frame-filling background, which follows the frame's edges at any size.
    fn background(&self) -> EffectInstance {
        let mut e = self.rect(BG, "Background", 0.0, 0.0, self.w, self.h, 0.0);
        pin(&mut e, 0, [true; 4], [0.0; 4]);
        e
    }
    /// A media window (corner + size).
    #[allow(clippy::too_many_arguments)]
    fn window(&self, uid: u64, name: &str, role: &str, source: u32, x: f64, y: f64, w: f64, h: f64, radius: f64) -> EffectInstance {
        let mut e = named(new_media_layer(source, Vec2::new(x + w / 2.0, y + h / 2.0), Vec2::new(w.max(0.0), h.max(0.0))), uid, name);
        set(&mut e, "role", ParamValue::Text(role.into()));
        set(&mut e, "corner_radius", ParamValue::Float(radius.max(0.0)));
        e
    }
    /// Point text with its baseline origin at (`x`, `y`); `align` 0 left, 1 centre.
    #[allow(clippy::too_many_arguments)]
    fn text(&self, uid: u64, name: &str, s: &str, x: f64, y: f64, size: f64, align: u32) -> EffectInstance {
        let mut e = named(new_text_layer(s, Vec2::new(x, y), size * self.u), uid, name);
        set(&mut e, "align", ParamValue::Choice(align));
        e
    }
    fn soft_shadow(&self, e: &mut EffectInstance) {
        set(e, "shadow", ParamValue::Bool(true));
        set(e, "shadow_opacity", ParamValue::Float(45.0));
        set(e, "shadow_angle", ParamValue::Float(180.0));
        set(e, "shadow_distance", ParamValue::Float(18.0 * self.u));
        set(e, "shadow_blur", ParamValue::Float(70.0 * self.u));
    }
}

fn bind(layer: u64, param: &str, token: &str) -> ThemeBinding {
    ThemeBinding { layer, param: param.into(), token: token.into() }
}
fn ctl(id: &str, name: &str, layer: u64, param: &str) -> TemplateControl {
    TemplateControl { id: id.into(), name: name.into(), kind: ControlKind::for_param(param), layer, param: param.into(), min: None, max: None }
}
fn slider(id: &str, name: &str, layer: u64, param: &str, min: f64, max: f64) -> TemplateControl {
    TemplateControl { min: Some(min), max: Some(max), ..ctl(id, name, layer, param) }
}

/// The frame shapes the built-in layouts are drawn for.
const SHAPES: [(u32, u32); 4] = [(1920, 1080), (1080, 1920), (1080, 1080), (1080, 1350)];

struct Draft {
    id: &'static str,
    name: &'static str,
    kind: &'static str,
    description: &'static str,
    /// Seconds (0 = no length of its own).
    seconds: i64,
    draw: fn(Frame) -> Vec<EffectInstance>,
    controls: Vec<TemplateControl>,
    bindings: Vec<ThemeBinding>,
}

/// Round a design's measures to hundredths of a pixel: the numbers then read back from JSON
/// exactly as written, and the files stay short.
fn tidy(mut layers: Vec<EffectInstance>) -> Vec<EffectInstance> {
    let r = |v: f64| if v.is_finite() { (v * 100.0).round() / 100.0 } else { v };
    for l in &mut layers {
        for p in l.params.values_mut() {
            p.map_values(|v| match v {
                ParamValue::Float(x) => ParamValue::Float(r(x)),
                ParamValue::Vec2(q) => ParamValue::Vec2(Vec2::new(r(q.x), r(q.y))),
                o => o,
            });
        }
        if let Some(pin) = l.layer.as_mut().and_then(|x| x.pin.as_mut()) {
            pin.offsets.iter_mut().for_each(|o| *o = r(*o));
        }
    }
    layers
}

impl Draft {
    fn build(self) -> Layout {
        Layout {
            id: self.id.into(),
            name: self.name.into(),
            kind: self.kind.into(),
            description: self.description.into(),
            duration: Tick(self.seconds * TICKS_PER_SECOND),
            graphic: GraphicMeta::default(),
            controls: self.controls,
            bindings: self.bindings,
            designs: SHAPES.iter().map(|&(w, h)| Design { canvas: [w, h], layers: tidy((self.draw)(Frame::new(w, h))) }).collect(),
        }
    }
}

// Speaker – Full: the picture fills the frame; a name plate sits bottom left and widens with the name.
fn draw_speaker_full(f: Frame) -> Vec<EffectInstance> {
    let (m, u) = (f.m(), f.u);
    let mut speaker = f.window(SPEAKER, "Speaker", "camera", 1, 0.0, 0.0, f.w, f.h, 0.0);
    pin(&mut speaker, 0, [true; 4], [0.0; 4]);
    let (ph, px) = (156.0 * u, m);
    let py = f.h - m - ph;
    let mut plate = f.rect(PLATE, "Plate", px, py, 560.0 * u, ph, 18.0 * u);
    set(&mut plate, "opacity", ParamValue::Float(94.0));
    pin(&mut plate, HEADING, [true, false, true, false], [-46.0 * u, 0.0, 52.0 * u, 0.0]);
    let accent = f.rect(ACCENT, "Accent", px + 18.0 * u, py + 30.0 * u, 8.0 * u, ph - 60.0 * u, 4.0 * u);
    let heading = f.text(HEADING, "Heading", "Alex Morgan", px + 46.0 * u, py + 72.0 * u, 56.0, 0);
    let sub = f.text(SUB, "Subheading", "Founder, Northwind", px + 46.0 * u, py + 122.0 * u, 34.0, 0);
    vec![speaker, plate, accent, heading, sub]
}

// Speaker – Framed: the picture in a rounded window on the pack's background, with a caption strip under it.
fn draw_speaker_framed(f: Frame) -> Vec<EffectInstance> {
    let (m, u) = (f.m(), f.u);
    let strip = 170.0 * u;
    let mut speaker = f.window(SPEAKER, "Speaker", "camera", 1, m, m, f.w - 2.0 * m, f.h - 2.0 * m - strip, 28.0 * u);
    f.soft_shadow(&mut speaker);
    let ty = f.h - m - strip;
    let accent = f.rect(ACCENT, "Accent", m, ty + 50.0 * u, 8.0 * u, 104.0 * u, 4.0 * u);
    let heading = f.text(HEADING, "Heading", "What we learned shipping it", m + 32.0 * u, ty + 98.0 * u, 54.0, 0);
    let sub = f.text(SUB, "Subheading", "Alex Morgan", m + 32.0 * u, ty + 148.0 * u, 34.0, 0);
    vec![f.background(), speaker, accent, heading, sub]
}

// Speaker + Title: the picture on one side, a heading on the other.
fn draw_speaker_title(f: Frame) -> Vec<EffectInstance> {
    let (m, u) = (f.m(), f.u);
    if f.landscape() {
        let x = f.w * 0.46;
        let mut speaker = f.window(SPEAKER, "Speaker", "camera", 1, x, m, f.w - x - m, f.h - 2.0 * m, 28.0 * u);
        f.soft_shadow(&mut speaker);
        let ty = f.h * 0.5;
        let accent = f.rect(ACCENT, "Accent", m + 12.0 * u, ty - 160.0 * u, 120.0 * u, 10.0 * u, 5.0 * u);
        let heading = f.text(HEADING, "Heading", "Three things\nto know", m + 12.0 * u, ty - 50.0 * u, 92.0, 0);
        let sub = f.text(SUB, "Subheading", "A short walkthrough", m + 12.0 * u, ty + 170.0 * u, 38.0, 0);
        vec![f.background(), speaker, accent, heading, sub]
    } else {
        let wh = (f.h - 3.0 * m) * if f.portrait() { 0.6 } else { 0.56 };
        let mut speaker = f.window(SPEAKER, "Speaker", "camera", 1, m, m, f.w - 2.0 * m, wh, 28.0 * u);
        f.soft_shadow(&mut speaker);
        let ty = 2.0 * m + wh;
        let accent = f.rect(ACCENT, "Accent", m, ty + 8.0 * u, 120.0 * u, 10.0 * u, 5.0 * u);
        let heading = f.text(HEADING, "Heading", "Three things\nto know", m, ty + 112.0 * u, 80.0, 0);
        let sub = f.text(SUB, "Subheading", "A short walkthrough", m, ty + 300.0 * u, 38.0, 0);
        vec![f.background(), speaker, accent, heading, sub]
    }
}

// Screen + Speaker: a screen recording, shown whole, beside (or above) the speaker.
fn draw_split(f: Frame) -> Vec<EffectInstance> {
    let (m, u) = (f.m(), f.u);
    let r = 24.0 * u;
    let (mut screen, mut speaker) = if f.landscape() {
        // the screen's window has the shape of a 16:9 recording; the pair is centred in the frame
        let sw = (f.w - 3.0 * m) * 0.7;
        let sh = (sw * 9.0 / 16.0).min(f.h - 2.0 * m);
        let y = (f.h - sh) / 2.0;
        (f.window(SCREEN, "Screen", "screen", 1, m, y, sw, sh, r), f.window(SPEAKER, "Speaker", "camera", 2, 2.0 * m + sw, y, f.w - 3.0 * m - sw, sh, r))
    } else {
        let sw = f.w - 2.0 * m;
        let sh = (sw * 9.0 / 16.0).min((f.h - 3.0 * m) * 0.5);
        (f.window(SCREEN, "Screen", "screen", 1, m, m, sw, sh, r), f.window(SPEAKER, "Speaker", "camera", 2, m, 2.0 * m + sh, sw, f.h - 3.0 * m - sh, r))
    };
    // a screen recording is shown whole; the window's own fill shows around it
    set(&mut screen, "fit", ParamValue::Choice(1));
    f.soft_shadow(&mut screen);
    f.soft_shadow(&mut speaker);
    vec![f.background(), screen, speaker]
}

// Screen + Bubble: the screen recording fills the frame, the speaker rides in a round window in a corner.
fn draw_screen_bubble(f: Frame) -> Vec<EffectInstance> {
    let (m, u) = (f.m(), f.u);
    let mut screen = f.window(SCREEN, "Screen", "screen", 1, 0.0, 0.0, f.w, f.h, 0.0);
    pin(&mut screen, 0, [true; 4], [0.0; 4]);
    let d = 320.0 * u;
    let mut speaker = f.window(SPEAKER, "Speaker", "camera", 2, f.w - m - d, f.h - m - d, d, d, d / 2.0);
    set(&mut speaker, "stroke", ParamValue::Bool(true));
    set(&mut speaker, "stroke_width", ParamValue::Float(6.0 * u));
    f.soft_shadow(&mut speaker);
    pin(&mut speaker, 0, [false, false, true, true], [0.0, 0.0, -m, -m]);
    vec![screen, speaker]
}

// Title Card: a heading and a line under it on the pack's background.
fn draw_title(f: Frame) -> Vec<EffectInstance> {
    let (m, u) = (f.m(), f.u);
    let (x, y) = (1.5 * m, f.h * 0.5);
    let size = if f.landscape() { 120.0 } else { 88.0 };
    let accent = f.rect(ACCENT, "Accent", x, y - 150.0 * u, 140.0 * u, 12.0 * u, 6.0 * u);
    let heading = f.text(HEADING, "Heading", "Getting started", x, y + 14.0 * u, size, 0);
    let sub = f.text(SUB, "Subheading", "Part one of the series", x, y + 100.0 * u, 42.0, 0);
    vec![f.background(), accent, heading, sub]
}

// Chapter Card: a large chapter number with the chapter's title.
fn draw_chapter(f: Frame) -> Vec<EffectInstance> {
    let (m, u) = (f.m(), f.u);
    let (x, y) = (1.5 * m, f.h * 0.5);
    let mut kicker = f.text(KICKER, "Kicker", "CHAPTER", x, y - 200.0 * u, 30.0, 0);
    set(&mut kicker, "tracking", ParamValue::Float(220.0));
    let number = f.text(NUMBER, "Number", "01", x - 8.0 * u, y + 50.0 * u, 270.0, 0);
    let accent = f.rect(ACCENT, "Accent", x, y + 100.0 * u, 140.0 * u, 12.0 * u, 6.0 * u);
    let heading = f.text(HEADING, "Heading", "Setting up", x, y + 216.0 * u, if f.landscape() { 84.0 } else { 68.0 }, 0);
    vec![f.background(), kicker, number, accent, heading]
}

// Pull Quote: a quotation in the serif face, with the speaker in a round window beside their name.
fn draw_quote(f: Frame) -> Vec<EffectInstance> {
    let (m, u) = (f.m(), f.u);
    let x = 1.5 * m;
    let top = f.h * if f.landscape() { 0.16 } else { 0.2 };
    let mark = f.text(MARK, "Quote Mark", "\u{201c}", x - 10.0 * u, top + 190.0 * u, 320.0, 0);
    // paragraph text: the box's top-left corner is the origin, and the words wrap at its width
    let mut quote = named(new_text_layer("The best layout is the one you stop noticing.", Vec2::new(x, top + 170.0 * u), 78.0 * u), QUOTE, "Quote");
    set(&mut quote, "box_width", ParamValue::Float(f.w - 2.0 * x));
    set(&mut quote, "leading", ParamValue::Float(12.0 * u));
    let d = 200.0 * u;
    let (sy, sx) = (f.h - 1.5 * m - d, x);
    let mut speaker = f.window(SPEAKER, "Speaker", "camera", 1, sx, sy, d, d, d / 2.0);
    set(&mut speaker, "stroke", ParamValue::Bool(true));
    set(&mut speaker, "stroke_width", ParamValue::Float(6.0 * u));
    let heading = f.text(HEADING, "Heading", "Alex Morgan", sx + d + 36.0 * u, sy + d / 2.0 - 2.0 * u, 50.0, 0);
    let sub = f.text(SUB, "Subheading", "Founder, Northwind", sx + d + 36.0 * u, sy + d / 2.0 + 50.0 * u, 34.0, 0);
    vec![f.background(), mark, quote, speaker, heading, sub]
}

// End Card: a centred sign-off.
fn draw_outro(f: Frame) -> Vec<EffectInstance> {
    let u = f.u;
    let (x, y) = (f.w / 2.0, f.h / 2.0);
    let heading = f.text(HEADING, "Heading", "Thanks for watching", x, y - 10.0 * u, if f.landscape() { 104.0 } else { 76.0 }, 1);
    let accent = f.rect(ACCENT, "Accent", x - 70.0 * u, y + 44.0 * u, 140.0 * u, 12.0 * u, 6.0 * u);
    let sub = f.text(SUB, "Subheading", "More at example.com", x, y + 140.0 * u, 42.0, 1);
    vec![f.background(), heading, accent, sub]
}

fn text_bindings(heading: bool, sub: bool) -> Vec<ThemeBinding> {
    let mut v = Vec::new();
    if heading {
        v.push(bind(HEADING, "fill_color", "color.text"));
        v.push(bind(HEADING, "font", "font.heading"));
    }
    if sub {
        v.push(bind(SUB, "fill_color", "color.muted"));
        v.push(bind(SUB, "font", "font.body"));
    }
    v
}

fn studio_layouts() -> Vec<Layout> {
    let with = |mut base: Vec<ThemeBinding>, more: Vec<ThemeBinding>| {
        base.extend(more);
        base
    };
    let window_fill = |uid: u64| bind(uid, "fill_color", "color.surface");
    vec![
        Draft {
            id: "speaker-full",
            name: "Speaker \u{2013} Full",
            kind: "camera",
            description: "The picture fills the frame, with a name plate that widens with the name.",
            seconds: 0,
            draw: draw_speaker_full,
            controls: vec![ctl("name", "Name", HEADING, "text"), ctl("role", "Role", SUB, "text"), ctl("show_role", "Show Role", SUB, "enabled")],
            bindings: with(
                text_bindings(true, true),
                vec![bind(PLATE, "fill_color", "color.surface"), bind(ACCENT, "fill_color", "color.accent"), window_fill(SPEAKER)],
            ),
        },
        Draft {
            id: "speaker-framed",
            name: "Speaker \u{2013} Framed",
            kind: "camera",
            description: "The picture in a rounded window on the pack's background, over a caption.",
            seconds: 0,
            draw: draw_speaker_framed,
            controls: vec![ctl("heading", "Heading", HEADING, "text"), ctl("subheading", "Subheading", SUB, "text")],
            bindings: with(
                text_bindings(true, true),
                vec![bind(BG, "fill_color", "color.background"), bind(ACCENT, "fill_color", "color.accent"), window_fill(SPEAKER)],
            ),
        },
        Draft {
            id: "speaker-title",
            name: "Speaker + Title",
            kind: "camera",
            description: "The picture on one side and a heading on the other.",
            seconds: 0,
            draw: draw_speaker_title,
            controls: vec![
                ctl("heading", "Heading", HEADING, "text"),
                ctl("subheading", "Subheading", SUB, "text"),
                slider("heading_size", "Heading Size", HEADING, "size", 24.0, 240.0),
            ],
            bindings: with(
                text_bindings(true, true),
                vec![bind(BG, "fill_color", "color.background"), bind(ACCENT, "fill_color", "color.accent"), window_fill(SPEAKER)],
            ),
        },
        Draft {
            id: "screen-speaker",
            name: "Screen + Speaker",
            kind: "split",
            description: "A screen recording, shown whole, beside the speaker (stacked on tall frames).",
            seconds: 0,
            draw: draw_split,
            controls: Vec::new(),
            bindings: vec![bind(BG, "fill_color", "color.background"), window_fill(SCREEN), window_fill(SPEAKER)],
        },
        Draft {
            id: "screen-bubble",
            name: "Screen + Bubble",
            kind: "media",
            description: "A screen recording fills the frame; the speaker rides in a round window in the corner.",
            seconds: 0,
            draw: draw_screen_bubble,
            controls: Vec::new(),
            bindings: vec![window_fill(SCREEN), window_fill(SPEAKER), bind(SPEAKER, "stroke_color", "color.accent")],
        },
        Draft {
            id: "title",
            name: "Title Card",
            kind: "title",
            description: "A heading and a line under it on the pack's background.",
            seconds: 4,
            draw: draw_title,
            controls: vec![
                ctl("heading", "Title", HEADING, "text"),
                ctl("subheading", "Subtitle", SUB, "text"),
                slider("heading_size", "Title Size", HEADING, "size", 24.0, 300.0),
            ],
            bindings: with(text_bindings(true, true), vec![bind(BG, "fill_color", "color.background"), bind(ACCENT, "fill_color", "color.accent")]),
        },
        Draft {
            id: "chapter",
            name: "Chapter Card",
            kind: "chapter",
            description: "A large chapter number with the chapter's title.",
            seconds: 3,
            draw: draw_chapter,
            controls: vec![ctl("number", "Number", NUMBER, "text"), ctl("heading", "Title", HEADING, "text"), ctl("kicker", "Kicker", KICKER, "text")],
            bindings: with(
                text_bindings(true, false),
                vec![
                    bind(BG, "fill_color", "color.surface"),
                    bind(ACCENT, "fill_color", "color.accent"),
                    bind(NUMBER, "fill_color", "color.accent"),
                    bind(NUMBER, "font", "font.heading"),
                    bind(KICKER, "fill_color", "color.muted"),
                    bind(KICKER, "font", "font.body"),
                ],
            ),
        },
        Draft {
            id: "quote",
            name: "Pull Quote",
            kind: "quote",
            description: "A quotation, with the speaker in a round window beside their name.",
            seconds: 0,
            draw: draw_quote,
            controls: vec![ctl("quote", "Quote", QUOTE, "text"), ctl("name", "Name", HEADING, "text"), ctl("role", "Role", SUB, "text")],
            bindings: with(
                text_bindings(true, true),
                vec![
                    bind(BG, "fill_color", "color.background"),
                    bind(MARK, "fill_color", "color.accent"),
                    bind(MARK, "font", "font.quote"),
                    bind(QUOTE, "fill_color", "color.text"),
                    bind(QUOTE, "font", "font.quote"),
                    window_fill(SPEAKER),
                    bind(SPEAKER, "stroke_color", "color.accent"),
                ],
            ),
        },
        Draft {
            id: "outro",
            name: "End Card",
            kind: "outro",
            description: "A centred sign-off.",
            seconds: 5,
            draw: draw_outro,
            controls: vec![ctl("heading", "Heading", HEADING, "text"), ctl("subheading", "Line", SUB, "text")],
            bindings: with(text_bindings(true, true), vec![bind(BG, "fill_color", "color.background"), bind(ACCENT, "fill_color", "color.accent")]),
        },
    ]
    .into_iter()
    .map(Draft::build)
    .collect()
}

fn theme(colors: &[(&str, &str)], fonts: &[(&str, &str, &str)]) -> Theme {
    Theme {
        colors: colors.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        fonts: fonts.iter().map(|(k, f, s)| (k.to_string(), ThemeFont { family: f.to_string(), style: s.to_string() })).collect(),
    }
}

/// The built-in packs: one set of original FilmCraft layouts in three themes, drawn in code for
/// 16:9, 9:16, 1:1 and 4:5 frames with the bundled fonts (no image, font or pack files).
pub fn builtin_packs() -> Vec<LayoutPack> {
    let fonts = [("heading", "Inter", "Bold"), ("body", "Inter", "Regular"), ("quote", "Noto Serif", "Regular")];
    let studio = LayoutPack {
        id: "builtin:studio".into(),
        name: "Studio".into(),
        description: "Dark and quiet, with one warm accent. Speaker, screen, title, chapter, quote and end layouts.".into(),
        author: "FilmCraft contributors".into(),
        license: "MIT OR Apache-2.0".into(),
        tags: vec!["dark".into(), "talking head".into(), "tutorial".into()],
        theme: theme(&[("background", "#0f1115"), ("surface", "#1b1f27"), ("text", "#f5f6f8"), ("muted", "#a3acba"), ("accent", "#ff5a36")], &fonts),
        layouts: studio_layouts(),
        ..Default::default()
    };
    let mut paper = studio.remix(
        "builtin:paper",
        "Paper",
        &theme(&[("background", "#f4f1ea"), ("surface", "#ffffff"), ("text", "#16181d"), ("muted", "#5d6470"), ("accent", "#1f5eff")], &[]),
    );
    paper.description = "Light and editorial: warm paper, dark type, a blue accent.".into();
    paper.tags = vec!["light".into(), "editorial".into()];
    let mut harbor = studio.remix(
        "builtin:harbor",
        "Harbor",
        &theme(
            &[("background", "#0b1f33"), ("surface", "#12304d"), ("text", "#f2f7fb"), ("muted", "#9db6cc"), ("accent", "#c9a24a")],
            &[("heading", "Noto Serif", "Regular")],
        ),
    );
    harbor.description = "Navy and brass with a serif heading: measured, professional.".into();
    harbor.tags = vec!["dark".into(), "professional".into(), "serif".into()];
    vec![studio, paper, harbor]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphic::{MediaProps, eval_layer};

    fn specs(layers: &[EffectInstance], canvas: (u32, u32)) -> Vec<LayerSpec> {
        layers.iter().filter_map(|e| eval_layer(e, Tick::ZERO, canvas)).collect()
    }

    #[test]
    fn builtin_packs_are_valid_and_round_trip() {
        let packs = builtin_packs();
        assert_eq!(packs.len(), 3);
        for p in &packs {
            p.validate().unwrap_or_else(|e| panic!("{}: {e}", p.id));
            assert_eq!(p.layouts.len(), 9);
            let back = LayoutPack::from_bytes(p.to_json().as_bytes()).unwrap_or_else(|e| panic!("{}: {e}", p.id));
            if &back != p {
                let (a, b) = (p.to_json(), back.to_json());
                let d = a.lines().zip(b.lines()).enumerate().find(|(_, (x, y))| x != y);
                panic!("{}: round trip differs at {d:?}", p.id);
            }
            for l in &p.layouts {
                assert_eq!(l.designs.len(), SHAPES.len());
                assert!(LAYOUT_KINDS.contains(&l.kind.as_str()), "{}", l.kind);
                // every design has the same layers (uids), so controls and bindings hold everywhere
                let uids = |d: &Design| d.layers.iter().map(layer_uid).collect::<Vec<_>>();
                assert!(l.designs.iter().all(|d| uids(d) == uids(&l.designs[0])), "{}", l.id);
            }
        }
    }

    #[test]
    fn pack_files_leave_out_default_parameters() {
        let studio = builtin_packs().remove(0);
        let json = studio.to_json();
        assert!(json.len() < 400_000, "a pack file of {} bytes", json.len());
        // a background keeps what the design set and drops the rest
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let bg = &v["layouts"][1]["designs"][0]["layers"][0];
        assert_eq!(bg["params"]["name"]["value"]["Text"], "Background");
        assert!(bg["params"]["size"].is_object() && bg["params"]["shadow_blur"].is_null() && bg["params"]["stroke_width"].is_null(), "{bg}");
        // a hand-written layer with only what matters reads back whole
        let mut small = v.clone();
        small["layouts"][1]["designs"][0]["layers"][0]["params"] = serde_json::json!({"name": {"value": {"Text": "Background"}}});
        let back = LayoutPack::from_bytes(small.to_string().as_bytes()).unwrap();
        let layer = &back.layouts[1].designs[0].layers[0];
        assert!(layer.params.contains_key("fill_color") && layer.params.contains_key("opacity"));
        assert!(eval_layer(layer, Tick::ZERO, (1920, 1080)).is_some());
    }

    #[test]
    fn instantiate_picks_the_nearest_shape_and_themes_it() {
        let packs = builtin_packs();
        let (studio, paper) = (&packs[0], &packs[1]);
        let l = studio.layout("speaker-framed").unwrap();
        assert_eq!(l.design_for((3840, 2160)).unwrap().canvas, [1920, 1080]);
        assert_eq!(l.design_for((720, 1280)).unwrap().canvas, [1080, 1920]);
        assert_eq!(l.design_for((1000, 1000)).unwrap().canvas, [1080, 1080]);
        let (layers, meta) = studio.instantiate(l, (3840, 2160)).unwrap();
        let link = meta.layout.as_ref().unwrap();
        assert_eq!((link.pack.as_str(), link.layout.as_str(), link.kind.as_str()), ("builtin:studio", "speaker-framed", "camera"));
        assert_eq!(meta.template.as_ref().unwrap().controls.len(), 2);
        assert!(meta.smart.as_ref().unwrap().active());
        let sp = specs(&layers, (3840, 2160));
        let bg = sp.iter().find(|s| s.name == "Background").unwrap();
        assert_eq!(bg.appearance.fill, parse_hex_color("#0f1115"));
        let win = sp.iter().find(|s| s.name == "Speaker").unwrap();
        // a 4K frame doubles the 1080p design
        assert_eq!(win.transform.scale, Vec2::new(2.0, 2.0));
        assert!(matches!(&win.content, LayerContent::Media(m) if m.role == "camera"));
        // the same layout from the other pack differs only in its theme
        let (layers2, _) = paper.instantiate(paper.layout("Speaker \u{2013} Framed").unwrap(), (3840, 2160)).unwrap();
        let bg2 = specs(&layers2, (3840, 2160)).into_iter().find(|s| s.name == "Background").unwrap();
        assert_eq!(bg2.appearance.fill, parse_hex_color("#f4f1ea"));
    }

    #[test]
    fn remix_and_apply_theme() {
        let studio = &builtin_packs()[0];
        let mine = studio.remix("user:mine", "Mine", &theme(&[("accent", "#00ff00")], &[("heading", "Noto Serif", "Regular")]));
        mine.validate().unwrap();
        assert_eq!(mine.theme.colors["background"], "#0f1115", "untouched entries stay");
        let (layers, _) = mine.instantiate(mine.layout("title").unwrap(), (1920, 1080)).unwrap();
        let sp = specs(&layers, (1920, 1080));
        assert_eq!(sp.iter().find(|s| s.name == "Accent").unwrap().appearance.fill, Some([0.0, 1.0, 0.0, 1.0]));
        let LayerContent::Text(t) = &sp.iter().find(|s| s.name == "Heading").unwrap().content else { panic!() };
        assert_eq!((t.font.as_str(), t.style.as_str()), ("Noto Serif", "Regular"));
        // restyling placed layers: bindings to missing layers or tokens are skipped
        let mut layers = layers;
        let n = apply_theme(
            &mut layers,
            &[bind(ACCENT, "fill_color", "color.text"), bind(999, "fill_color", "color.text"), bind(ACCENT, "fill_color", "color.nope")],
            &mine.theme,
        );
        assert_eq!(n, 1);
    }

    #[test]
    fn hex_colours() {
        assert_eq!(parse_hex_color("#ff0000"), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_hex_color(" #00ff0080 ").map(|c| (c[1], (c[3] * 255.0).round())), Some((1.0, 128.0)));
        for bad in ["", "#", "ff0000", "#ff00", "#gg0000", "#ff00000", "#ñ00000", "#ff0000ff0"] {
            assert_eq!(parse_hex_color(bad), None, "{bad}");
        }
        assert_eq!(hex_color([1.0, 0.0, 0.0, 1.0]), "#ff0000");
        assert_eq!(hex_color([0.0, 0.0, 0.0, 0.5]), "#00000080");
        assert_eq!(hex_color([f32::NAN, 2.0, -1.0, 1.0]), "#00ff00");
    }

    #[test]
    fn easing_ends_and_shape() {
        for e in Easing::ALL {
            assert!(e.apply(0.0).abs() < 1e-12, "{e:?}");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-12, "{e:?}");
            assert!(e.apply(-3.0).abs() < 1e-12);
            assert!((e.apply(f64::NAN) - 1.0).abs() < 1e-12);
            assert_eq!(Easing::from_name(e.name()), Some(e));
            assert_eq!(Easing::from_name(e.label()), Some(e));
        }
        assert!(Easing::EaseIn.apply(0.5) < 0.5 && Easing::EaseOut.apply(0.5) > 0.5);
        assert!((Easing::EaseInOut.apply(0.5) - 0.5).abs() < 1e-12);
        assert!((0..100).any(|i| Easing::Overshoot.apply(i as f64 / 100.0) > 1.0));
        assert_eq!(Easing::from_name("bounce"), None);
        let t = SmartTransition::default();
        assert!(t.progress(Tick(0)).is_some_and(|p| p.abs() < 1e-12));
        assert_eq!(t.progress(t.duration), None);
        assert_eq!(t.progress(Tick(-1)), None);
        assert_eq!(SmartTransition { enabled: false, ..t.clone() }.progress(Tick(0)), None);
        assert_eq!(SmartTransition { duration: Tick(0), ..t }.progress(Tick(0)), None);
    }

    #[test]
    fn tween_moves_what_is_shared_and_fades_the_rest() {
        let studio = &builtin_packs()[0];
        let c = (1920, 1080);
        let of = |id: &str| specs(&studio.instantiate(studio.layout(id).unwrap(), c).unwrap().0, c);
        let (full, framed) = (of("speaker-full"), of("speaker-framed"));
        let find = |v: &[LayerSpec], n: &str| v.iter().find(|s| s.name == n).cloned();
        let size = |s: &LayerSpec| match &s.content {
            LayerContent::Media(m) => m.shape.size,
            LayerContent::Shape(m) => m.size,
            LayerContent::Text(_) => (0.0, 0.0),
        };
        // ends: nothing of the outgoing layout is left at 1, and it is the incoming one exactly
        let end = tween_layers(&full, &framed, 1.0, true);
        assert_eq!(end.len(), framed.len());
        for (a, b) in end.iter().zip(&framed) {
            assert_eq!((a.name.as_str(), &a.content, &a.transform), (b.name.as_str(), &b.content, &b.transform));
        }
        // half way: the speaker window is between its two boxes, the plate (outgoing only) and the
        // background (incoming only) are half faded
        let mid = tween_layers(&full, &framed, 0.5, true);
        let (w0, w1, wm) = (find(&full, "Speaker").unwrap(), find(&framed, "Speaker").unwrap(), find(&mid, "Speaker").unwrap());
        assert!((size(&wm).0 - (size(&w0).0 + size(&w1).0) / 2.0).abs() < 1e-3);
        assert!((wm.transform.position.y - (w0.transform.position.y + w1.transform.position.y) / 2.0).abs() < 1e-6);
        assert!((find(&mid, "Plate").unwrap().transform.opacity - 0.94 * 0.5).abs() < 1e-6);
        assert!((find(&mid, "Background").unwrap().transform.opacity - 0.5).abs() < 1e-6);
        // the heading reads differently in the two layouts: both are there, cross-fading
        assert_eq!(mid.iter().filter(|s| s.name == "Heading").count(), 2);
        // the accent bar is in both: one bar, moving
        assert_eq!(mid.iter().filter(|s| s.name == "Accent").count(), 1);
        // order: the outgoing plate stays above the window it sat on
        let pos = |n: &str| mid.iter().position(|s| s.name == n).unwrap();
        assert!(pos("Background") < pos("Speaker") && pos("Speaker") < pos("Plate"));
        // without fades the outgoing-only layers are gone at once and the incoming ones are whole
        let cut = tween_layers(&full, &framed, 0.25, false);
        assert!(find(&cut, "Plate").is_none());
        assert_eq!(find(&cut, "Background").unwrap().transform.opacity, 1.0);
        // overshoot runs past the end but opacity and sizes stay sane
        let over = tween_layers(&framed, &full, 1.08, true);
        assert!(over.iter().all(|s| (0.0..=1.0).contains(&s.transform.opacity) && size(s).0 >= 0.0));
        // hostile progress
        assert_eq!(tween_layers(&full, &framed, f64::NAN, true).len(), framed.len());
        assert!(tween_layers(&[], &[], 0.5, true).is_empty());
    }

    #[test]
    fn media_placement() {
        let mut m = MediaProps {
            shape: crate::graphic::ShapeProps { shape: 0, size: (400.0, 400.0), sides: 4, corner_radius: 0.0, points: vec![] },
            source: 0,
            fit: 0,
            focus: (0.5, 0.5),
            zoom: 1.0,
            role: String::new(),
        };
        // fill: a 16:9 frame covers a square box, cropped equally left and right
        let (x, y, w, h) = m.placement((1920.0, 1080.0));
        assert!((h - 400.0).abs() < 1e-9 && (w - 400.0 * 16.0 / 9.0).abs() < 1e-9);
        assert!((y + 200.0).abs() < 1e-9 && (x + w / 2.0).abs() < 1e-9);
        // focus left: the frame's left edge is at the box's left edge
        m.focus = (0.0, 0.5);
        assert!((m.placement((1920.0, 1080.0)).0 + 200.0).abs() < 1e-9);
        // fit: the whole frame, centred, bars above and below
        m.fit = 1;
        let (x, y, w, h) = m.placement((1920.0, 1080.0));
        assert!((w - 400.0).abs() < 1e-9 && (x + 200.0).abs() < 1e-9 && (y + h / 2.0).abs() < 1e-9 && h < 400.0);
        // stretch: exactly the box
        m.fit = 2;
        assert_eq!(m.placement((1920.0, 1080.0)), (-200.0, -200.0, 400.0, 400.0));
        // zoom 2 doubles the picture; degenerate inputs give finite numbers
        m.zoom = 2.0;
        assert_eq!(m.placement((1920.0, 1080.0)).2, 800.0);
        m.zoom = f32::NAN;
        m.shape.size = (0.0, 0.0);
        let p = m.placement((0.0, 0.0));
        assert!(p.0.is_finite() && p.1.is_finite() && p.2.is_finite() && p.3.is_finite());
    }

    #[test]
    fn hostile_packs_are_refused_not_crashed() {
        let good = builtin_packs().remove(0);
        let json = good.to_json();
        assert_eq!(LayoutPack::from_bytes(b"PK\x03\x04rest"), Err(PackError::ForeignPackage));
        assert!(matches!(LayoutPack::from_bytes(b"\xff\xfe"), Err(PackError::NotPack(_))));
        assert!(matches!(LayoutPack::from_bytes(b"{}"), Err(PackError::NotPack(_))));
        assert!(matches!(LayoutPack::from_bytes(br#"{"format":"filmcraft.graphicsTemplate","version":1}"#), Err(PackError::NotPack(_))));
        assert_eq!(LayoutPack::from_bytes(br#"{"format":"filmcraft.layoutPack","version":99}"#), Err(PackError::TooNew(99)));
        assert!(matches!(LayoutPack::from_bytes(br#"{"format":"filmcraft.layoutPack"}"#), Err(PackError::Invalid(_))));
        let broken = |f: &dyn Fn(&mut LayoutPack)| {
            let mut p = good.clone();
            f(&mut p);
            matches!(LayoutPack::from_bytes(p.to_json().as_bytes()), Err(PackError::Invalid(_)))
        };
        assert!(broken(&|p| p.name.clear()));
        assert!(broken(&|p| p.layouts.clear()));
        assert!(broken(&|p| p.layouts[0].id = "Has Spaces".into()));
        assert!(broken(&|p| p.layouts[1].id = p.layouts[0].id.clone()));
        assert!(broken(&|p| p.layouts[0].designs.clear()));
        assert!(broken(&|p| p.layouts[0].designs[0].canvas = [0, 1080]));
        assert!(broken(&|p| p.layouts[0].designs[0].canvas = [u32::MAX, 1080]));
        assert!(broken(&|p| p.layouts[0].designs[0].layers.clear()));
        assert!(broken(&|p| p.layouts[0].designs[0].layers[0].effect = "gaussian_blur".into()));
        assert!(broken(&|p| p.layouts[0].designs[0].layers[1].layer = p.layouts[0].designs[0].layers[0].layer.clone()));
        assert!(broken(&|p| p.layouts[0].controls[0].layer = 4242));
        assert!(broken(&|p| p.layouts[0].controls[0].param = "nope".into()));
        assert!(broken(&|p| p.layouts[0].bindings[0].token = "color.nope".into()));
        assert!(broken(&|p| p.layouts[0].bindings[0].layer = 4242));
        assert!(broken(&|p| {
            p.theme.colors.insert("accent".into(), "red".into());
        }));
        assert!(broken(&|p| p.layouts = vec![p.layouts[0].clone(); MAX_LAYOUTS + 1]));
        // durations are clamped rather than refused
        let mut p = good.clone();
        p.transition.duration = Tick(i64::MAX);
        p.layouts[0].duration = Tick(i64::MIN);
        let back = LayoutPack::from_bytes(p.to_json().as_bytes()).unwrap();
        assert_eq!((back.transition.duration, back.layouts[0].duration), (Tick(MAX_DURATION), Tick(0)));
        // truncations and bit flips of a real pack never panic
        let bytes = json.as_bytes();
        for cut in (0..bytes.len()).step_by(bytes.len() / 200 + 1) {
            let _ = LayoutPack::from_bytes(&bytes[..cut]);
        }
        let mut b = bytes.to_vec();
        for i in (0..b.len()).step_by(b.len() / 400 + 1) {
            b[i] ^= 0x15;
            let _ = LayoutPack::from_bytes(&b);
            b[i] ^= 0x15;
        }
    }
}
