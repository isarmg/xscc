pub mod assets;
pub mod dev;
pub mod graph;
pub mod inventory;
pub mod javascript;
pub mod server_build;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

pub const RUST: &str = "1.99.0";
pub const BINDGEN: &str = "0.2.129";
pub const TARGET: &str = "wasm32-unknown-unknown";

/// Bind provenance to the implementation actually compiled into this binary.
pub fn compiled_inputs() -> Vec<(&'static str, String)> {
    [
        ("Cargo.toml", include_bytes!("../Cargo.toml").as_slice()),
        ("Cargo.lock", include_bytes!("../Cargo.lock").as_slice()),
        ("src/lib.rs", include_bytes!("lib.rs").as_slice()),
        ("src/main.rs", include_bytes!("main.rs").as_slice()),
        ("src/assets.rs", include_bytes!("assets.rs").as_slice()),
        (
            "src/inventory.rs",
            include_bytes!("inventory.rs").as_slice(),
        ),
        (
            "src/javascript.rs",
            include_bytes!("javascript.rs").as_slice(),
        ),
        ("src/dev.rs", include_bytes!("dev.rs").as_slice()),
        (
            "src/server_build.rs",
            include_bytes!("server_build.rs").as_slice(),
        ),
        ("src/graph.rs", include_bytes!("graph.rs").as_slice()),
    ]
    .into_iter()
    .map(|(name, bytes)| (name, inventory::sha(bytes)))
    .collect()
}

#[derive(Clone, Debug)]
pub struct Options {
    pub root: PathBuf,
    pub offline: bool,
    pub debug: bool,
    pub dist: Option<PathBuf>,
}

pub fn distribution(options: &Options) -> Result<PathBuf> {
    let output = options.dist.clone().unwrap_or(PathBuf::from(
        assets::Config::read(&options.root.join("web"))?.web.dist,
    ));
    let output = if output.is_absolute() {
        output
    } else {
        options.root.join(output)
    };
    let parent = output
        .parent()
        .context("Distribution needs a parent directory")?;
    fs::create_dir_all(parent)?;
    inventory::physical(parent)?;
    let output = parent
        .canonicalize()?
        .join(output.file_name().context("Distribution directory name")?);
    ensure!(
        !options.root.starts_with(&output)
            && output != options.root.join("web")
            && !output.starts_with(options.root.join("web/src"))
            && !output.starts_with(options.root.join("web/vendor"))
            && !output.starts_with(options.root.join("web/xcss-web"))
            && output != options.root.join("web/pkg"),
        "Distribution must be a generated directory separate from authored source and source ancestors"
    );
    Ok(output)
}

pub fn cargo_action(options: &Options, action: &str) -> Result<()> {
    graph::resolve(options)?;
    let mut command = Command::new("cargo");
    command
        .arg(format!("+{RUST}"))
        .arg(action)
        .args(["--manifest-path"])
        .arg(options.root.join("web/xcss-web/Cargo.toml"))
        .arg("--locked")
        .current_dir(&options.root);
    if action != "test" {
        command.args(["--target", TARGET]);
    }
    if action == "build" && !options.debug {
        command.arg("--release");
    }
    if options.offline {
        command.arg("--offline");
    }
    command.env("CARGO_TARGET_DIR", target_directory(options)?);
    if env::var_os("CARGO_BUILD_JOBS").is_none() {
        command.env("CARGO_BUILD_JOBS", "2");
    }
    if env::var_os("CARGO_PROFILE_DEV_DEBUG").is_none() {
        command.env("CARGO_PROFILE_DEV_DEBUG", "0");
    }
    if env::var_os("CARGO_PROFILE_TEST_DEBUG").is_none() {
        command.env("CARGO_PROFILE_TEST_DEBUG", "0");
    }
    ensure!(
        command
            .status()
            .context("Run Cargo browser build")?
            .success(),
        "Browser cargo {action} failed"
    );
    Ok(())
}

fn target_directory(options: &Options) -> Result<PathBuf> {
    let path = env::var_os("XCSS_WASM_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| options.root.join("target/xcss-web"));
    Ok(if path.is_absolute() {
        path
    } else {
        options.root.join(path)
    })
}

fn validate_generated(directory: &Path) -> Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    inventory::physical(directory)?;
    ensure!(
        directory.is_dir(),
        "Generated destination must be a physical directory"
    );
    inventory::files(directory)?;
    ensure!(
        fs::read_dir(directory)?.next().is_none()
            || directory.join("xcss-web-build.json").is_file()
            || directory.join("assets/xcss-web-build.json").is_file(),
        "Refusing to replace a nonempty directory without xcss-web generated-output provenance: {}",
        directory.display()
    );
    Ok(())
}

fn replace_pair(
    pkg: &Path,
    stage_pkg: &Path,
    dist: &Path,
    stage_dist: &Path,
    backup: &Path,
) -> Result<()> {
    validate_generated(pkg)?;
    validate_generated(dist)?;
    let old_pkg = backup.join("previous-pkg");
    let old_dist = backup.join("previous-dist");
    let had_pkg = pkg.exists();
    let had_dist = dist.exists();
    if had_pkg {
        fs::rename(pkg, &old_pkg)?;
    }
    if had_dist && let Err(error) = fs::rename(dist, &old_dist) {
        if had_pkg {
            fs::rename(&old_pkg, pkg)?;
        }
        return Err(error.into());
    }
    if let Err(error) = fs::rename(stage_pkg, pkg) {
        if had_pkg {
            fs::rename(&old_pkg, pkg)?;
        }
        if had_dist {
            fs::rename(&old_dist, dist)?;
        }
        return Err(error.into());
    }
    if let Err(error) = fs::rename(stage_dist, dist) {
        fs::rename(pkg, stage_pkg)?;
        if had_pkg {
            fs::rename(&old_pkg, pkg)?;
        }
        if had_dist {
            fs::rename(&old_dist, dist)?;
        }
        return Err(error.into());
    }
    Ok(())
}

pub fn build(options: &Options) -> Result<Value> {
    inventory::physical(&options.root)?;
    let web = options.root.join("web");
    let manifest = web.join("xcss-web/Cargo.toml");
    let declaration: toml::Value =
        toml::from_str(&String::from_utf8(inventory::read(&manifest)?)?)?;
    let package = declaration["package"]["name"]
        .as_str()
        .context("Browser package name")?
        .replace('-', "_");
    let executable = env::var_os("WASM_BINDGEN").unwrap_or_else(|| "wasm-bindgen".into());
    let version = Command::new(&executable)
        .arg("--version")
        .output()
        .context("Find wasm-bindgen; install the pinned CLI or set WASM_BINDGEN")?;
    ensure!(
        version.status.success()
            && String::from_utf8_lossy(&version.stdout).trim() == format!("wasm-bindgen {BINDGEN}"),
        "Expected wasm-bindgen {BINDGEN}"
    );
    let config = assets::Config::read(&web)?;
    let runtime_root = if config.web.runtime_root == "product" {
        options.root.clone()
    } else {
        web.clone()
    };
    let dist = distribution(options)?;
    validate_generated(&web.join("pkg"))?;
    validate_generated(&dist)?;
    let inventory_path = env::var_os("XSSI_FRONTEND_METADATA_FILE").map(PathBuf::from);
    let tools = env::var_os("XSSI_BUILD_TOOLS_DIR").map(PathBuf::from);
    ensure!(
        inventory_path.is_some() == tools.is_some(),
        "Explicit runtime inventory requires both XSSI_BUILD_TOOLS_DIR and XSSI_FRONTEND_METADATA_FILE"
    );
    if let (Some(path), Some(tools)) = (&inventory_path, &tools) {
        ensure!(
            path.is_absolute() && tools.is_absolute() && !path.exists(),
            "Runtime inventory needs absolute paths and a new output file"
        );
        inventory::physical(path.parent().context("Runtime inventory parent")?)?;
        ensure!(
            dist.starts_with(&runtime_root),
            "XSSI runtime receipt requires distribution inside its configured source root"
        );
    }
    let csr_graph = graph::resolve(options)?;
    let before = inventory::snapshot(&options.root)?;
    let mut rust_runtime = graph::runtime(options, &before)?;
    cargo_action(options, "build")?;
    ensure!(
        inventory::snapshot(&options.root)? == before,
        "Source changed during browser compilation; previous outputs were preserved"
    );
    let stage = tempfile::Builder::new()
        .prefix(".rust-web-stage-")
        .tempdir_in(&web)?;
    let stage_pkg = stage.path().join("pkg");
    let stage_dist = stage.path().join("dist");
    fs::create_dir(&stage_pkg)?;
    fs::create_dir(&stage_dist)?;
    let profile = if options.debug { "debug" } else { "release" };
    let binary = target_directory(options)?
        .join(TARGET)
        .join(profile)
        .join(format!("{package}.wasm"));
    let status = Command::new(executable)
        .args(["--target", "web", "--out-dir"])
        .arg(&stage_pkg)
        .args(["--out-name", &package, "--no-typescript"])
        .arg(binary)
        .current_dir(&options.root)
        .status()?;
    ensure!(
        status.success(),
        "wasm-bindgen failed; previous outputs were preserved"
    );
    let generated = inventory::files(&stage_pkg)?;
    let mut metadata = before.clone();
    let object = metadata
        .as_object_mut()
        .context("Source inventory object")?;
    object.extend(json!({"format":1,"rust":RUST,"wasm_bindgen":BINDGEN,"package":package,"profile":profile,"files":generated,"asset_builder":"xcss-web-build/1.0.0","csr_graph":csr_graph,"rust_runtime":rust_runtime.clone()}).as_object().expect("Constant object").clone());
    let mut provenance = serde_json::to_vec(&metadata)?;
    provenance.push(b'\n');
    fs::write(stage_pkg.join("xcss-web-build.json"), &provenance)?;
    let (outputs, mut runtime) =
        assets::Linker::new(&web, &stage_pkg, config).finish(&provenance)?;
    rust_runtime["outputs"] = json!(
        outputs
            .iter()
            .filter(|(name, _)| name.ends_with(".wasm"))
            .map(|(name, bytes)| json!({"path":name,"sha256":inventory::sha(bytes)}))
            .collect::<Vec<_>>()
    );
    runtime["rust"] = rust_runtime;
    let final_files = assets::write_outputs(&stage_dist, &outputs)?;
    ensure!(
        inventory::snapshot(&options.root)? == before,
        "Source changed during asset linking; previous outputs were preserved"
    );
    runtime["distribution"] = json!(if let Ok(relative) = dist.strip_prefix(&runtime_root) {
        inventory::relative(relative)?
    } else {
        dist.to_string_lossy().into_owned()
    });
    let mut runtime_bytes = serde_json::to_vec_pretty(&runtime)?;
    runtime_bytes.push(b'\n');
    ensure!(
        runtime_bytes.len() <= 32 * 1024 * 1024,
        "Runtime inventory exceeds byte bound"
    );
    // Metadata is staged with the output and published as a create-new file only
    // after every source/hash/license/budget gate has passed.
    replace_pair(
        &web.join("pkg"),
        &stage_pkg,
        &dist,
        &stage_dist,
        stage.path(),
    )?;
    if let Some(path) = inventory_path {
        use std::io::Write;
        let mut open = fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        let mut file = open.open(path)?;
        file.write_all(&runtime_bytes)?;
        file.sync_all()?;
    }
    let summary = json!({"distribution":dist,"files":final_files.len(),"bytes":outputs.values().map(Vec::len).sum::<usize>(),"runtime":runtime});
    println!(
        "Built {} ({} files, {} bytes)",
        dist.display(),
        summary["files"],
        summary["bytes"]
    );
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_publish_preserves_previous_pair() -> Result<()> {
        let dir = tempfile::tempdir()?;
        for name in ["pkg", "dist", "stage-pkg", "backups"] {
            fs::create_dir(dir.path().join(name))?;
        }
        fs::write(dir.path().join("pkg/old"), "old package")?;
        fs::write(dir.path().join("dist/old"), "old dist")?;
        fs::write(dir.path().join("pkg/xcss-web-build.json"), "{}")?;
        fs::write(dir.path().join("dist/xcss-web-build.json"), "{}")?;
        let result = replace_pair(
            &dir.path().join("pkg"),
            &dir.path().join("stage-pkg"),
            &dir.path().join("dist"),
            &dir.path().join("missing-dist"),
            &dir.path().join("backups"),
        );
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("pkg/old"))?,
            "old package"
        );
        assert_eq!(fs::read_to_string(dir.path().join("dist/old"))?, "old dist");
        Ok(())
    }
}
