//! Essential Graphics ▸ Layouts (layout packs), the smart transition controls and the Remix
//! Layout Pack / Save As Layout dialogs, driven headless through the control channel (like
//! `graphics_templates_ui.rs`). With `FILMCRAFT_UI_SHOTS=<dir>` it also renders the UI with wgpu
//! and writes PNGs there for review.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    shots: Option<std::path::PathBuf>,
}

impl Driver {
    fn demo(data: &str) -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let dir = std::env::temp_dir().join(format!("filmcraft-layouts-ui-{data}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        session.prefs_path = Some(dir.join("prefs.json"));
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let shots = std::env::var_os("FILMCRAFT_UI_SHOTS").map(std::path::PathBuf::from);
        let mut b = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000);
        if shots.is_some() {
            b = b.wgpu().with_pixels_per_point(1.0);
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, shots };
        d.frames(4);
        d
    }
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }
    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params.clone());
        assert_eq!(r["ok"], true, "{method} {params}: {r}");
        r["result"].clone()
    }
    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }
    fn shot(&mut self, name: &str) {
        let Some(dir) = self.shots.clone() else { return };
        self.frames(8);
        let img = self.harness.render().expect("wgpu render");
        std::fs::create_dir_all(&dir).unwrap();
        img.save(dir.join(format!("{name}.png"))).unwrap();
    }
    fn ids(&mut self, prefix: &str) -> Vec<String> {
        let v = self.ok("ui.elements", json!({"prefix": prefix}));
        v.as_array().unwrap().iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()
    }
}

impl Driver {
    /// (clip id, name) of the clips on the track named "Layouts".
    fn layout_clips(&mut self) -> Vec<(u64, String)> {
        let q = self.exec("sequence.inspect", json!({}));
        let mut out = Vec::new();
        for t in q["video"].as_array().cloned().unwrap_or_default() {
            if t["name"] == "Layouts" {
                for c in t["items"].as_array().cloned().unwrap_or_default() {
                    out.push((c["clip"].as_u64().unwrap(), c["name"].as_str().unwrap_or("").to_string()));
                }
            }
        }
        out
    }
    fn open_layouts(&mut self) {
        self.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
        self.ok("ui.menu.invoke", json!({"id": "window.panel.EssentialGraphics"}));
        self.ok("ui.set", json!({"gfxTemplates": {"tab": "layouts"}}));
        self.frames(4);
    }
}

#[test]
fn browse_apply_restyle_and_edit_the_smart_transition() {
    let mut d = Driver::demo("browse");
    d.open_layouts();
    let items = d.ids("layouts.item.");
    assert_eq!(items.len(), 9, "the pack's layouts: {items:?}");
    assert!(items.contains(&"layouts.item.speaker-framed".to_string()));
    d.shot("layouts-tab");
    // the kind filter narrows the cards
    d.ok("ui.set", json!({"layouts": {"kind": "camera"}}));
    d.frames(3);
    assert_eq!(d.ids("layouts.item.").len(), 3);
    d.ok("ui.set", json!({"layouts": {"kind": ""}}));
    // lay out the first scene, then the second: select a card, Apply to Scene
    d.exec("playhead.set", json!({"seconds": 1}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "layouts.item.speaker-full"}));
    d.ok("ui.click", json!({"id": "layouts.apply"}));
    d.frames(3);
    d.exec("playhead.set", json!({"seconds": 6}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "layouts.item.speaker-framed"}));
    d.ok("ui.click", json!({"id": "layouts.apply"}));
    d.frames(3);
    let clips = d.layout_clips();
    assert_eq!(clips.iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), ["Speaker \u{2013} Full", "Speaker \u{2013} Framed"]);
    d.shot("layouts-applied");
    // the second clip is selected: its smart transition shows under the cards
    let ids = d.ids("layouts.transition.");
    assert!(ids.contains(&"layouts.transition.enabled".to_string()) && ids.contains(&"layouts.transition.fades".to_string()), "{ids:?}");
    let smart = |d: &mut Driver, clip: u64| d.exec("graphics.list", json!({"clip": clip}))["graphic"]["smart"].clone();
    assert_eq!(smart(&mut d, clips[1].0)["enabled"], true);
    d.ok("ui.click", json!({"id": "layouts.transition.enabled"}));
    d.frames(3);
    assert_eq!(smart(&mut d, clips[1].0)["enabled"], false);
    assert_eq!(smart(&mut d, clips[0].0)["enabled"], true, "only the selected clip changed");
    // another pack: Restyle Sequence gives both clips its look
    d.ok("ui.set", json!({"layouts": {"pack": "builtin:paper"}}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "layouts.restyle"}));
    d.frames(3);
    for (clip, _) in &clips {
        assert_eq!(d.exec("graphics.list", json!({"clip": clip}))["graphic"]["layout"]["pack"], "builtin:paper");
    }
    d.shot("layouts-restyled");
    // applying again replaces the layout of the scene under the playhead
    d.ok("ui.set", json!({"layouts": {"kind": "quote"}}));
    d.frames(3);
    assert_eq!(d.ids("layouts.item."), ["layouts.item.quote"]);
    d.ok("ui.click", json!({"id": "layouts.item.quote"}));
    d.ok("ui.click", json!({"id": "layouts.apply"}));
    d.frames(3);
    let names: Vec<String> = d.layout_clips().into_iter().map(|c| c.1).collect();
    assert_eq!(names, ["Speaker \u{2013} Full", "Pull Quote"]);
}

#[test]
fn remix_and_save_dialogs() {
    let mut d = Driver::demo("dialogs");
    d.open_layouts();
    // Remix… opens the dialog with the pack's theme
    d.ok("ui.click", json!({"id": "layouts.remix"}));
    d.frames(3);
    let ids = d.ids("remixPack.");
    for want in ["remixPack.name", "remixPack.color.accent", "remixPack.color.background", "remixPack.font.heading", "remixPack.ok"] {
        assert!(ids.contains(&want.to_string()), "{want} in {ids:?}");
    }
    let st = d.ok("ui.inspect", json!({}));
    assert_eq!(st["ui"]["layouts"]["remix"]["pack"], "builtin:studio");
    d.shot("layouts-remix-dialog");
    d.ok("ui.set", json!({"layouts": {"remix": {"name": "House Style", "colors": {"accent": "#c8102e"}}}}));
    d.frames(2);
    d.ok("ui.click", json!({"id": "remixPack.ok"}));
    d.frames(3);
    let packs = d.exec("layout.pack.list", json!({"source": "user"}));
    assert_eq!(packs["packs"][0]["id"], "user:house-style", "{packs}");
    assert_eq!(packs["packs"][0]["theme"]["colors"]["accent"], "#c8102e");
    assert_eq!(packs["packs"][0]["theme"]["colors"]["background"], "#0f1115", "the rest of the theme is kept");
    // the tab now shows the new pack
    let st = d.ok("ui.inspect", json!({}));
    assert_eq!(st["ui"]["layouts"]["pack"], "user:house-style");
    assert!(st["ui"]["layouts"]["remix"].is_null());
    // an empty name is an error shown in the dialog, which stays open
    d.exec("layout.pack.remix", json!({}));
    d.ok("ui.set", json!({"layouts": {"remix": {"name": " "}}}));
    d.frames(2);
    d.ok("ui.click", json!({"id": "remixPack.ok"}));
    d.frames(3);
    let st = d.ok("ui.inspect", json!({}));
    assert!(st["ui"]["layouts"]["remix"]["error"].as_str().is_some_and(|e| !e.is_empty()), "{}", st["ui"]["layouts"]["remix"]);
    d.ok("ui.click", json!({"id": "remixPack.cancel"}));
    d.frames(2);
    // Save As Layout…: a graphic clip goes into the pack
    d.exec("playhead.set", json!({"seconds": 300}));
    let r = d.exec("graphics.newText", json!({"text": "Lower third"}));
    let v = d.exec("layout.save", json!({}));
    assert_eq!(v["dialog"], "saveLayout");
    d.frames(3);
    let st = d.ok("ui.inspect", json!({}));
    assert_eq!(st["ui"]["layouts"]["save"]["clip"], r["clip"]);
    assert_eq!(st["ui"]["layouts"]["save"]["pack"], "House Style", "the user's pack is suggested");
    d.ok("ui.set", json!({"layouts": {"save": {"name": "My Card", "kind": "title"}}}));
    d.frames(2);
    d.ok("ui.click", json!({"id": "saveLayout.ok"}));
    d.frames(130);
    let packs = d.exec("layout.pack.list", json!({"source": "user"}));
    assert_eq!(packs["packs"][0]["layouts"].as_array().unwrap().len(), 10);
    assert!(d.ids("layouts.item.").contains(&"layouts.item.my-card".to_string()));
}
