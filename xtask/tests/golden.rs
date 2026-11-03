#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../src/config.rs"]
mod config;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use craft_xtask::{ico, layers, version};
use serde_json::Value;

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("texturecraft-70-{}-{}", std::process::id(), NEXT_TEMP.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn metadata() -> Value {
    serde_json::from_str(include_str!("golden/metadata.json")).unwrap()
}

fn cli(args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask")).args(args).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    output
}

fn hex_bytes(text: &str) -> Vec<u8> {
    text.trim().as_bytes().as_chunks::<2>().0.iter().map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect()
}

#[test]
fn layers_report_matches_original_tree_and_live_dispatch() {
    let policy = config::layers(&root());
    let crates = layers::from_metadata(&metadata()).unwrap();
    let report = layers::check_graph(&policy, &crates).unwrap();
    assert!(report.ok);
    let rendered = config::render_layers(&policy, &report);
    assert_eq!(rendered.as_bytes(), include_bytes!("golden/layers.stdout"));
    let temp = Temp::new();
    temp.write("metadata.json", include_bytes!("golden/metadata.json"));
    config::run_layers(&config::layers(&temp.0), &["--metadata".into(), "metadata.json".into()]).unwrap();
    assert!(config::run_layers(&policy, &["--bad".into()]).is_err());
    assert_eq!(cli(&["layers"]).stdout, include_bytes!("golden/layers.stdout"));
}

#[test]
fn layers_reject_upward_ui_gpu_and_unknown_edges() {
    let policy = config::layers(&root());
    let graph = layers::from_metadata(&metadata()).unwrap();
    for (source, target, workspace, kind) in [
        ("texturecraft-core", "texturecraft-geom", true, layers::DepKind::Normal),
        ("texturecraft-compose", "texturecraft-paint", true, layers::DepKind::Dev),
        ("texturecraft-paint", "texturecraft-formats", true, layers::DepKind::Build),
        ("texturecraft-engine", "texturecraft", true, layers::DepKind::Normal),
        ("texturecraft-automation", "egui", false, layers::DepKind::Normal),
        ("texturecraft-automation", "egui_extras", false, layers::DepKind::Normal),
        ("texturecraft-automation", "egui-wgpu", false, layers::DepKind::Normal),
        ("texturecraft-automation", "eframe", false, layers::DepKind::Normal),
        ("texturecraft-automation", "winit", false, layers::DepKind::Normal),
        ("texturecraft-automation", "rfd", false, layers::DepKind::Normal),
        ("texturecraft-engine", "wgpu", false, layers::DepKind::Normal),
        ("texturecraft-bake", "wgpu-types", false, layers::DepKind::Normal),
    ] {
        let mut mutated = graph.clone();
        mutated.iter_mut().find(|krate| krate.name == source).unwrap().deps.push(layers::Dep { name: target.into(), kind, workspace });
        assert_eq!(layers::check(&policy, &mutated).unwrap().len(), 1, "{source} -> {target}");
    }

    let mut allowed = graph.clone();
    for (source, target, workspace, kind) in [
        ("texturecraft-automation", "eguilike", false, layers::DepKind::Normal),
        ("texturecraft-render", "wgpu", false, layers::DepKind::Normal),
        ("xtask", "texturecraft-ui-egui", true, layers::DepKind::Normal),
        ("texturecraft-core", "texturecraft-core", true, layers::DepKind::Dev),
    ] {
        allowed.iter_mut().find(|krate| krate.name == source).unwrap().deps.push(layers::Dep { name: target.into(), kind, workspace });
    }
    assert!(layers::check(&policy, &allowed).unwrap().is_empty());

    let mut unknown = graph;
    unknown.push(layers::Crate { name: "texturecraft-mystery".into(), deps: vec![] });
    assert_eq!(layers::check(&policy, &unknown).unwrap().len(), 1);
}

#[test]
fn version_plan_dry_run_and_read_match_original() {
    let temp = Temp::new();
    temp.write("Cargo.toml", include_bytes!("golden/version-before.toml"));
    let policy = config::version(&temp.0);

    let show_plan = version::compute(&policy, &version::Args::Show).unwrap();
    assert_eq!(version::render(&show_plan).as_bytes(), include_bytes!("golden/version.stdout"));
    assert_eq!(cli(&["version"]).stdout, include_bytes!("golden/version.stdout"));

    let tag_plan = version::compute(&policy, &version::Args::Tag).unwrap();
    assert_eq!(version::render(&tag_plan), "v0.3.0\n");

    let set_plan = version::compute(&policy, &version::Args::Set("1.2.3-rc.1".into())).unwrap();
    assert_eq!(version::render(&set_plan).as_bytes(), include_bytes!("golden/version-set.stdout"));
    assert_eq!(set_plan.edits.len(), 1);
    assert_eq!(set_plan.edits[0].2.as_bytes(), include_bytes!("golden/version-after.toml"));
    assert_eq!(std::fs::read(temp.0.join("Cargo.toml")).unwrap(), include_bytes!("golden/version-before.toml"));
}

#[test]
fn version_apply_edits_manifest_and_lockfile_identically() {
    let temp = Temp::new();
    temp.write("Cargo.toml", include_bytes!("golden/version-before.toml"));
    temp.write("src/lib.rs", "");
    let policy = config::version(&temp.0);
    let plan = version::compute(&policy, &version::Args::Set("1.2.3-rc.1".into())).unwrap();
    version::apply(&plan).unwrap();
    assert_eq!(version::render(&plan).as_bytes(), include_bytes!("golden/version-set.stdout"));
    assert_eq!(std::fs::read(temp.0.join("Cargo.toml")).unwrap(), include_bytes!("golden/version-after.toml"));
    assert_eq!(std::fs::read(temp.0.join("Cargo.lock")).unwrap(), include_bytes!("golden/version-after.lock"));
}

#[test]
fn version_keeps_parser_and_validation_rules() {
    let policy = config::version(Path::new("."));
    assert!(policy.refresh_lockfile && policy.online_fallback);
    assert!(!policy.strict_prerelease && !policy.bounded_core && !policy.rollback_on_failure && !policy.update_path_pins);
    let crlf = "[workspace.package]\r\nversion = \"0.3.0\"\r\n";
    assert_eq!(version::rewrite(&policy, crlf, "2.0.0").unwrap(), "[workspace.package]\r\nversion = \"2.0.0\"\r\n");
    for ok in ["0.1.0", "1.20.300", "1.0.0-rc.1", "1.0.0-alpha", "1.0.0-x-y.2"] {
        assert!(version::validate(ok).is_ok(), "{ok}");
    }
    for bad in ["1.0", "1.0.0.0", "v1.0.0", "01.0.0", "1.0.0-", "1.0.0-a..b", "1.0.0+meta", "1.a.0", ""] {
        assert!(version::validate(bad).is_err(), "{bad}");
    }
}

#[test]
fn ico_and_icns_reports_bytes_and_stdout_match_original() {
    let temp = Temp::new();
    let pngs: Vec<String> = serde_json::from_str(include_str!("golden/pngs.json")).unwrap();
    let mut ico_args = vec!["out.ico".to_owned()];
    let mut icns_args = vec!["out.icns".to_owned()];
    for (index, png) in pngs.iter().enumerate() {
        let path = format!("{index}.png");
        temp.write(&path, hex_bytes(png));
        ico_args.push(path.clone());
        icns_args.push(path);
    }

    let ico_cfg = ico::Config { root: temp.0.clone(), ..ico::Config::default() };
    let ico_report = ico::compute(&ico_cfg, &ico::Args::parse(&ico_args).unwrap()).unwrap();
    assert!(!ico_report.destination.exists());
    assert_eq!(ico_report.bytes, hex_bytes(include_str!("golden/ico.hex")));
    assert_eq!(ico::render(&ico_report).as_bytes(), include_bytes!("golden/ico.stdout"));

    let ico_out = Command::new(env!("CARGO_BIN_EXE_xtask")).current_dir(&temp.0).arg("ico").args(&ico_args).output().unwrap();
    assert!(ico_out.status.success());
    assert_eq!(ico_out.stdout, include_bytes!("golden/ico.stdout"));
    assert_eq!(std::fs::read(temp.0.join("out.ico")).unwrap(), hex_bytes(include_str!("golden/ico.hex")));

    let icns_cfg = ico::Config { root: temp.0.clone(), icns: true, ..ico::Config::default() };
    let icns_report = ico::compute(&icns_cfg, &ico::Args::parse(&icns_args).unwrap()).unwrap();
    assert!(!icns_report.destination.exists());
    assert_eq!(icns_report.bytes, hex_bytes(include_str!("golden/icns.hex")));
    assert_eq!(ico::render(&icns_report).as_bytes(), include_bytes!("golden/icns.stdout"));

    let icns_out = Command::new(env!("CARGO_BIN_EXE_xtask")).current_dir(&temp.0).arg("icns").args(&icns_args).output().unwrap();
    assert!(icns_out.status.success());
    assert_eq!(icns_out.stdout, include_bytes!("golden/icns.stdout"));
    assert_eq!(std::fs::read(temp.0.join("out.icns")).unwrap(), hex_bytes(include_str!("golden/icns.hex")));
}

#[test]
fn ico_and_icns_reject_bad_input_and_verify_committed_icons() {
    let pngs: Vec<String> = serde_json::from_str(include_str!("golden/pngs.json")).unwrap();
    let sample = hex_bytes(&pngs[0]);
    assert!(ico::pack_ico(&[]).is_err());
    assert!(ico::pack_icns(&[]).is_err());
    assert!(ico::pack_ico(&[b"GIF89a".to_vec()]).is_err());
    for (size, ico_ok, icns_ok) in [(0u32, false, false), (48, true, false), (512, false, true)] {
        let mut png = sample.clone();
        png[16..20].copy_from_slice(&size.to_be_bytes());
        png[20..24].copy_from_slice(&size.to_be_bytes());
        assert_eq!(ico::pack_ico(&[png.clone()]).is_ok(), ico_ok, "ico {size}");
        assert_eq!(ico::pack_icns(&[png]).is_ok(), icns_ok, "icns {size}");
    }

    let dir = root().join("assets/app-icon");
    let ico_bytes = std::fs::read(dir.join("texturecraft.ico")).unwrap();
    assert_eq!(&ico_bytes[..4], &[0, 0, 1, 0]);
    assert!(u16::from_le_bytes([ico_bytes[4], ico_bytes[5]]) >= 4);
    let icns_bytes = std::fs::read(dir.join("texturecraft.icns")).unwrap();
    assert_eq!(&icns_bytes[..4], b"icns");
    assert_eq!(u32::from_be_bytes(icns_bytes[4..8].try_into().unwrap()) as usize, icns_bytes.len());
}

#[test]
fn help_text_is_preserved() {
    let output = cli(&["help"]);
    let added = output.stdout.strip_prefix(include_bytes!("golden/help.stdout")).unwrap();
    assert_eq!(added, format!("\nShared task syntax:\n  {}\n  {}\n  {}\n", version::cli_help(), layers::cli_help(), ico::cli_help()).as_bytes());
}
