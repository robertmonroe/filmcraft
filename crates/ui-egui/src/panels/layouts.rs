//! Essential Graphics ▸ Layouts: the layout pack library, and the Remix Layout Pack and Save As
//! Layout dialogs.
//!
//! The tab lists the layouts of one pack as cards with thumbnails rendered by the engine, with a
//! pack chooser and a kind filter. Click selects, double-click (or Apply to Scene) lays the scene
//! under the playhead out with it. Restyle Sequence gives every layout clip of the sequence the
//! chosen pack's look. With a layout clip selected, its smart transition is edited underneath.
//! Everything runs engine commands (`layout.*`, see `docs/layout-packs.md`).
//!
//! Automation ids: `essentialGraphics.tab.layouts`, `layouts.pack`, `layouts.kind`,
//! `layouts.item.<layout id>`, `layouts.apply`, `layouts.restyle`, `layouts.install`,
//! `layouts.remix`, `layouts.transition.<enabled|seconds|easing|fades>`; dialogs:
//! `remixPack.name|color.<name>|font.<name>|ok|cancel`, `saveLayout.pack|name|kind|ok|cancel`.
//! The UI state (`UiState::layouts`) is serde, so agents can read and set it.

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_engine::layouts::{PackEntry, library, scenes, thumbnail_cached};
use filmcraft_project::layoutpack::{Easing, LAYOUT_KINDS, SmartTransition, hex_color, parse_hex_color};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::theme::Tokens;

/// The Remix Layout Pack dialog.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemixDraft {
    /// The pack being remixed (id).
    pub pack: String,
    pub name: String,
    /// Theme colour → `#rrggbb`.
    pub colors: BTreeMap<String, String>,
    /// Theme font → family.
    pub fonts: BTreeMap<String, String>,
    pub error: String,
}

/// The Save As Layout dialog.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveDraft {
    pub clip: u64,
    pub pack: String,
    pub name: String,
    pub kind: String,
    pub error: String,
}

/// Frontend state (`UiState::layouts`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutsState {
    /// The pack shown (id; "" = the first).
    pub pack: String,
    /// Kind filter ("" = all).
    pub kind: String,
    /// The selected layout (id).
    pub selected: Option<String>,
    pub remix: Option<RemixDraft>,
    pub save: Option<SaveDraft>,
}

type Elems = Vec<(String, Rect, String)>;

#[derive(Clone)]
struct Lib(Arc<(f64, Vec<PackEntry>)>);

fn lib_id() -> egui::Id {
    egui::Id::new("layout-pack-library")
}

/// The pack library, re-read at most every two seconds (or after [`invalidate`]).
fn cached_library(app: &FilmcraftApp, ctx: &egui::Context) -> Vec<PackEntry> {
    let now = ctx.input(|i| i.time);
    if let Some(Lib(l)) = ctx.data(|d| d.get_temp::<Lib>(lib_id()))
        && now - l.0 < 2.0
    {
        return l.1.clone();
    }
    let v = library(&app.session);
    ctx.data_mut(|d| d.insert_temp(lib_id(), Lib(Arc::new((now, v.clone())))));
    v
}

fn invalidate(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<Lib>(lib_id()));
}

const THUMB_W: u32 = 224;

fn frame_shape(app: &FilmcraftApp) -> (u32, u32) {
    app.session.active_sequence().map_or((1920, 1080), |q| (q.settings.width.max(1), q.settings.height.max(1)))
}

fn thumb_texture(ctx: &egui::Context, e: &PackEntry, layout: &filmcraft_project::layoutpack::Layout, shape: (u32, u32)) -> egui::TextureHandle {
    let px = thumbnail_cached(&e.pack, layout, shape, THUMB_W);
    let key = egui::Id::new(("layout-thumb", e.pack.id.as_str(), layout.id.as_str(), Arc::as_ptr(&px) as usize));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(key)) {
        return t;
    }
    let img = egui::ColorImage::from_rgba_unmultiplied([px.0 as usize, px.1 as usize], &px.2);
    let t = ctx.load_texture(format!("layout-thumb-{}-{}", e.pack.id, layout.id), img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(key, t.clone()));
    t
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "camera" => tl!("Speaker"),
        "media" => tl!("Media"),
        "split" => tl!("Split Screen"),
        "title" => tl!("Title"),
        "chapter" => tl!("Chapter"),
        "quote" => tl!("Quote"),
        "outro" => tl!("End"),
        "overlay" => tl!("Overlay"),
        _ => tl!("Other"),
    }
}

fn easing_label(e: Easing) -> &'static str {
    match e {
        Easing::Linear => tl!("Linear"),
        Easing::EaseIn => tl!("Ease In"),
        Easing::EaseOut => tl!("Ease Out"),
        Easing::EaseInOut => tl!("Ease In and Out"),
        Easing::Overshoot => tl!("Overshoot"),
    }
}

/// The selected layout clip and its smart transition.
fn selected_layout_clip(app: &FilmcraftApp) -> Option<(u64, SmartTransition)> {
    let clip = filmcraft_engine::graphics::target_clip(&app.session, &Value::Null)?;
    let (_, it) = app.session.active_sequence()?.find_item(clip)?;
    let g = it.graphic.as_deref()?;
    g.layout.as_ref()?;
    Some((clip.0, g.smart.clone().unwrap_or_default()))
}

/// The Layouts tab of the Essential Graphics panel.
pub fn tab(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let lib = cached_library(app, &ctx);
    let shape = frame_shape(app);
    let mut elems: Elems = Vec::new();
    let mut actions: Vec<(String, Value)> = Vec::new();
    let (mut install, mut remix) = (false, false);
    let st = &mut app.ui.layouts;
    if !lib.iter().any(|e| e.pack.id == st.pack) {
        st.pack = lib.first().map(|e| e.pack.id.clone()).unwrap_or_default();
    }
    let Some(entry) = lib.iter().find(|e| e.pack.id == st.pack) else {
        crate::dock::placeholder(ui, rect, &t, tl!("No layout packs"));
        return;
    };
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 6.0))).id_salt("layouts-tab"));
    b.set_clip_rect(rect);
    // two short rows, so that nothing is lost in a narrow panel
    b.horizontal(|ui| {
        let r = egui::ComboBox::from_id_salt("layouts-pack").selected_text(crate::i18n::t(&entry.pack.name)).width(140.0).show_ui(ui, |ui| {
            for e in &lib {
                ui.selectable_value(&mut st.pack, e.pack.id.clone(), crate::i18n::t(&e.pack.name));
            }
        });
        elems.push(("layouts.pack".into(), r.response.rect, "Pack".into()));
        let cur = if st.kind.is_empty() { tl!("All Kinds") } else { kind_label(&st.kind) };
        let r = egui::ComboBox::from_id_salt("layouts-kind").selected_text(cur).width(110.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut st.kind, String::new(), tl!("All Kinds"));
            for k in LAYOUT_KINDS.iter().filter(|k| entry.pack.layouts.iter().any(|l| l.kind == **k)) {
                ui.selectable_value(&mut st.kind, (*k).to_string(), kind_label(k));
            }
        });
        elems.push(("layouts.kind".into(), r.response.rect, "Kind".into()));
    });
    b.horizontal(|ui| {
        let r = ui.button(tl!("Remix…")).on_hover_text(tl!("Save this pack's layouts in your own colors and fonts"));
        elems.push(("layouts.remix".into(), r.rect, "Remix".into()));
        remix = r.clicked();
        let r = ui.button(tl!("Install…")).on_hover_text(tl!("Install Layout Pack (.fcpack)"));
        elems.push(("layouts.install".into(), r.rect, "Install".into()));
        install = r.clicked();
        if !entry.pack.description.is_empty() {
            ui.add(egui::Label::new(egui::RichText::new(crate::i18n::t(&entry.pack.description)).color(t.text_faint).size(10.5)).truncate());
        }
    });
    b.add_space(6.0);
    let shown: Vec<&filmcraft_project::layoutpack::Layout> = entry.pack.layouts.iter().filter(|l| st.kind.is_empty() || l.kind == st.kind).collect();
    let selected = st.selected.clone().filter(|s| entry.pack.layouts.iter().any(|l| &l.id == s));
    // cards take the sequence's shape, within reason
    let aspect = (shape.0 as f32 / shape.1 as f32).clamp(0.5, 2.4);
    let thumb = if aspect >= 1.0 { vec2(136.0, 136.0 / aspect) } else { vec2(150.0 * aspect, 150.0) };
    let card = vec2(thumb.x + 10.0, thumb.y + 10.0 + 34.0);
    let avail = (b.available_width()).max(card.x);
    let cols = ((avail + 8.0) / (card.x + 8.0)).floor().max(1.0) as usize;
    let mut new_sel: Option<String> = None;
    let mut apply: Option<String> = None;
    let smart = selected_layout_clip(app);
    let footer = if smart.is_some() { 96.0 } else { 40.0 };
    egui::ScrollArea::vertical().id_salt("layouts-scroll").max_height((rect.max.y - b.cursor().min.y - footer).max(60.0)).auto_shrink([false, false]).show(
        &mut b,
        |ui| {
            if shown.is_empty() {
                ui.label(egui::RichText::new(tl!("No layouts of this kind.")).color(t.text_faint));
            }
            for row in shown.chunks(cols) {
                ui.horizontal(|ui| {
                    for l in row {
                        let (r, resp) = ui.allocate_exact_size(card, Sense::click());
                        let on = selected.as_deref() == Some(l.id.as_str());
                        ui.painter().rect_filled(
                            r,
                            4.0,
                            if on {
                                t.row_selected
                            } else if resp.hovered() {
                                t.hover
                            } else {
                                t.field_bg
                            },
                        );
                        let ir = Rect::from_min_size(r.min + vec2(5.0, 5.0), thumb);
                        ui.painter().rect_filled(ir, 2.0, Color32::from_rgb(0x2a, 0x30, 0x3a));
                        let tex = thumb_texture(ui.ctx(), entry, l, shape);
                        let sz = tex.size_vec2();
                        let k = (ir.width() / sz.x).min(ir.height() / sz.y);
                        let tr = Rect::from_center_size(ir.center(), sz * k);
                        ui.painter().image(tex.id(), tr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        ui.painter().text(pos2(r.min.x + 6.0, ir.max.y + 6.0), Align2::LEFT_TOP, crate::i18n::t(&l.name), Tokens::ui(11.5), t.text);
                        ui.painter().text(pos2(r.min.x + 6.0, ir.max.y + 20.0), Align2::LEFT_TOP, kind_label(&l.kind), Tokens::ui(10.0), t.text_faint);
                        if on {
                            ui.painter().rect_stroke(r, 4.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
                        }
                        elems.push((format!("layouts.item.{}", l.id), r, l.name.clone()));
                        let resp = resp.on_hover_text(crate::i18n::t(&l.description));
                        if resp.double_clicked() {
                            apply = Some(l.id.clone());
                        } else if resp.clicked() {
                            new_sel = Some(l.id.clone());
                        }
                    }
                });
                ui.add_space(6.0);
            }
        },
    );
    b.add_space(4.0);
    // the scene the playhead is in
    let scene = app.session.active_sequence().and_then(|q| {
        let list = scenes(&app.session.project, q);
        let ph = app.session.playhead();
        let i = list.iter().position(|s| s.start <= ph && ph < s.end).or_else(|| list.len().checked_sub(1).filter(|&i| ph >= list[i].end))?;
        Some((i, list.len()))
    });
    let pack_id = entry.pack.id.clone();
    b.horizontal(|ui| {
        let r = ui
            .add_enabled(selected.is_some() && scene.is_some(), egui::Button::new(tl!("Apply to Scene")))
            .on_hover_text(tl!("Lay out the scene under the playhead"));
        elems.push(("layouts.apply".into(), r.rect, "Apply to Scene".into()));
        if r.clicked() {
            apply = selected.clone();
        }
        let r = ui
            .add_enabled(scene.is_some(), egui::Button::new(tl!("Restyle Sequence")))
            .on_hover_text(tl!("Give every layout in this sequence this pack's colors and fonts"));
        elems.push(("layouts.restyle".into(), r.rect, "Restyle Sequence".into()));
        if r.clicked() {
            actions.push(("layout.retheme".into(), json!({"to": pack_id})));
        }
        if let Some((i, n)) = scene {
            ui.label(egui::RichText::new(tlf!("Scene {i} of {n}", i = i + 1, n)).color(t.text_faint));
        }
    });
    if let Some((clip, mut tr)) = smart {
        b.add_space(4.0);
        b.separator();
        b.label(egui::RichText::new(tl!("Smart Transition")).font(Tokens::semibold(11.5)).color(t.text));
        let before = tr.clone();
        let mut seconds = tr.duration.seconds();
        b.horizontal(|ui| {
            let r = ui.checkbox(&mut tr.enabled, tl!("On"));
            elems.push(("layouts.transition.enabled".into(), r.rect, "On".into()));
            let r = ui.add_enabled(tr.enabled, egui::DragValue::new(&mut seconds).speed(0.01).range(0.0..=10.0).fixed_decimals(2).suffix(" s"));
            elems.push(("layouts.transition.seconds".into(), r.rect, "Duration".into()));
            let r = egui::ComboBox::from_id_salt("layouts-easing").selected_text(easing_label(tr.easing)).width(120.0).show_ui(ui, |ui| {
                for e in Easing::ALL {
                    ui.selectable_value(&mut tr.easing, e, easing_label(e));
                }
            });
            elems.push(("layouts.transition.easing".into(), r.response.rect, "Easing".into()));
            let r = ui
                .add_enabled(tr.enabled, egui::Checkbox::new(&mut tr.fades, tl!("Fade")))
                .on_hover_text(tl!("Fade the layers only one of the two layouts has"));
            elems.push(("layouts.transition.fades".into(), r.rect, "Fade".into()));
        });
        if tr.enabled != before.enabled || tr.easing != before.easing || tr.fades != before.fades || (seconds - before.duration.seconds()).abs() > 1e-6 {
            actions.push((
                "layout.transition".into(),
                json!({"clip": clip, "enabled": tr.enabled, "seconds": seconds.max(0.0), "easing": tr.easing.name(), "fades": tr.fades}),
            ));
        }
    }
    if let Some(s) = new_sel {
        app.ui.layouts.selected = Some(s);
    }
    if let Some(id) = apply {
        app.ui.layouts.selected = Some(id.clone());
        actions.push(("layout.apply".into(), json!({"pack": pack_id, "layout": id})));
    }
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if install {
        let _ = route(app, &ctx, "layout.pack.install", &json!({}));
    }
    if remix {
        let _ = route(app, &ctx, "layout.pack.remix", &json!({}));
    }
    for (cmd, p) in actions {
        match app.session.execute(&cmd, p) {
            Ok(v) => {
                if cmd == "layout.apply"
                    && let Some(names) = v["unfilled"].as_array().filter(|a| !a.is_empty())
                {
                    let names = names.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ");
                    app.ui.status = tlf!("No footage for: {names}", names);
                }
                invalidate(&ctx);
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Menu routing and dialogs
// ------------------------------------------------------------------------------------------------

/// The pack the tab shows (else the first of the library).
fn current_pack(app: &FilmcraftApp) -> Option<PackEntry> {
    let lib = library(&app.session);
    lib.iter().find(|e| e.pack.id == app.ui.layouts.pack).or(lib.first()).cloned()
}

/// Menu / button entry points of this module (None for other ids).
pub fn route(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let empty = params.as_object().is_none_or(|m| m.is_empty());
    match id {
        "layout.pack.install" if params.get("path").is_none() => {
            let path = app.hooks.pick_open_file.as_mut().and_then(|f| f(tl!("FilmCraft Layout Pack"), &["fcpack"]))?;
            let r = app.session.execute(id, json!({"path": path})).map_err(|e| e.to_string());
            if let Ok(v) = &r
                && let Some(pack) = v["id"].as_str()
            {
                app.ui.layouts.pack = pack.into();
            }
            invalidate(ctx);
            Some(r)
        }
        "layout.pack.export" if empty => {
            let Some(e) = current_pack(app) else { return Some(Err("there is no layout pack".into())) };
            let suggested = format!("{}.fcpack", filmcraft_project::gtemplate::slug(&e.pack.name));
            let path = app.hooks.pick_save_as.as_mut().and_then(|f| f(tl!("FilmCraft Layout Pack"), &["fcpack"], &suggested))?;
            Some(app.session.execute(id, json!({"pack": e.pack.id, "path": path})).map_err(|e| e.to_string()))
        }
        "layout.pack.remix" if empty => {
            let Some(e) = current_pack(app) else { return Some(Err("there is no layout pack".into())) };
            app.ui.layouts.remix = Some(RemixDraft {
                pack: e.pack.id.clone(),
                name: tlf!("My {name}", name = e.pack.name),
                colors: e.pack.theme.colors.clone(),
                fonts: e.pack.theme.fonts.iter().map(|(k, f)| (k.clone(), f.family.clone())).collect(),
                error: String::new(),
            });
            Some(Ok(json!({"dialog": "remixPack"})))
        }
        "layout.save" if empty => {
            let clip = filmcraft_engine::graphics::target_clip(&app.session, &Value::Null);
            let Some(clip) = clip else { return Some(Err("select a graphic clip".into())) };
            let it = app.session.active_sequence().and_then(|q| q.find_item(clip)).map(|(_, i)| i.clone())?;
            let user = library(&app.session).into_iter().find(|e| e.path.is_some()).map(|e| e.pack.name);
            let kind = it.graphic.as_deref().and_then(|g| g.layout.as_ref()).map(|l| l.kind.clone()).unwrap_or_default();
            app.ui.layouts.save =
                Some(SaveDraft { clip: clip.0, pack: user.unwrap_or_else(|| tl!("My Layouts").into()), name: it.name.clone(), kind, error: String::new() });
            Some(Ok(json!({"dialog": "saveLayout"})))
        }
        _ => None,
    }
}

/// Draw this module's dialogs.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    remix_dialog(app, ctx);
    save_dialog(app, ctx);
}

fn remix_dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.layouts.remix.clone() else { return };
    let mut elems: Elems = Vec::new();
    let mut action: Option<bool> = None;
    egui::Window::new(tl!("Remix Layout Pack"))
        .id(egui::Id::new("Remix Layout Pack"))
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Grid::new("remix-pack-grid").num_columns(2).show(ui, |ui| {
                ui.label(tl!("Name:"));
                let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(240.0));
                elems.push(("remixPack.name".into(), r.rect, "Name".into()));
                ui.end_row();
                for (name, hex) in d.colors.iter_mut() {
                    ui.label(format!("{}:", crate::i18n::t(&title_case(name))));
                    let c = parse_hex_color(hex).unwrap_or([0.0, 0.0, 0.0, 1.0]);
                    let mut rgb = [(c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8];
                    let r = ui.color_edit_button_srgb(&mut rgb);
                    if r.changed() {
                        *hex = hex_color([rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0, 1.0]);
                    }
                    elems.push((format!("remixPack.color.{name}"), r.rect, name.clone()));
                    ui.end_row();
                }
                for (name, family) in d.fonts.iter_mut() {
                    ui.label(tlf!("{name} font:", name = crate::i18n::t(&title_case(name))));
                    let r = ui.add(egui::TextEdit::singleline(family).desired_width(240.0));
                    elems.push((format!("remixPack.font.{name}"), r.rect, name.clone()));
                    ui.end_row();
                }
            });
            ui.label(egui::RichText::new(tl!("Saved as a FilmCraft layout pack (.fcpack) in your layout packs folder.")).weak().size(11.0));
            if !d.error.is_empty() {
                ui.colored_label(Color32::from_rgb(0xff, 0x80, 0x80), &d.error);
            }
            ui.horizontal(|ui| {
                let ok = ui.button(tl!("Save"));
                elems.push(("remixPack.ok".into(), ok.rect, "Save".into()));
                let cancel = ui.button(tl!("Cancel"));
                elems.push(("remixPack.cancel".into(), cancel.rect, "Cancel".into()));
                if ok.clicked() {
                    action = Some(true);
                }
                if cancel.clicked() {
                    action = Some(false);
                }
            });
        });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    match action {
        Some(true) => {
            // a font keeps its style unless the family changed (then Regular)
            let base = library(&app.session).into_iter().find(|e| e.pack.id == d.pack).map(|e| e.pack.theme.fonts).unwrap_or_default();
            let fonts: serde_json::Map<String, Value> = d
                .fonts
                .iter()
                .map(|(k, family)| {
                    let style = base.get(k).filter(|f| &f.family == family).map_or("Regular", |f| f.style.as_str());
                    (k.clone(), json!({"family": family, "style": style}))
                })
                .collect();
            match app.session.execute("layout.pack.remix", json!({"pack": d.pack, "name": d.name, "colors": d.colors, "fonts": fonts})) {
                Ok(v) => {
                    app.ui.layouts.remix = None;
                    if let Some(id) = v["id"].as_str() {
                        app.ui.layouts.pack = id.into();
                    }
                    app.ui.status = tlf!("Saved layout pack to {path}", path = v["path"].as_str().unwrap_or_default());
                    invalidate(ctx);
                }
                Err(e) => {
                    d.error = e.to_string();
                    app.ui.layouts.remix = Some(d);
                }
            }
        }
        Some(false) => app.ui.layouts.remix = None,
        None => app.ui.layouts.remix = Some(d),
    }
}

/// `background` → `Background`.
fn title_case(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn save_dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.layouts.save.clone() else { return };
    let mut elems: Elems = Vec::new();
    let mut action: Option<bool> = None;
    egui::Window::new(tl!("Save As Layout"))
        .id(egui::Id::new("Save As Layout"))
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Grid::new("save-layout-grid").num_columns(2).show(ui, |ui| {
                ui.label(tl!("Pack:"));
                let r = ui.add(egui::TextEdit::singleline(&mut d.pack).desired_width(240.0));
                elems.push(("saveLayout.pack".into(), r.rect, "Pack".into()));
                ui.end_row();
                ui.label(tl!("Name:"));
                let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(240.0));
                elems.push(("saveLayout.name".into(), r.rect, "Name".into()));
                ui.end_row();
                ui.label(tl!("Kind:"));
                let cur = if d.kind.is_empty() { tl!("Automatic") } else { kind_label(&d.kind) };
                let r = egui::ComboBox::from_id_salt("save-layout-kind").selected_text(cur).width(140.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.kind, String::new(), tl!("Automatic"));
                    for k in LAYOUT_KINDS {
                        ui.selectable_value(&mut d.kind, (*k).to_string(), kind_label(k));
                    }
                });
                elems.push(("saveLayout.kind".into(), r.response.rect, "Kind".into()));
                ui.end_row();
            });
            ui.label(egui::RichText::new(tl!("The clip is added to the pack as a layout. A pack with this name is made if you have none.")).weak().size(11.0));
            if !d.error.is_empty() {
                ui.colored_label(Color32::from_rgb(0xff, 0x80, 0x80), &d.error);
            }
            ui.horizontal(|ui| {
                let ok = ui.button(tl!("Save"));
                elems.push(("saveLayout.ok".into(), ok.rect, "Save".into()));
                let cancel = ui.button(tl!("Cancel"));
                elems.push(("saveLayout.cancel".into(), cancel.rect, "Cancel".into()));
                if ok.clicked() {
                    action = Some(true);
                }
                if cancel.clicked() {
                    action = Some(false);
                }
            });
        });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    match action {
        Some(true) => {
            let mut p = json!({"clip": d.clip, "pack": d.pack, "name": d.name});
            if !d.kind.is_empty() {
                p["kind"] = json!(d.kind);
            }
            match app.session.execute("layout.save", p) {
                Ok(v) => {
                    app.ui.layouts.save = None;
                    if let Some(id) = v["pack"].as_str() {
                        app.ui.layouts.pack = id.into();
                    }
                    app.ui.status = tlf!("Saved layout to {path}", path = v["path"].as_str().unwrap_or_default());
                    invalidate(ctx);
                }
                Err(e) => {
                    d.error = e.to_string();
                    app.ui.layouts.save = Some(d);
                }
            }
        }
        Some(false) => app.ui.layouts.save = None,
        None => app.ui.layouts.save = Some(d),
    }
}
