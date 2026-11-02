//! Verify integrated sources and Cargo's conservative resolution graph.
//! Metadata may include features from conditional edges that the current host
//! compilation does not activate. This policy guard is not a compiled-unit
//! inventory; host proc-macros are never counted as browser WASM code.
use crate::{Options, inventory};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::Command,
};

/// Record Cargo's selected target runtime tree, excluding host macros and all
/// build/dev edges. Portable identities can be independently rechecked by the
/// release producer in a private source export.
pub fn runtime(options: &Options, inputs: &Value) -> Result<Value> {
    let frontend = options.root.join("web/xcss-web");
    let manifest = frontend.join("Cargo.toml");
    let run = |arguments: &[&str]| -> Result<Vec<u8>> {
        let mut command = Command::new("cargo");
        command
            .arg(format!("+{}", crate::RUST))
            .args(arguments)
            .arg("--manifest-path")
            .arg(&manifest)
            .arg("--locked")
            .current_dir(&options.root);
        if options.offline {
            command.arg("--offline");
        }
        let output = command
            .output()
            .context("Query browser runtime Cargo graph")?;
        ensure!(
            output.status.success(),
            "Browser runtime graph failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            output.stdout.len() <= 32 * 1024 * 1024,
            "Browser runtime graph exceeds byte bound"
        );
        Ok(output.stdout)
    };
    let metadata: Value = serde_json::from_slice(&run(&[
        "metadata",
        "--format-version",
        "1",
        "--filter-platform",
        crate::TARGET,
    ])?)?;
    let tree = String::from_utf8(run(&[
        "tree",
        "--target",
        crate::TARGET,
        "--edges",
        "normal,no-proc-macro",
        "--no-dedupe",
        "--prefix",
        "depth",
        "--format",
        "\t{p}",
        "--color",
        "never",
    ])?)?;
    let graph = runtime_selection(&metadata, &tree, &frontend)?;
    let mut source_inputs = BTreeMap::<String, String>::new();
    let mut append = |prefix: &str, values: &Value| -> Result<()> {
        for (name, digest) in values.as_object().context("Rust source input inventory")? {
            let name = format!("web/xcss-web/{prefix}{name}");
            inventory::relative(Path::new(&name))?;
            source_inputs.insert(
                name,
                digest.as_str().context("Rust source input hash")?.into(),
            );
        }
        Ok(())
    };
    append("", &inputs["browser_inputs"])?;
    for (directory, files) in inputs["patched_inputs"]
        .as_object()
        .context("Local Rust dependencies")?
    {
        append(&format!("{directory}/"), files)?;
    }
    Ok(
        json!({"target":crate::TARGET,"mode":"csr","package":metadata["packages"].as_array().context("Cargo packages")?.iter().find(|p| p["id"]==metadata["resolve"]["root"]).context("Browser Cargo root")?["name"],
        "manifest":"web/xcss-web/Cargo.toml","default_features":true,"features":[],"profile":if options.debug {"debug"} else {"release"},"toolchain":{"rust":crate::RUST,"wasm_bindgen":crate::BINDGEN},
        "lock_sha256":inventory::sha(&inventory::read(&frontend.join("Cargo.lock"))?),"source_inputs":source_inputs,"graph":graph}),
    )
}

fn runtime_selection(metadata: &Value, tree: &str, frontend: &Path) -> Result<Value> {
    let packages = metadata["packages"]
        .as_array()
        .context("Cargo metadata packages")?;
    ensure!(
        packages.len() <= 4096,
        "Cargo runtime package count exceeds bound"
    );
    let mut selected = BTreeMap::<String, (&Value, BTreeSet<String>)>::new();
    let mut stack = Vec::<String>::new();
    let mut root = None;
    for (number, line) in tree.lines().enumerate() {
        ensure!(number < 200000, "Cargo runtime tree exceeds line bound");
        let (depth, display) = line.split_once('\t').context("Cargo runtime tree depth")?;
        let depth: usize = depth.parse()?;
        ensure!(
            depth <= 256 && depth <= stack.len() && (number == 0 || depth > 0),
            "Cargo runtime tree depth changed"
        );
        let matches = packages
            .iter()
            .filter(|p| {
                let prefix = format!(
                    "{} v{}",
                    p["name"].as_str().unwrap_or(""),
                    p["version"].as_str().unwrap_or("")
                );
                if p["source"].is_null() {
                    Path::new(p["manifest_path"].as_str().unwrap_or(""))
                        .parent()
                        .is_some_and(|path| display == format!("{prefix} ({})", path.display()))
                } else if p["source"] == "registry+https://github.com/rust-lang/crates.io-index" {
                    display == prefix
                } else if let Some(source) =
                    p["source"].as_str().and_then(|s| s.strip_prefix("git+"))
                {
                    source.rsplit_once('#').is_some_and(|(url, revision)| {
                        display
                            .strip_prefix(&format!("{prefix} ({url}#"))
                            .and_then(|s| s.strip_suffix(')'))
                            .is_some_and(|short| short.len() >= 7 && revision.starts_with(short))
                    })
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();
        ensure!(
            matches.len() == 1,
            "Cargo runtime tree source is ambiguous: {display}"
        );
        let package = matches[0];
        ensure!(
            !package["targets"]
                .as_array()
                .context("Cargo package targets")?
                .iter()
                .any(|t| t["kind"]
                    .as_array()
                    .is_some_and(|k| k.iter().any(|k| k == "proc-macro"))),
            "Host macro was selected as browser runtime"
        );
        let origin = if let Some(source) = package["source"].as_str() {
            source.to_owned()
        } else {
            let path = Path::new(
                package["manifest_path"]
                    .as_str()
                    .context("Local Cargo manifest")?,
            );
            format!(
                "path:{}",
                inventory::relative(path.strip_prefix(frontend)?)?
            )
        };
        let id = format!(
            "{}@{} {origin}",
            package["name"].as_str().context("Cargo name")?,
            package["version"].as_str().context("Cargo version")?
        );
        if number == 0 {
            ensure!(
                package["id"] == metadata["resolve"]["root"],
                "Cargo runtime tree has another root"
            );
            root = Some(id.clone());
        }
        if depth > 0 {
            let parent = &stack[depth - 1];
            ensure!(
                !stack[..depth].contains(&id),
                "Cargo runtime dependency cycle"
            );
            let parent_package = selected[parent].0;
            let node = metadata["resolve"]["nodes"]
                .as_array()
                .context("Cargo nodes")?
                .iter()
                .find(|n| n["id"] == parent_package["id"])
                .context("Cargo runtime parent")?;
            ensure!(
                node["deps"]
                    .as_array()
                    .context("Cargo dependency edges")?
                    .iter()
                    .any(|d| {
                        d["pkg"] == package["id"]
                            && d["dep_kinds"].as_array().is_some_and(|k| {
                                k.is_empty() || k.iter().any(|k| k["kind"].is_null())
                            })
                    }),
                "Cargo runtime tree edge differs from normal metadata"
            );
            selected
                .get_mut(parent)
                .context("Cargo runtime parent missing")?
                .1
                .insert(id.clone());
        }
        selected
            .entry(id.clone())
            .or_insert((package, BTreeSet::new()));
        stack.truncate(depth);
        stack.push(id);
    }
    let lock: toml::Value = toml::from_str(&String::from_utf8(inventory::read(
        &frontend.join("Cargo.lock"),
    )?)?)?;
    let records = selected.into_iter().map(|(id, (p, dependencies))| -> Result<Value> {
        let manifest = if p["source"].is_null() { Some(inventory::relative(Path::new(p["manifest_path"].as_str().context("Cargo manifest")?).strip_prefix(frontend)?)?) } else { None };
        let checksum = lock["package"].as_array().context("Cargo lock packages")?.iter().find(|v| v["name"].as_str()==p["name"].as_str() && v["version"].as_str()==p["version"].as_str() && v.get("source").and_then(toml::Value::as_str)==p["source"].as_str()).and_then(|p| p.get("checksum")).and_then(toml::Value::as_str);
        Ok(json!({"id":id,"name":p["name"],"version":p["version"],"source":p["source"],"manifest":manifest,"checksum":checksum,"dependencies":dependencies}))
    }).collect::<Result<Vec<_>>>()?;
    Ok(json!({"root":root.context("Cargo runtime tree is empty")?,"packages":records}))
}

pub fn resolve(options: &Options) -> Result<Value> {
    let frontend = options.root.join("web/xcss-web");
    let mut command = Command::new("cargo");
    command
        .arg(format!("+{}", crate::RUST))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--filter-platform",
            crate::TARGET,
            "--manifest-path",
        ])
        .arg(frontend.join("Cargo.toml"))
        .current_dir(&options.root);
    if options.offline {
        command.arg("--offline");
    }
    let output = command
        .output()
        .context("Inspect selected CSR dependency graph")?;
    ensure!(
        output.status.success(),
        "Cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        output.stdout.len() <= 32 * 1024 * 1024,
        "Cargo metadata exceeds byte bound"
    );
    validate(&serde_json::from_slice(&output.stdout)?, &frontend)
}

pub fn validate(metadata: &Value, frontend: &Path) -> Result<Value> {
    let packages = metadata["packages"]
        .as_array()
        .context("Cargo metadata packages")?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .context("Cargo metadata resolve nodes")?;
    let by_id = packages
        .iter()
        .map(|p| Ok((p["id"].as_str().context("Cargo package id")?, p)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let root = metadata["resolve"]["root"]
        .as_str()
        .context("Standalone browser crate must have a Cargo resolve root")?;
    let node_ids = nodes
        .iter()
        .map(|p| Ok((p["id"].as_str().context("Cargo node id")?, p)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut reachable = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if !reachable.insert(id) {
            continue;
        }
        let node = node_ids
            .get(id)
            .context("Missing resolved dependency node")?;
        for dependency in node["dependencies"]
            .as_array()
            .context("Dependency node edges")?
        {
            pending.push(dependency.as_str().context("Dependency id")?);
        }
    }
    let integrated = if by_id[root]["name"] == "xcss-web" {
        frontend.to_owned()
    } else {
        frontend.join("vendor/xcss-web")
    };
    let facade = reachable
        .iter()
        .filter_map(|id| by_id.get(id).filter(|p| p["name"] == "xcss-web").copied())
        .collect::<Vec<_>>();
    ensure!(
        facade.len() == 1,
        "CSR requires exactly one integrated xcss-web facade"
    );
    ensure!(
        facade[0]["source"].is_null()
            && Path::new(
                facade[0]["manifest_path"]
                    .as_str()
                    .context("Facade manifest")?
            )
            .canonicalize()?
                == integrated.join("Cargo.toml").canonicalize()?,
        "xcss-web must resolve to the integrated facade source"
    );
    let mut selected = Vec::new();
    for (name, directory) in [
        ("xcss-web-core", "xcss-web-core"),
        ("xcss-web-macro", "xcss-web-macro"),
    ] {
        let candidates = reachable
            .iter()
            .filter_map(|id| {
                by_id
                    .get(id)
                    .filter(|p| p["name"] == name)
                    .map(|p| (*id, *p))
            })
            .collect::<Vec<_>>();
        ensure!(
            candidates.len() == 1,
            "CSR requires exactly one integrated {name}; found {}",
            candidates.len()
        );
        let (id, package) = candidates[0];
        let expected = integrated.join("vendor").join(directory).join("Cargo.toml");
        let actual = Path::new(
            package["manifest_path"]
                .as_str()
                .context("Selected package manifest")?,
        );
        ensure!(
            package["source"].is_null() && actual.canonicalize()? == expected.canonicalize()?,
            "{name} must resolve to the xcss integrated CSR source"
        );
        let features = node_ids[id]["features"]
            .as_array()
            .context("Resolved package features")?
            .iter()
            .map(|v| v.as_str().context("Feature name"))
            .collect::<Result<BTreeSet<_>>>()?;
        let allowed = if name == "xcss-web-macro" {
            BTreeSet::from(["csr", "__internal_erase_components"])
        } else {
            BTreeSet::from(["csr"])
        };
        ensure!(
            features.contains("csr") && features.is_subset(&allowed),
            "{name} only permits csr and the host macro's internal erase-components flag; selected {features:?}"
        );
        let host_macro = package["targets"]
            .as_array()
            .context("Package targets")?
            .iter()
            .any(|target| {
                target["kind"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "proc-macro"))
            });
        ensure!(
            host_macro == (name == "xcss-web-macro"),
            "Integrated runtime/proc-macro target roles changed"
        );
        selected.push(json!({"name":name,"version":package["version"],"path":inventory::relative(expected.strip_prefix(frontend)?)?,"features":features,"role":if host_macro {"host-proc-macro"} else {"browser"}}));
    }
    for id in reachable {
        let package = by_id[id];
        let name = package["name"].as_str().context("Package name")?;
        ensure!(
            !matches!(
                name,
                "leptos"
                    | "leptos_macro"
                    | "server_fn"
                    | "server_fn_macro"
                    | "leptos_server"
                    | "hydration_context"
                    | "leptos_config"
            ),
            "Server/hydration package unexpectedly selected in CSR graph: {name}"
        );
        if matches!(name, "reactive_graph" | "tachys") {
            let features = node_ids[id]["features"]
                .as_array()
                .context("Dependency features")?;
            ensure!(
                !features.iter().any(|feature| matches!(
                    feature.as_str(),
                    Some("ssr" | "hydrate" | "hydration" | "islands")
                )),
                "{name} selected a server/hydration feature"
            );
        }
    }
    Ok(
        json!({"target":crate::TARGET,"mode":"csr","feature_scope":"cargo-resolution-union","integrated":selected}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_receipt_uses_target_normal_tree_without_host_build_or_dev_packages() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let frontend = temp.path().join("web/xcss-web");
        std::fs::create_dir_all(frontend.join("src"))?;
        std::fs::write(frontend.join("src/lib.rs"), "")?;
        std::fs::write(
            frontend.join("Cargo.toml"),
            "[package]\nname='browser'\nversion='1.0.0'\nedition='2024'\n[dependencies]\nruntime={path='runtime'}\nhost_macro={path='host_macro'}\n[build-dependencies]\nbuild_only={path='build_only'}\n[dev-dependencies]\ndev_only={path='dev_only'}\n",
        )?;
        for name in ["runtime", "host_macro", "build_only", "dev_only"] {
            let path = frontend.join(name);
            std::fs::create_dir_all(path.join("src"))?;
            std::fs::write(path.join("src/lib.rs"), "")?;
            std::fs::write(
                path.join("Cargo.toml"),
                format!(
                    "[package]\nname='{name}'\nversion='1.0.0'\nedition='2024'\n{}",
                    if name == "host_macro" {
                        "[lib]\nproc-macro=true\n"
                    } else {
                        ""
                    }
                ),
            )?;
        }
        let lock = "version = 4\n[[package]]\nname='browser'\nversion='1.0.0'\ndependencies=['runtime','host_macro','build_only','dev_only']\n[[package]]\nname='runtime'\nversion='1.0.0'\n[[package]]\nname='host_macro'\nversion='1.0.0'\n[[package]]\nname='build_only'\nversion='1.0.0'\n[[package]]\nname='dev_only'\nversion='1.0.0'\n";
        std::fs::write(frontend.join("Cargo.lock"), lock)?;
        let options = Options {
            root: temp.path().to_owned(),
            offline: true,
            debug: false,
            dist: None,
        };
        let result = runtime(&options, &json!({"browser_inputs":{},"patched_inputs":{}}))?;
        let names = result["graph"]["packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, ["browser", "runtime"]);
        assert!(!serde_json::to_string(&result)?.contains(temp.path().to_str().unwrap()));
        Ok(())
    }
    fn fixture() -> Result<(tempfile::TempDir, Value)> {
        let temp = tempfile::tempdir()?;
        let frontend = temp.path();
        for name in ["xcss-web-core", "xcss-web-macro"] {
            std::fs::create_dir_all(frontend.join("vendor/xcss-web/vendor").join(name))?;
            std::fs::write(
                frontend
                    .join("vendor/xcss-web/vendor")
                    .join(name)
                    .join("Cargo.toml"),
                "test",
            )?;
        }
        let facade = frontend.join("vendor/xcss-web/Cargo.toml");
        std::fs::write(&facade, "test")?;
        let core = frontend.join("vendor/xcss-web/vendor/xcss-web-core/Cargo.toml");
        let macros = frontend.join("vendor/xcss-web/vendor/xcss-web-macro/Cargo.toml");
        let value = json!({"packages":[
            {"id":"root","name":"product-web","version":"1","source":null,"manifest_path":frontend.join("Cargo.toml"),"targets":[{"kind":["cdylib"]}]},
            {"id":"core","name":"xcss-web-core","version":"0.8.19","source":null,"manifest_path":core,"targets":[{"kind":["lib"]}]},
            {"id":"macro","name":"xcss-web-macro","version":"0.8.17","source":null,"manifest_path":macros,"targets":[{"kind":["proc-macro"]}]},
            {"id":"facade","name":"xcss-web","version":"1.0.0","source":null,"manifest_path":facade,"targets":[{"kind":["lib"]}]},
            {"id":"graph","name":"reactive_graph","version":"0.2","source":"registry","manifest_path":"registry","targets":[{"kind":["lib"]}]}
        ],"resolve":{"root":"root","nodes":[
            {"id":"root","dependencies":["facade"],"features":["default"]},
            {"id":"core","dependencies":["macro","graph"],"features":["csr"]},
            {"id":"macro","dependencies":[],"features":["csr"]},
            {"id":"facade","dependencies":["core"],"features":[]},
            {"id":"graph","dependencies":[],"features":["default","effects"]}
        ]}});
        Ok((temp, value))
    }
    #[test]
    fn csr_graph_distinguishes_host_macros_and_unrelated_default_features() -> Result<()> {
        let (temp, value) = fixture()?;
        let receipt = validate(&value, temp.path())?;
        assert_eq!(receipt["integrated"][1]["role"], "host-proc-macro");
        Ok(())
    }
    #[test]
    fn csr_graph_rejects_upstream_framework_beside_integrated_runtime() -> Result<()> {
        let (temp, mut value) = fixture()?;
        value["packages"].as_array_mut().unwrap().push(json!({
            "id":"upstream","name":"leptos","version":"0.8.19","source":"registry",
            "manifest_path":"registry","targets":[{"kind":["lib"]}]
        }));
        value["resolve"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id":"upstream","dependencies":[],"features":["csr"]
            }));
        value["resolve"]["nodes"][0]["dependencies"]
            .as_array_mut()
            .unwrap()
            .push(json!("upstream"));
        assert!(validate(&value, temp.path()).is_err());
        Ok(())
    }
    #[test]
    fn csr_graph_rejects_full_and_hydration_and_registry_runtime() -> Result<()> {
        let (temp, value) = fixture()?;
        let mut full = value.clone();
        full["resolve"]["nodes"][1]["features"] = json!(["csr", "full"]);
        assert!(validate(&full, temp.path()).is_err());
        let mut hydration = value.clone();
        hydration["resolve"]["nodes"][4]["features"] = json!(["default", "hydration"]);
        assert!(validate(&hydration, temp.path()).is_err());
        let mut registry = value.clone();
        registry["packages"][1]["source"] = json!("registry");
        assert!(validate(&registry, temp.path()).is_err());
        let mut registry_facade = value;
        registry_facade["packages"][3]["source"] = json!("registry");
        assert!(validate(&registry_facade, temp.path()).is_err());
        Ok(())
    }
}
