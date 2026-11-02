use anyhow::{Context, Result, bail};
use std::{collections::BTreeMap, env, path::PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("xcss-web-build: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    let mut values = BTreeMap::<String, Vec<String>>::new();
    while let Some(key) = args.next() {
        if matches!(
            key.as_str(),
            "--offline"
                | "--debug"
                | "--no-watch"
                | "--skip-build"
                | "--web-only"
                | "--rust-only"
                | "--no-install"
        ) {
            values.entry(key).or_default();
        } else if key.starts_with("--") {
            values.entry(key.clone()).or_default().push(
                args.next()
                    .with_context(|| format!("{key} requires a value"))?,
            );
        } else {
            bail!("Unexpected argument {key:?}");
        }
    }
    let allowed = [
        "--product-root",
        "--offline",
        "--debug",
        "--dist",
        "--bind",
        "--proxy",
        "--no-watch",
        "--source",
        "--destination",
        "--origin",
        "--revision",
        "--include",
        "--skip-build",
        "--config",
        "--mode",
        "--source-revision",
        "--web-only",
        "--rust-only",
        "--no-install",
        "--binary",
        "--receipt",
    ];
    for key in values.keys() {
        if !allowed.contains(&key.as_str()) {
            bail!("Unknown option {key}");
        }
        if key != "--include" && key != "--proxy" && values[key].len() > 1 {
            bail!("Duplicate option {key}");
        }
    }
    let value = |name: &str| values.get(name).and_then(|v| v.first()).cloned();
    let root = value("--product-root")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?)
        .canonicalize()?;
    let options = xcss_web_build::Options {
        root,
        offline: values.contains_key("--offline"),
        debug: values.contains_key("--debug"),
        dist: value("--dist")
            .map(PathBuf::from)
            .or_else(|| env::var_os("XCSS_WEB_DIST").map(PathBuf::from)),
    };
    match command.as_str() {
        "build" => {
            xcss_web_build::build(&options)?;
        }
        "server" => {
            let mode = value("--mode").unwrap_or_else(|| "development".into());
            let config = value("--config");
            let revision = value("--source-revision");
            let output = xcss_web_build::server_build::build(
                &options,
                config.as_deref(),
                &mode,
                values.contains_key("--web-only"),
                values.contains_key("--rust-only"),
                revision.as_deref(),
            )?;
            println!("{}", output.display());
        }
        "verify" => {
            let binary = PathBuf::from(value("--binary").context("verify requires --binary")?);
            let receipt = PathBuf::from(value("--receipt").context("verify requires --receipt")?);
            let dist = xcss_web_build::distribution(&options)?;
            xcss_web_build::server_build::verify(&binary, &dist, &receipt)?;
            println!("Verified {}", binary.display());
        }
        "check" | "test" => xcss_web_build::cargo_action(&options, &command)?,
        "graph" => println!("{}", xcss_web_build::graph::resolve(&options)?),
        "check-vendor" => {
            xcss_web_build::inventory::validate_vendors(&options.root.join("web/vendor"))?
        }
        "vendor" => {
            let required = |key: &str| value(key).with_context(|| format!("vendor requires {key}"));
            xcss_web_build::inventory::vendor(
                &PathBuf::from(required("--source")?),
                &options.root.join("web").join(required("--destination")?),
                &required("--origin")?,
                &required("--revision")?,
                values.get("--include").map(Vec::as_slice).unwrap_or(&[]),
            )?;
        }
        "serve" | "preview" => {
            let bind = value("--bind")
                .unwrap_or_else(|| "127.0.0.1:5173".into())
                .parse()?;
            let proxies = values.get("--proxy").cloned().unwrap_or_default();
            tokio::runtime::Runtime::new()?.block_on(xcss_web_build::dev::serve(
                options,
                bind,
                proxies,
                command == "serve" && !values.contains_key("--no-watch"),
                command == "preview" || values.contains_key("--skip-build"),
            ))?;
        }
        "help" | "--help" | "-h" => println!(
            "xcss-web-build build|check|test|serve|preview|server|vendor|check-vendor\n  --product-root DIR --offline --debug --dist DIR\n  serve: --bind 127.0.0.1:5173 --proxy /api=http://127.0.0.1:8080 [--no-watch] [--skip-build]\n  server: --mode development|release --config xcss-web-build.json [--web-only|--rust-only] [--source-revision SHA]\n  vendor: --source DIR --destination vendor/NAME --origin HTTPS_URL --revision IMMUTABLE_ID --include PATH (repeatable)\nConfiguration: web/rust-web.toml; see tools/web-build/README.md"
        ),
        _ => bail!("Unknown command {command:?}; use help"),
    }
    Ok(())
}
