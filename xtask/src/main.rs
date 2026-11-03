//! Workspace tooling: `cargo xtask <command>`.
//!
//! Pure Rust (craft-xtask + serde_json). `cargo` itself is invoked through `std::process::Command`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod config;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use craft_xtask::{Cargo, ico, layers, version};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  layers          enforce the crate dependency layering (xtask/src/layers.rs)
  wasm            cargo check --target wasm32-unknown-unknown for the wasm crates and the web app
  ci              fmt --check, clippy -D warnings, test, layers, wasm (stops at the first failure)
  version [set X.Y.Z[-pre]]
                  print the workspace version, or set it (Cargo.toml + Cargo.lock)
  ico <out.ico> <in.png>...
                  pack square PNGs (<= 256 px) into a Windows .ico (see packaging/icons.sh)
  icns <out.icns> <in.png>...
                  pack square PNGs (16-1024 px) into a macOS .icns (see packaging/icons.sh)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let task_args = args.get(1..).unwrap_or_default();
    let result = match args.first().map(String::as_str) {
        Some("layers") => config::run_layers(&config::layers(&root()), task_args),
        Some("wasm") => cmd_wasm(),
        Some("ci") => cmd_ci(),
        Some("version") => version::run(&config::version(&root()), task_args),
        Some("ico") => ico::run(&ico::Config::default(), task_args),
        Some("icns") => ico::run_icns(&ico::Config::default(), task_args),
        Some("-h" | "--help" | "help") | None => {
            print!("{USAGE}");
            println!("\nShared task syntax:");
            for help in [version::cli_help(), layers::cli_help(), ico::cli_help()] {
                println!("  {help}");
            }
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Workspace root (parent of the xtask crate).
pub fn root() -> PathBuf {
    let xtask = Path::new(env!("CARGO_MANIFEST_DIR"));
    xtask.parent().unwrap_or(xtask).to_path_buf()
}

pub fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

pub fn run(mut cmd: Command, what: &str) -> Result<(), String> {
    eprintln!("$ {what}");
    let status = cmd.status().map_err(|e| format!("{what}: failed to spawn: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{what}: exited with {status}")) }
}

/// Packages that must build for wasm32-unknown-unknown, in dependency order.
const WASM_PACKAGES: &[&str] = &[
    "texturecraft-core",
    "texturecraft-texel",
    "texturecraft-geom",
    "texturecraft-compose",
    "texturecraft-bake",
    "texturecraft-paint",
    "texturecraft-formats",
    "texturecraft-engine",
    "texturecraft-render",
    "texturecraft-ui-egui",
    "texturecraft-web",
];

fn cmd_wasm() -> Result<(), String> {
    let meta = Cargo { root: root(), ..Cargo::default() }.metadata(false)?;
    let members = layers::from_metadata(&meta)?;
    let mut results = Vec::new();
    for pkg in WASM_PACKAGES {
        if !members.iter().any(|c| c.name == *pkg) {
            println!("skip: {pkg} (not a workspace member yet)");
            continue;
        }
        // One package per invocation, so no crate passes only thanks to features another enables.
        let mut c = cargo();
        c.args(["check", "--target", "wasm32-unknown-unknown", "-p", pkg]);
        let ok = run(c, &format!("cargo check --target wasm32-unknown-unknown -p {pkg}")).is_ok();
        results.push((*pkg, ok));
    }
    println!("\nwasm32-unknown-unknown check:");
    for (p, ok) in &results {
        println!("  {:<6} {p}", if *ok { "ok" } else { "FAIL" });
    }
    let failed = results.iter().filter(|r| !r.1).count();
    if failed == 0 { Ok(()) } else { Err(format!("{failed} package(s) failed the wasm check")) }
}

fn cmd_ci() -> Result<(), String> {
    let cargo_step = |args: &'static [&'static str]| {
        move || {
            let mut c = cargo();
            c.args(args);
            run(c, &format!("cargo {}", args.join(" ")))
        }
    };
    type Step = (&'static str, Box<dyn Fn() -> Result<(), String>>);
    let steps: Vec<Step> = vec![
        ("fmt", Box::new(cargo_step(&["fmt", "--all", "--", "--check"]))),
        ("clippy", Box::new(cargo_step(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]))),
        ("test", Box::new(cargo_step(&["test", "--workspace"]))),
        ("layers", Box::new(|| config::run_layers(&config::layers(&root()), &[]))),
        ("wasm", Box::new(cmd_wasm)),
    ];
    for (i, (name, step)) in steps.iter().enumerate() {
        eprintln!("\n=== ci: {name} ===");
        if let Err(e) = step() {
            println!("\nCI summary:");
            for (done, _) in steps.get(..i).unwrap_or_default() {
                println!("  ok    {done}");
            }
            println!("  FAIL  {name}: {e}");
            for (skipped, _) in steps.iter().skip(i + 1) {
                println!("  skip  {skipped}");
            }
            return Err(format!("ci failed at `{name}`"));
        }
    }
    println!("\nCI summary: all {} steps passed", steps.len());
    Ok(())
}
