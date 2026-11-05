//! Offscreen pictures of the whole window, for checking layout and design without a visible
//! screen (locked machines, CI, agents). Each is compared with a committed golden in
//! `tests/goldens/` (see the README there); a difference beyond `Tolerance::RENDER` fails the test
//! and writes the render and a x8 diff PNG to `target/golden-failures/`. Bless new or intended
//! changes with:
//!
//! ```sh
//! TC_BLESS=1 cargo test -p texturecraft-ui-egui --test ui_snapshots
//! ```
//!
//! To also write the full-size 1600x960 pictures (for looking at):
//!
//! ```sh
//! TC_UI_SNAPSHOTS=<dir> cargo test -p texturecraft-ui-egui --release --test ui_snapshots
//! ```
//!
//! It needs a GPU adapter and skips (with a message) without one, as CI has none.

#[path = "../../render/tests/common/golden.rs"]
mod golden;

use std::path::{Path, PathBuf};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use texturecraft_engine::core::{BakeResolution, BakedMap, CurvatureMethod, HeightNormalization};
use texturecraft_ui_egui::TextureCraftApp;
use texturecraft_ui_egui::docking::Panel;
use texturecraft_ui_egui::state::Mode;
use texturecraft_ui_egui::theme::Brightness;
use texturecraft_ui_egui::titlebar::TitleBar;

/// The picture with the status bar blanked. It shows wall-clock UI frame time ("UI 45.5 ms") and
/// job progress, which differ run to run, so goldens (and the settling check) leave it out; the
/// full-size pictures written to `TC_UI_SNAPSHOTS` keep it.
fn without_status_bar(h: &Harness<'_, TextureCraftApp>, img: &image::RgbaImage) -> image::RgbaImage {
    let strip = (22.0 * h.ctx.pixels_per_point()).ceil() as u32;
    let mut out = img.clone();
    for y in img.height().saturating_sub(strip)..img.height() {
        for x in 0..img.width() {
            out.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
        }
    }
    out
}

fn save(h: &mut Harness<'_, TextureCraftApp>, dir: Option<&Path>, name: &str) {
    // Compositing is paced by wall-clock budgets and tiles, GPU uploads and read-backs land over
    // several frames, so step with pauses until the picture stops changing (three equal frames).
    let (mut img, mut masked, mut same) = (None, None, 0);
    for _ in 0..400 {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(15));
        let now = h.render().unwrap_or_else(|e| panic!("{name}: {e}"));
        let now_masked = without_status_bar(h, &now);
        same = if masked.as_ref() == Some(&now_masked) { same + 1 } else { 0 };
        (img, masked) = (Some(now), Some(now_masked));
        if same >= 3 && h.state().session.pending_tiles() == 0 {
            break;
        }
    }
    let (img, masked) = (img.unwrap(), masked.unwrap());
    assert!(same >= 3, "{name}: the picture never settled");
    let (w, ht) = (img.width(), img.height());
    if let Some(dir) = dir {
        let path = dir.join(format!("{name}.png"));
        img.save(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        eprintln!("wrote {}", path.display());
    }
    // GPU output, so Tolerance::RENDER (the pictures are also box-downscaled to fit the size cap).
    golden::assert_golden(
        env!("CARGO_MANIFEST_DIR"),
        &format!("ui-{name}"),
        w,
        ht,
        masked.as_raw(),
        golden::Tolerance::RENDER,
        &format!("TextureCraft window snapshot: {name}"),
    );
}

#[test]
fn window_snapshots() {
    if texturecraft_render::headless::Headless::new().is_err() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    let output_dir = std::env::var_os("TC_UI_SNAPSHOTS").map(PathBuf::from);
    if let Some(dir) = &output_dir {
        std::fs::create_dir_all(dir).unwrap();
    }
    let output_dir = output_dir.as_deref();
    let session = texturecraft_engine::sample_session(1024, 256).unwrap();
    let mut harness =
        Harness::builder().with_size(egui::vec2(1600.0, 960.0)).with_pixels_per_point(1.0).wgpu().build_eframe(|cc| TextureCraftApp::new(cc, session));
    for (brightness, name) in [(Brightness::MediumDark, "main"), (Brightness::Dark, "brightness-dark"), (Brightness::Light, "brightness-light")] {
        harness.state_mut().ui.prefs.brightness = brightness;
        save(&mut harness, output_dir, name);
    }
    harness.state_mut().ui.prefs.brightness = Brightness::MediumDark;
    harness.state_mut().title_bar = TitleBar::Integrated;
    save(&mut harness, output_dir, "titlebar-macos");
    harness.state_mut().title_bar = TitleBar::App(craft_window::Layout::default());
    save(&mut harness, output_dir, "titlebar-app");

    // Check hover feedback and fractional scaling
    harness.hover_at(egui::pos2(1600.0 - 23.0, 14.0));
    save(&mut harness, output_dir, "titlebar-app-close-hover");
    harness.hover_at(egui::pos2(800.0, 500.0));
    harness.set_pixels_per_point(1.5);
    save(&mut harness, output_dir, "titlebar-app-150");
    harness.set_pixels_per_point(1.0);
    harness.state_mut().title_bar = TitleBar::System;

    for texture_set in &mut harness.state_mut().session.doc.project.texture_sets {
        texture_set.bake = Default::default();
    }
    harness.state_mut().ui.mode = Mode::Baking;
    save(&mut harness, output_dir, "mode-bake");
    harness.state_mut().ui.dock.focus(Panel::BakeMaps);
    harness.run_steps(3);
    harness.get_by_label("Ambient Occlusion").scroll_to_me();
    save(&mut harness, output_dir, "mode-bake-maps");
    harness.state_mut().ui.prefs.brightness = Brightness::Light;
    save(&mut harness, output_dir, "mode-bake-maps-light");
    harness.state_mut().ui.prefs.brightness = Brightness::MediumDark;
    harness.get_by_label("Opacity").scroll_to_me();
    save(&mut harness, output_dir, "mode-bake-unchecked");
    harness.state_mut().ui.dock.focus(Panel::TextureSetList);
    save(&mut harness, output_dir, "bake-texture-sets");
    harness.state_mut().ui.dock.focus(Panel::BakeSettings);
    save(&mut harness, output_dir, "bake-settings");
    harness.state_mut().session.add_high_poly_bytes("High Poly.obj", b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n".to_vec()).unwrap();
    save(&mut harness, output_dir, "bake-settings-high-poly");
    harness.get_by_label("Cage").scroll_to_me();
    save(&mut harness, output_dir, "bake-settings-cage-skew");
    harness.get_by_label("Edge Contrast").scroll_to_me();
    save(&mut harness, output_dir, "bake-settings-skew");
    harness.get_by_label("Output Size").scroll_to_me();
    harness.state_mut().session.doc.project.texture_sets[0].bake.common.resolution_height = Some(BakeResolution::R1024);
    save(&mut harness, output_dir, "bake-settings-unlocked");
    harness.set_pixels_per_point(1.5);
    save(&mut harness, output_dir, "bake-settings-150");
    harness.set_pixels_per_point(1.0);
    {
        let settings = &mut harness.state_mut().session.doc.project.texture_sets[0].bake;
        settings.maps.select([BakedMap::Curvature, BakedMap::Height]);
        settings.maps.curvature.parameters.method = CurvatureMethod::Mesh;
        settings.maps.height.parameters.normalization = HeightNormalization::Manual;
        settings.maps.height.parameters.scaling_divisor = 100.0;
    }
    harness.state_mut().ui.dock.focus(Panel::BakeMaps);
    harness.run_steps(3);
    harness.get_by_label("Curvature").scroll_to_me();
    save(&mut harness, output_dir, "bake-curvature-mesh");
    harness.get_by_label("Scaling Divisor").scroll_to_me();
    save(&mut harness, output_dir, "bake-height-manual");
    harness.state_mut().session.bake_meshes.high.clear();
    harness.state_mut().ui.mode = Mode::Painting;
    save(&mut harness, output_dir, "painting-restored");

    harness.state_mut().ui.dock.focus(Panel::Display);
    save(&mut harness, output_dir, "panel-display");
    harness.state_mut().ui.dock.close(Panel::Display);
    harness.state_mut().ui.windows.export = true;
    save(&mut harness, output_dir, "window-export");
    harness.state_mut().ui.windows.export = false;
    harness.state_mut().ui.windows.settings = true;
    save(&mut harness, output_dir, "window-settings");
    harness.state_mut().ui.windows.settings = false;

    // A taller window exposes every mesh-map slot
    harness.state_mut().ui.dock.focus(Panel::TextureSetSettings);
    harness.set_size(egui::vec2(1600.0, 1800.0));
    save(&mut harness, output_dir, "panel-texture-set-settings");
    harness.set_size(egui::vec2(1600.0, 960.0));

    // Show the mask histogram in Properties
    let session = &mut harness.state_mut().session;
    for (id, params) in [
        ("layer.add.fill", serde_json::json!({"color": [0.8, 0.3, 0.1]})),
        ("mask.add.black", serde_json::Value::Null),
        ("effect.add.fill", serde_json::json!({"target": "mask", "procedural": "clouds"})),
        ("effect.add.levels", serde_json::json!({"target": "mask", "inLow": 0.3, "inHigh": 0.8})),
    ] {
        texturecraft_engine::execute(session, id, &params).unwrap();
    }
    save(&mut harness, output_dir, "properties-levels");

    // Show the second path selected in Properties
    let session = &mut harness.state_mut().session;
    for (id, params) in [
        ("layer.add.paint", serde_json::Value::Null),
        ("tool.path", serde_json::json!({"uvPoints": [[0.2, 0.5], [0.5, 0.4], [0.8, 0.5]], "commit": true})),
        ("tool.path", serde_json::json!({"uvPoints": [[0.3, 0.7], [0.7, 0.7], [0.7, 0.9]], "closed": true, "fill": true, "commit": true})),
    ] {
        texturecraft_engine::execute(session, id, &params).unwrap();
    }

    let effect = match session.target {
        texturecraft_engine::StackTarget::Effect(e) => e.0,
        _ => 0,
    };
    texturecraft_engine::execute(session, "path.edit", &serde_json::json!({"effect": effect, "index": 1})).unwrap();
    save(&mut harness, output_dir, "properties-paths");

    // Show the brush cursor and readout during Ctrl+right drag
    let cursor_position = harness.state().viewport_rect.center();
    let (modifiers, button) = (egui::Modifiers::COMMAND, egui::PointerButton::Secondary);
    harness.event(egui::Event::ModifiersChanged(modifiers));
    harness.event(egui::Event::PointerButton { pos: cursor_position, button, pressed: true, modifiers });
    for step in 1..=4 {
        harness.hover_at(cursor_position + egui::vec2(15.0 * step as f32, -8.0 * step as f32));
        harness.run_steps(1);
    }
    save(&mut harness, output_dir, "brush-ctrl-drag");
    harness.event(egui::Event::PointerButton { pos: cursor_position, button, pressed: false, modifiers });
    harness.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
}
