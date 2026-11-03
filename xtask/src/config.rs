use std::path::Path;

use craft_xtask::{Cargo, Result, layers, version};

pub fn version(root: &Path) -> version::Config {
    version::Config { cargo: Cargo { root: root.into(), ..Cargo::default() }, temporary_dir: ".".into(), ..version::Config::default() }
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

pub fn layers(root: &Path) -> layers::Config {
    layers::Config {
        root: root.into(),
        layers: [
            ("L0", &["core", "texel", "geom"][..]),
            ("L1", &["compose", "bake"]),
            ("L2", &["paint", "formats"]),
            ("L3", &["engine", "render"]),
            ("L4", &["automation"]),
            ("L5", &["ui-egui"]),
        ]
        .into_iter()
        .map(|(label, members)| (label.into(), members.iter().map(|name| format!("texturecraft-{name}")).collect()))
        .collect(),
        exempt: names(&["texturecraft", "texturecraft-cli", "texturecraft-web", "xtask"]),
        external_rules: vec![
            layers::ExternalRule {
                dependencies: names(&["egui", "egui_*", "egui-*", "eframe", "winit", "rfd"]),
                allowed_from: names(&["texturecraft-ui-egui"]),
                allowed_dev_from: Vec::new(),
            },
            layers::ExternalRule {
                dependencies: names(&["wgpu", "wgpu-*"]),
                allowed_from: names(&["texturecraft-render", "texturecraft-ui-egui"]),
                allowed_dev_from: Vec::new(),
            },
        ],
        ..layers::Config::default()
    }
}

fn describe(policy: &layers::Config, name: &str) -> String {
    if policy.exempt.iter().any(|member| member == name) {
        "exempt".into()
    } else {
        policy
            .layers
            .iter()
            .find(|(_, members)| members.iter().any(|member| member == name))
            .map(|(label, _)| label.clone())
            .unwrap_or_else(|| "UNKNOWN".into())
    }
}

pub fn render_layers(policy: &layers::Config, report: &layers::Report) -> String {
    let mut out = String::from("Dependency layering (xtask/src/layers.rs)\n\n");
    for (_, members) in &policy.layers {
        for pkg in members {
            let dir = pkg.strip_prefix("texturecraft-").unwrap_or(pkg);
            if !report.crates.iter().any(|c| c.name == *pkg) {
                if policy.root.join("crates").join(dir).join("Cargo.toml").is_file() {
                    out += &format!("note: crates/{dir} has a Cargo.toml but is not a workspace member; not checked\n");
                } else {
                    out += &format!("skip: crates/{dir} (no Cargo.toml yet)\n");
                }
            }
        }
    }
    out += &format!("\n{:<28} {:<8} workspace deps\n", "crate", "layer");
    for c in &report.crates {
        let ws: Vec<&str> = c.deps.iter().filter(|d| d.workspace).map(|d| d.name.as_str()).collect();
        out += &format!("{:<28} {:<8} {}\n", c.name, describe(policy, &c.name), ws.join(", "));
    }
    out += "\n";
    if report.ok {
        out += &format!("OK: {} crates, no layering violations.\n", report.crates.len());
    } else {
        out += &format!("{} violation(s):\n", report.violations.len());
        for v in &report.violations {
            out += &format!("  - {v}\n");
        }
    }
    out
}

pub fn run_layers(policy: &layers::Config, args: &[String]) -> Result {
    let report = layers::compute(policy, &layers::Args::parse(args)?)?;
    print!("{}", render_layers(policy, &report));
    if report.ok { Ok(()) } else { Err(format!("{} layering violation(s)", report.violations.len())) }
}
