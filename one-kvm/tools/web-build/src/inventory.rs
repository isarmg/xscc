use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

pub type Files = BTreeMap<String, String>;
pub const MAX_FILE: u64 = 32 * 1024 * 1024;

pub fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn relative(path: &Path) -> Result<String> {
    ensure!(
        !path.as_os_str().is_empty() && !path.is_absolute(),
        "Expected a nonempty relative path"
    );
    ensure!(
        path.components().all(|p| matches!(p, Component::Normal(_))),
        "Uncontrolled relative path: {}",
        path.display()
    );
    Ok(path
        .to_str()
        .context("Path must be UTF-8")?
        .replace('\\', "/"))
}

pub fn physical(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        let info = fs::symlink_metadata(ancestor)
            .with_context(|| format!("Inspect {}", ancestor.display()))?;
        ensure!(
            !info.file_type().is_symlink(),
            "Symlink input/output is forbidden: {}",
            ancestor.display()
        );
    }
    Ok(())
}

pub fn read(path: &Path) -> Result<Vec<u8>> {
    physical(path)?;
    let info = fs::symlink_metadata(path)?;
    ensure!(
        info.is_file() && info.len() <= MAX_FILE,
        "Expected bounded physical file: {}",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            info.nlink() == 1,
            "Hard linked input/output is forbidden: {}",
            path.display()
        );
    }
    let bytes = fs::read(path)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE,
        "Input grew beyond byte bound"
    );
    Ok(bytes)
}

pub fn walk(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    if !root.exists() {
        return Ok(paths);
    }
    physical(root)?;
    let mut entries = fs::read_dir(root)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if matches!(
            name.to_str(),
            Some("target" | ".git" | "__pycache__" | ".pytest_cache")
        ) {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        ensure!(
            !kind.is_symlink(),
            "Symlink input/output is forbidden: {}",
            path.display()
        );
        if kind.is_dir() {
            paths.extend(walk(&path)?);
        } else if kind.is_file() {
            paths.push(path);
        } else {
            bail!("Special object is forbidden: {}", path.display());
        }
        ensure!(
            paths.len() <= 16384,
            "Input/output file count exceeds bound"
        );
    }
    Ok(paths)
}

pub fn files(root: &Path) -> Result<Files> {
    walk(root)?
        .into_iter()
        .map(|path| Ok((relative(path.strip_prefix(root)?)?, sha(&read(&path)?))))
        .collect()
}

pub fn validate_vendor(root: &Path) -> Result<()> {
    let provenance = root.join("SOURCE.json");
    let value: Value = serde_json::from_slice(&read(&provenance)?)?;
    let expected: Files = serde_json::from_value(
        value
            .get("files")
            .context("SOURCE.json is missing files")?
            .clone(),
    )?;
    ensure!(
        !expected.is_empty(),
        "Vendor source inventory cannot be empty"
    );
    for (name, digest) in &expected {
        relative(Path::new(name))?;
        ensure!(
            digest.len() == 64 && digest.bytes().all(|v| v.is_ascii_hexdigit()),
            "Invalid SOURCE digest"
        );
    }
    let mut actual = files(root)?;
    actual.remove("SOURCE.json");
    ensure!(
        actual == expected,
        "Vendored source differs from SOURCE.json: {}",
        root.display()
    );
    Ok(())
}

pub fn validate_vendors(root: &Path) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        ensure!(
            path.is_dir(),
            "Vendor root must contain package directories"
        );
        validate_vendor(&path)?;
    }
    Ok(())
}

pub fn builder_source(root: &Path, required: bool) -> Result<Option<Value>> {
    let provenance = root.join("SOURCE.json");
    if !provenance.exists() {
        ensure!(!required, "Embedded Web builder requires SOURCE.json");
        return Ok(None);
    }
    validate_vendor(root)?;
    let source: Value = serde_json::from_slice(&read(&provenance)?)?;
    let manifest: toml::Value =
        toml::from_str(&String::from_utf8(read(&root.join("Cargo.toml"))?)?)?;
    let package = manifest
        .get("package")
        .context("Builder package is missing")?;
    let version = package
        .get("version")
        .and_then(toml::Value::as_str)
        .context("Builder package version is missing")?;
    ensure!(
        source["format"].as_u64() == Some(1)
            && source["package"] == "xcss-web-build"
            && source["origin"] == "https://github.com/isarmg/xcss"
            && source["source_directory"] == "tools/web-build"
            && package.get("name").and_then(toml::Value::as_str) == Some("xcss-web-build")
            && source["version"].as_str() == Some(version)
            && source["base_revision"].as_str().is_some_and(|revision| {
                revision.len() == 40
                    && revision
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            }),
        "Web builder source identity differs from the committed snapshot contract"
    );
    Ok(Some(source))
}

fn cargo_sources(
    frontend: &Path,
    base: &Path,
    seen: &mut Vec<PathBuf>,
    records: &mut BTreeMap<String, Files>,
) -> Result<()> {
    physical(base)?;
    let base = base.canonicalize()?;
    ensure!(
        base.starts_with(frontend),
        "Local Rust dependency escapes web/xcss-web: {}",
        base.display()
    );
    if seen.contains(&base) {
        return Ok(());
    }
    seen.push(base.clone());
    let manifest: toml::Value =
        toml::from_str(&String::from_utf8(read(&base.join("Cargo.toml"))?)?)?;
    let mut input = Files::new();
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "LICENSE",
        "SOURCE.json",
    ] {
        let path = base.join(name);
        if path.exists() {
            input.insert(name.into(), sha(&read(&path)?));
        }
    }
    for path in walk(&base.join("src"))? {
        input.insert(relative(path.strip_prefix(&base)?)?, sha(&read(&path)?));
    }
    if base.join("SOURCE.json").exists() {
        validate_vendor(&base)?;
        input = files(&base)?;
    }
    records.insert(
        base.strip_prefix(frontend)?
            .to_string_lossy()
            .replace('\\', "/"),
        input,
    );
    fn paths(value: &toml::Value, found: &mut Vec<String>) {
        if let Some(table) = value.as_table() {
            for (key, child) in table {
                if key == "path" {
                    if let Some(path) = child.as_str() {
                        found.push(path.into());
                    }
                } else {
                    paths(child, found);
                }
            }
        }
    }
    let mut dependencies = Vec::new();
    for name in [
        "dependencies",
        "dev-dependencies",
        "build-dependencies",
        "patch",
        "target",
    ] {
        if let Some(value) = manifest.get(name) {
            paths(value, &mut dependencies);
        }
    }
    for dependency in dependencies {
        cargo_sources(frontend, &base.join(dependency), seen, records)?;
    }
    Ok(())
}

pub fn snapshot(root: &Path) -> Result<Value> {
    let web = root.join("web");
    let frontend = web.join("xcss-web");
    let mut rust = BTreeMap::new();
    cargo_sources(&frontend, &frontend, &mut Vec::new(), &mut rust)?;
    validate_vendors(&web.join("vendor"))?;
    let mut assets = Files::new();
    for entry in fs::read_dir(&web)? {
        let path = entry?.path();
        if path.is_file() {
            assets.insert(relative(path.strip_prefix(&web)?)?, sha(&read(&path)?));
        }
    }
    for name in ["src", "vendor", "public", "licenses"] {
        for path in walk(&web.join(name))? {
            assets.insert(relative(path.strip_prefix(&web)?)?, sha(&read(&path)?));
        }
    }
    let builder = root.join("tools/web-build");
    let builder_source = builder_source(
        &builder,
        root.join("xcss-web-build.json").is_file() && builder.join("Cargo.toml").is_file(),
    )?;
    let mut tool_inputs = Files::new();
    for name in ["Cargo.toml", "Cargo.lock", "SOURCE.json"] {
        let path = builder.join(name);
        if path.exists() {
            tool_inputs.insert(name.into(), sha(&read(&path)?));
        }
    }
    for path in walk(&builder.join("src"))? {
        tool_inputs.insert(relative(path.strip_prefix(&builder)?)?, sha(&read(&path)?));
    }
    if !tool_inputs.is_empty() {
        for (name, digest) in crate::compiled_inputs() {
            ensure!(
                tool_inputs
                    .get(name)
                    .is_some_and(|actual| actual == &digest),
                "Builder executable is stale or differs from vendored source ({name}); rebuild tools/web-build with --locked"
            );
        }
    }
    let lock: toml::Value =
        toml::from_str(&String::from_utf8(read(&frontend.join("Cargo.lock"))?)?)?;
    let packages = lock
        .get("package")
        .context("Cargo.lock has no package records")?
        .as_array()
        .context("Invalid Cargo lock packages")?;
    let cargo_packages = packages
        .iter()
        .map(|package| {
            let mut record = serde_json::Map::new();
            for name in ["name", "version", "source", "checksum"] {
                if let Some(value) = package.get(name) {
                    record.insert(
                        name.into(),
                        serde_json::to_value(value).expect("TOML scalar"),
                    );
                }
            }
            Value::Object(record)
        })
        .collect::<Vec<_>>();
    let browser_inputs = rust.remove("").unwrap_or_default();
    let shared_inputs = rust.get("vendor/xcss-web").cloned().unwrap_or_default();
    Ok(
        json!({"browser_inputs":browser_inputs,"shared_inputs":shared_inputs,"patched_inputs":rust,"asset_inputs":assets,
        "builder_sha256":sha(&serde_json::to_vec(&tool_inputs)?),"builder_inputs":tool_inputs,"builder_source":builder_source,"cargo_packages":cargo_packages}),
    )
}

/// Cheap watcher probe; the release build still hashes all inputs before and
/// after compilation/linking. Include inode/ctime on Unix to notice replacement
/// even when an editor restores the previous modification timestamp.
pub fn signature(root: &Path) -> Result<String> {
    let web = root.join("web");
    let mut result = String::new();
    let mut paths = Vec::new();
    for entry in fs::read_dir(&web)? {
        let path = entry?.path();
        if path.is_file() {
            paths.push(path);
        }
    }
    for name in ["src", "vendor", "public", "licenses", "xcss-web"] {
        paths.extend(walk(&web.join(name))?);
    }
    paths.extend(walk(&root.join("tools/web-build/src"))?);
    for path in paths {
        let info = fs::symlink_metadata(&path)?;
        result.push_str(&format!(
            "{}:{}:{:?}",
            path.display(),
            info.len(),
            info.modified()?
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            result.push_str(&format!(
                ":{}:{}:{}",
                info.ino(),
                info.ctime(),
                info.ctime_nsec()
            ));
        }
        result.push('\n');
    }
    Ok(sha(result.as_bytes()))
}

pub fn vendor(
    source: &Path,
    destination: &Path,
    origin: &str,
    revision: &str,
    includes: &[String],
) -> Result<()> {
    ensure!(
        origin.starts_with("https://") && !origin.contains(['\n', '\r', '#']),
        "Vendor origin must be canonical HTTPS"
    );
    ensure!(
        !revision.is_empty() && revision.len() <= 256 && !revision.contains(['\n', '\r']),
        "Immutable revision is required"
    );
    ensure!(
        !includes.is_empty(),
        "vendor requires explicit --include paths"
    );
    physical(source)?;
    let source = source.canonicalize()?;
    let parent = destination
        .parent()
        .context("Missing vendor destination parent")?;
    fs::create_dir_all(parent)?;
    physical(parent)?;
    ensure!(
        !destination.exists(),
        "Vendor destination already exists; review and remove it explicitly before updating"
    );
    let stage = tempfile::tempdir_in(parent)?;
    let mut selected = Vec::new();
    for name in includes {
        relative(Path::new(name))?;
        let path = source.join(name);
        physical(&path)?;
        if path.is_dir() {
            selected.extend(walk(&path)?);
        } else {
            selected.push(path);
        }
    }
    for path in selected {
        let name = path.strip_prefix(&source)?;
        let output = stage.path().join(name);
        fs::create_dir_all(output.parent().context("Missing output parent")?)?;
        fs::write(output, read(&path)?)?;
    }
    let inventory = files(stage.path())?;
    ensure!(
        inventory
            .keys()
            .any(|n| n.to_ascii_lowercase().contains("license") || n.ends_with("OFL.txt")),
        "Vendor must retain license texts"
    );
    fs::write(
        stage.path().join("SOURCE.json"),
        serde_json::to_vec_pretty(
            &json!({"format":1,"origin":origin,"revision":revision,"files":inventory}),
        )?,
    )?;
    validate_vendor(stage.path())?;
    fs::rename(stage.path(), destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builder_snapshot_requires_current_identity_and_exact_authored_bytes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        assert!(builder_source(dir.path(), true).is_err());
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='xcss-web-build'\nversion='1.0.0'\n",
        )?;
        fs::write(dir.path().join("LICENSE"), "Fixture license")?;
        let source = json!({
            "format":1,"package":"xcss-web-build","version":"1.0.0",
            "origin":"https://github.com/isarmg/xcss","source_directory":"tools/web-build",
            "base_revision":"a".repeat(40),"files":files(dir.path())?
        });
        let write_source = |source: &Value| -> Result<()> {
            fs::write(dir.path().join("SOURCE.json"), serde_json::to_vec(source)?)?;
            Ok(())
        };
        write_source(&source)?;
        assert_eq!(builder_source(dir.path(), true)?, Some(source.clone()));
        let mut wrong_identity = source.clone();
        wrong_identity["origin"] = json!("https://example.invalid/edited-fork");
        write_source(&wrong_identity)?;
        assert!(builder_source(dir.path(), true).is_err());
        write_source(&source)?;
        fs::write(dir.path().join("LICENSE"), "Changed license")?;
        assert!(builder_source(dir.path(), true).is_err());
        Ok(())
    }

    #[test]
    fn vendor_inventory_rejects_added_and_modified_inputs() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("LICENSE"), "license")?;
        fs::write(dir.path().join("module.js"), "export const x=1;")?;
        fs::write(
            dir.path().join("SOURCE.json"),
            serde_json::to_vec(&json!({"files":files(dir.path())?}))?,
        )?;
        validate_vendor(dir.path())?;
        fs::write(dir.path().join("module.js"), "export const x=2;")?;
        assert!(validate_vendor(dir.path()).is_err());
        Ok(())
    }
    #[test]
    fn paths_cannot_escape() {
        for name in ["", "/etc/passwd", "../file", "a/../file"] {
            assert!(relative(Path::new(name)).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn links_are_not_build_inputs() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("real"), "x")?;
        std::os::unix::fs::symlink("real", dir.path().join("link"))?;
        assert!(files(dir.path()).is_err());
        Ok(())
    }

    #[test]
    fn refreshing_a_vendor_manifest_does_not_hide_build_time_source_drift() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let frontend = dir.path().join("web/xcss-web");
        fs::create_dir_all(frontend.join("src"))?;
        fs::write(
            frontend.join("Cargo.toml"),
            "[package]\nname='test-web'\nversion='0.1.0'\nedition='2024'\n",
        )?;
        fs::write(
            frontend.join("Cargo.lock"),
            "version=4\n[[package]]\nname='test-web'\nversion='0.1.0'\n",
        )?;
        fs::write(frontend.join("src/lib.rs"), "pub fn test() {}")?;
        let vendor = dir.path().join("web/vendor/example");
        fs::create_dir_all(&vendor)?;
        fs::write(vendor.join("LICENSE"), "license")?;
        fs::write(vendor.join("module.js"), "export const x=1;")?;
        fs::write(
            vendor.join("SOURCE.json"),
            serde_json::to_vec(&json!({"files":files(&vendor)?}))?,
        )?;
        let before = snapshot(dir.path())?;
        fs::write(vendor.join("module.js"), "export const x=2;")?;
        let mut refreshed = files(&vendor)?;
        refreshed.remove("SOURCE.json");
        fs::write(
            vendor.join("SOURCE.json"),
            serde_json::to_vec(&json!({"files":refreshed}))?,
        )?;
        validate_vendor(&vendor)?;
        assert_ne!(snapshot(dir.path())?, before);
        Ok(())
    }

    #[test]
    fn refreshing_public_media_inventory_does_not_hide_build_time_source_drift() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let web = dir.path().join("web");
        let frontend = web.join("xcss-web");
        fs::create_dir_all(frontend.join("src"))?;
        fs::write(
            frontend.join("Cargo.toml"),
            "[package]\nname='test-web'\nversion='0.1.0'\nedition='2024'\n",
        )?;
        fs::write(
            frontend.join("Cargo.lock"),
            "version=4\n[[package]]\nname='test-web'\nversion='0.1.0'\n",
        )?;
        fs::write(frontend.join("src/lib.rs"), "pub fn test() {}")?;
        fs::create_dir(web.join("public"))?;
        fs::write(web.join("public/image.svg"), "<svg/>")?;
        fs::write(
            web.join("rust-web.toml"),
            "[web]\npublic_assets_manifest='public-assets.json'\n",
        )?;
        let write_manifest = || -> Result<()> {
            fs::write(
                web.join("public-assets.json"),
                serde_json::to_vec(&json!({"format":1,"files":files(&web.join("public"))?}))?,
            )?;
            Ok(())
        };
        write_manifest()?;
        let before = snapshot(dir.path())?;
        fs::write(web.join("public/image.svg"), "<svg>changed</svg>")?;
        write_manifest()?;
        let after = snapshot(dir.path())?;
        assert_ne!(
            after["asset_inputs"]["public/image.svg"],
            before["asset_inputs"]["public/image.svg"]
        );
        assert_ne!(
            after["asset_inputs"]["public-assets.json"],
            before["asset_inputs"]["public-assets.json"]
        );
        assert_ne!(after, before);
        Ok(())
    }
}
