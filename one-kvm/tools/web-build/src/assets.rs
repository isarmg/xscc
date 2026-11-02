//! A deliberately bounded ESM linker: no transpilation, eval, npm install or network.
//! Only static ESM imports, CSS imports and literal import.meta.url assets are accepted.
use crate::inventory::{self, Files};
use anyhow::{Context, Result, bail, ensure};
use flate2::{Compression, GzBuilder};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const MAX_PUBLIC_MEDIA_BYTES: usize = 4 * 1024 * 1024;
const MAX_PUBLIC_MEDIA_TOTAL_BYTES: usize = 8 * 1024 * 1024;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicAssetsManifest {
    format: u32,
    #[serde(deserialize_with = "unique_public_files")]
    files: Files,
}

fn unique_public_files<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Files, D::Error> {
    struct UniqueFiles;
    impl<'de> serde::de::Visitor<'de> for UniqueFiles {
        type Value = Files;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a unique public-relative-path to SHA-256 map")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<Files, M::Error> {
            let mut files = Files::new();
            while let Some((name, digest)) = map.next_entry::<String, String>()? {
                if files.insert(name.clone(), digest).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "Duplicate public asset: {name}"
                    )));
                }
            }
            Ok(files)
        }
    }
    deserializer.deserialize_map(UniqueFiles)
}

fn public_manifest_path(web: &Path, name: &str) -> Result<PathBuf> {
    let name = Path::new(name);
    inventory::relative(name)?;
    ensure!(
        name.components().count() == 1 && name.extension().is_some_and(|ext| ext == "json"),
        "public_assets_manifest must name a JSON file directly in web/"
    );
    Ok(web.join(name))
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub web: Web,
    pub aliases: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            web: Web::default(),
            aliases: BTreeMap::from([("@xcss/web/".into(), "vendor/xcss/".into())]),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Web {
    pub kind: String,
    pub entry: String,
    pub entries: BTreeMap<String, String>,
    pub copies: BTreeMap<String, String>,
    pub css_output: String,
    pub relative_urls: bool,
    pub runtime_root: String,
    pub dist: String,
    pub base: String,
    pub max_asset_bytes: usize,
    pub max_wasm_bytes: usize,
    pub max_font_bytes: usize,
    pub max_total_bytes: usize,
    pub public_assets_manifest: String,
    pub licenses: Vec<String>,
    pub proxies: BTreeMap<String, String>,
}

impl Default for Web {
    fn default() -> Self {
        Self {
            kind: "spa".into(),
            entry: String::new(),
            entries: BTreeMap::new(),
            copies: BTreeMap::new(),
            css_output: String::new(),
            relative_urls: false,
            runtime_root: "web".into(),
            dist: "web/dist".into(),
            base: "/".into(),
            max_asset_bytes: 512 * 1024,
            max_wasm_bytes: 4 * 1024 * 1024,
            max_font_bytes: 16 * 1024 * 1024,
            max_total_bytes: 16 * 1024 * 1024,
            public_assets_manifest: String::new(),
            licenses: vec![
                "vendor/xcss/web-fonts/OFL.txt".into(),
                "vendor/xcss/web-fonts/CJK-LICENSE.txt".into(),
                "vendor/xcss/web-fonts/NORMAL-LICENSE.txt".into(),
            ],
            proxies: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn read(web: &Path) -> Result<Self> {
        let path = web.join("rust-web.toml");
        let mut config: Self = if path.exists() {
            toml::from_str(&String::from_utf8(inventory::read(&path)?)?)?
        } else {
            Self::default()
        };
        if config.web.entry.is_empty() {
            config.web.entry = if web.join("src/main-xcss-web.js").is_file() {
                "src/main-xcss-web.js"
            } else {
                "src/main.js"
            }
            .into();
        }
        inventory::relative(Path::new(&config.web.entry))?;
        ensure!(
            matches!(config.web.kind.as_str(), "spa" | "embedded"),
            "web.kind must be spa or embedded"
        );
        ensure!(
            matches!(config.web.runtime_root.as_str(), "web" | "product"),
            "web.runtime_root must be web or product"
        );
        if config.web.kind == "embedded" {
            ensure!(
                !config.web.entries.is_empty(),
                "Embedded output needs explicit entry points"
            );
        }
        for (output, source) in config.web.entries.iter().chain(config.web.copies.iter()) {
            inventory::relative(Path::new(output))?;
            inventory::relative(Path::new(source))?;
        }
        if !config.web.css_output.is_empty() {
            inventory::relative(Path::new(&config.web.css_output))?;
        }
        if !config.web.public_assets_manifest.is_empty() {
            public_manifest_path(web, &config.web.public_assets_manifest)?;
        }
        ensure!(
            config.web.base.starts_with('/')
                && config.web.base.ends_with('/')
                && !config
                    .web
                    .base
                    .contains(['\\', '?', '#', '"', '\'', '<', '>', '\n', '\r'])
                && !config.web.base.contains("..")
                && !config.web.base.starts_with("//"),
            "base must be a local /path/ URL"
        );
        for limit in [
            config.web.max_asset_bytes,
            config.web.max_wasm_bytes,
            config.web.max_font_bytes,
            config.web.max_total_bytes,
        ] {
            ensure!(
                (1..=64 * 1024 * 1024).contains(&limit),
                "Asset budget must be in 1..67108864"
            );
        }
        // Raising an established project budget requires a separately reviewed code change.
        ensure!(
            config.web.max_asset_bytes <= 512 * 1024
                && config.web.max_wasm_bytes <= 4 * 1024 * 1024
                && config.web.max_font_bytes <= 16 * 1024 * 1024
                && config.web.max_total_bytes <= 16 * 1024 * 1024,
            "Configuration may tighten but cannot relax asset budgets"
        );
        for destination in config.aliases.values() {
            inventory::relative(Path::new(destination.trim_end_matches('/')))?;
        }
        Ok(config)
    }
}

#[derive(Clone)]
struct Module {
    source: PathBuf,
    output: String,
    imports: BTreeSet<PathBuf>,
    facades: Vec<String>,
}

pub struct Linker {
    web: PathBuf,
    pkg: PathBuf,
    pub config: Config,
    pub outputs: BTreeMap<String, Vec<u8>>,
    public_outputs: BTreeSet<String>,
    public_media: BTreeSet<String>,
    modules: BTreeMap<PathBuf, Module>,
    visiting: BTreeSet<PathBuf>,
    assets: BTreeMap<PathBuf, String>,
    css_seen: BTreeSet<PathBuf>,
    css_stack: BTreeSet<PathBuf>,
    css: Vec<String>,
}

struct CssUrls<'a> {
    linker: &'a mut Linker,
    source: &'a Path,
}
impl<'i> lightningcss::visitor::Visitor<'i> for CssUrls<'_> {
    type Error = anyhow::Error;
    fn visit_types(&self) -> lightningcss::visitor::VisitTypes {
        lightningcss::visitor::VisitTypes::URLS
    }
    fn visit_url(&mut self, url: &mut lightningcss::values::url::Url<'i>) -> Result<()> {
        let value = url.url.to_string();
        if value.starts_with('#') {
            return Ok(());
        }
        if value.starts_with("data:") {
            if self.linker.config.web.kind == "embedded" {
                let encoded=value.strip_prefix("data:image/svg+xml,").context("Embedded CSP only supports authored SVG data URLs, emitted as same-origin files")?;
                let svg = percent_encoding::percent_decode_str(encoded)
                    .decode_utf8()?
                    .into_owned();
                let output = self.linker.output("xcss-icon", "svg", svg.into_bytes())?;
                url.url = self.linker.css_url(&output).into();
            }
            return Ok(());
        }
        let specifier = if value.starts_with('.') || value.starts_with('/') {
            value
        } else {
            format!("./{value}")
        };
        let source = self
            .linker
            .resolve_with_public_priority(self.source, &specifier, true)?;
        let output = self.linker.asset(&source)?;
        url.url = self.linker.css_url(&output).into();
        Ok(())
    }
}

impl Linker {
    pub fn new(web: &Path, pkg: &Path, config: Config) -> Self {
        Self {
            web: web.into(),
            pkg: pkg.into(),
            config,
            outputs: BTreeMap::new(),
            public_outputs: BTreeSet::new(),
            public_media: BTreeSet::new(),
            modules: BTreeMap::new(),
            visiting: BTreeSet::new(),
            assets: BTreeMap::new(),
            css_seen: BTreeSet::new(),
            css_stack: BTreeSet::new(),
            css: Vec::new(),
        }
    }

    fn resolve(&self, parent: &Path, specifier: &str) -> Result<PathBuf> {
        self.resolve_with_public_priority(parent, specifier, false)
    }

    fn resolve_with_public_priority(
        &self,
        parent: &Path,
        specifier: &str,
        public_first: bool,
    ) -> Result<PathBuf> {
        ensure!(
            !specifier.contains(['\n', '\r', '\0', '\\', '#']),
            "Unsupported module/resource URL {specifier:?}"
        );
        let mut parts = specifier.split('?');
        let plain = parts.next().unwrap_or_default();
        if let Some(query) = parts.next() {
            ensure!(
                query == "url" && parts.next().is_none(),
                "Only ?url asset imports are supported"
            );
        }
        let mut aliased = None;
        let mut aliases = self.config.aliases.iter().collect::<Vec<_>>();
        aliases.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
        for (name, destination) in aliases {
            if plain == name || name.ends_with('/') && plain.starts_with(name) {
                aliased = Some(if plain == name {
                    self.web.join(destination)
                } else {
                    self.web
                        .join(destination)
                        .join(plain.strip_prefix(name).unwrap_or_default())
                });
                break;
            }
        }
        let path = if let Some(path) = aliased {
            path
        } else if plain.starts_with('.') {
            parent
                .parent()
                .context("Missing module parent")?
                .join(plain)
        } else if plain.starts_with('/') {
            // Public URLs are rooted at the output, not at a `public/` URL prefix.
            // CSS follows Vite's public-first URL lookup. ESM imports prefer an
            // actual Web source file, with public as a bounded fallback.
            let relative = plain.strip_prefix('/').context("Absolute resource URL")?;
            inventory::relative(Path::new(relative))?;
            let web = self.web.join(relative);
            let public = self.web.join("public").join(relative);
            let (first, second) = if public_first {
                (public, web)
            } else {
                (web, public)
            };
            let mut candidate = first;
            let staged = |path: &Path| {
                path.strip_prefix(self.web.join("pkg"))
                    .map(|relative| self.pkg.join(relative))
                    .unwrap_or_else(|_| path.into())
            };
            let mut physical = staged(&candidate);
            if !physical.try_exists()?
                && fs::symlink_metadata(&physical).is_err()
                && candidate.extension().is_none()
            {
                candidate.set_extension("js");
                physical = staged(&candidate);
            }
            // Do not conceal a dangling source symlink by falling back to public.
            if physical.try_exists()? || fs::symlink_metadata(&physical).is_ok() {
                candidate
            } else {
                second
            }
        } else {
            bail!("Unvendored bare import {specifier:?}; configure an explicit alias");
        };
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::ParentDir => {
                    ensure!(normalized.pop(), "Import path escapes filesystem root");
                }
                std::path::Component::CurDir => {}
                component => normalized.push(component.as_os_str()),
            }
        }
        let mut path = if normalized.starts_with(self.web.join("pkg")) {
            self.pkg
                .join(normalized.strip_prefix(self.web.join("pkg"))?)
        } else {
            normalized
        };
        if !path.exists() && path.extension().is_none() {
            path.set_extension("js");
        }
        inventory::physical(&path)?;
        let path = path
            .canonicalize()
            .with_context(|| format!("Resolve {specifier:?} in {}", parent.display()))?;
        ensure!(
            path.starts_with(&self.web) || path.starts_with(&self.pkg),
            "Import escapes the Web source workspace"
        );
        ensure!(path.is_file(), "Import must refer to a physical file");
        Ok(path)
    }

    fn generated_output(&mut self, name: String, bytes: Vec<u8>) -> Result<()> {
        ensure!(
            !self.public_outputs.contains(&name),
            "Public asset shadows a generated resource: {name}"
        );
        if let Some(previous) = self.outputs.get(&name) {
            ensure!(previous == &bytes, "Conflicting generated output: {name}");
        } else {
            self.outputs.insert(name, bytes);
        }
        Ok(())
    }

    fn public_output(&mut self, source: &Path, bytes: Vec<u8>) -> Result<Option<String>> {
        let Ok(relative) = source.strip_prefix(self.web.join("public")) else {
            return Ok(None);
        };
        let name = inventory::relative(relative)?;
        ensure!(name != "index.html", "Public asset shadows generated HTML");
        if let Some(previous) = self.outputs.get(&name) {
            ensure!(
                self.public_outputs.contains(&name) && previous == &bytes,
                "Public asset shadows a generated resource: {name}"
            );
        } else {
            self.outputs.insert(name.clone(), bytes);
            self.public_outputs.insert(name.clone());
        }
        Ok(Some(name))
    }

    fn copy_public(&mut self) -> Result<()> {
        self.public_media.clear();
        let mut actual = Files::new();
        for path in inventory::walk(&self.web.join("public"))? {
            let bytes = inventory::read(&path)?;
            let digest = inventory::sha(&bytes);
            let name = self
                .public_output(&path, bytes)?
                .context("Public source path")?;
            actual.insert(name, digest);
        }
        if !self.config.web.public_assets_manifest.is_empty() {
            let path = public_manifest_path(&self.web, &self.config.web.public_assets_manifest)?;
            let expected: PublicAssetsManifest = serde_json::from_slice(&inventory::read(&path)?)?;
            ensure!(
                expected.format == 1,
                "Unsupported public assets manifest format"
            );
            for (name, digest) in &expected.files {
                inventory::relative(Path::new(name))?;
                ensure!(
                    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "Invalid public asset SHA-256: {name}"
                );
            }
            ensure!(
                actual == expected.files,
                "Public assets differ from source inventory: {}",
                path.display()
            );
            self.public_media = actual
                .into_keys()
                .filter(|name| {
                    matches!(
                        mime_guess::from_path(name)
                            .first_or_octet_stream()
                            .type_()
                            .as_str(),
                        "image" | "video" | "audio"
                    )
                })
                .collect();
        }
        Ok(())
    }

    fn output(&mut self, stem: &str, extension: &str, bytes: Vec<u8>) -> Result<String> {
        let stem: String = stem
            .chars()
            .map(|v| {
                if v.is_ascii_alphanumeric() || v == '-' || v == '_' {
                    v
                } else {
                    '-'
                }
            })
            .collect();
        let name = format!(
            "assets/{stem}-{}.{}",
            &inventory::sha(&bytes)[..16],
            extension
        );
        self.generated_output(name.clone(), bytes)?;
        Ok(name)
    }

    fn url(&self, output: &str) -> String {
        if self.config.web.relative_urls {
            format!("./{output}")
        } else {
            format!("{}{}", self.config.web.base, output)
        }
    }

    fn module_url(&self, output: &str) -> String {
        if self.config.web.relative_urls {
            if let Some(name) = output.strip_prefix("assets/") {
                format!("./{name}")
            } else {
                format!("../{output}")
            }
        } else {
            self.url(output)
        }
    }

    fn css_url(&self, output: &str) -> String {
        if self.config.web.relative_urls {
            let depth = Path::new(&self.config.web.css_output)
                .parent()
                .map(|p| p.components().count())
                .unwrap_or(1);
            format!("{}{output}", "../".repeat(depth))
        } else {
            self.url(output)
        }
    }

    fn asset(&mut self, source: &Path) -> Result<String> {
        if let Some(name) = self.assets.get(source) {
            return Ok(name.clone());
        }
        let extension = source.extension().and_then(|v| v.to_str()).unwrap_or("bin");
        ensure!(extension != "map", "Source maps cannot be published");
        let bytes = inventory::read(source)?;
        if extension == "wasm" {
            ensure!(bytes.starts_with(b"\0asm"), "Expected WebAssembly bytes");
        }
        let fixed = self.config.web.copies.iter().find_map(|(output, input)| {
            self.web
                .join(input)
                .canonicalize()
                .ok()
                .filter(|path| path == source)
                .map(|_| output.clone())
        });
        let output = if let Some(output) = self.public_output(source, bytes.clone())? {
            output
        } else if let Some(output) = fixed {
            self.generated_output(output.clone(), bytes.clone())?;
            output
        } else {
            self.output(
                source
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .unwrap_or("asset"),
                extension,
                bytes.clone(),
            )?
        };
        if extension == "wasm" {
            let mut compressed = GzBuilder::new()
                .mtime(0)
                .write(Vec::new(), Compression::best());
            compressed.write_all(&bytes)?;
            self.generated_output(format!("{output}.gz"), compressed.finish()?)?;
        }
        self.assets.insert(source.into(), output.clone());
        Ok(output)
    }

    fn stylesheet(&mut self, source: &Path) -> Result<String> {
        use lightningcss::{
            rules::CssRule,
            stylesheet::{ParserOptions, PrinterOptions, StyleSheet},
            visitor::Visit,
        };
        ensure!(
            !self.css_stack.contains(source),
            "Circular CSS @import: {}",
            source.display()
        );
        if self.css_seen.contains(source) {
            return Ok(String::new());
        }
        self.css_stack.insert(source.into());
        let text = String::from_utf8(inventory::read(source)?)?;
        let mut sheet = StyleSheet::parse(&text, ParserOptions::default())
            .map_err(|e| anyhow::anyhow!("CSS syntax error in {}: {e}", source.display()))?;
        let mut imported = String::new();
        for rule in &sheet.rules.0 {
            if let CssRule::Import(import) = rule {
                ensure!(
                    import.media.media_queries.is_empty()
                        && import.supports.is_none()
                        && import.layer.is_none(),
                    "Conditional CSS imports must be explicitly bundled upstream"
                );
                let specifier = import.url.as_ref();
                let path = self.resolve(
                    source,
                    if specifier.starts_with('.') || specifier.starts_with('/') {
                        specifier.to_owned()
                    } else {
                        format!("./{specifier}")
                    }
                    .as_str(),
                )?;
                imported.push_str(&self.stylesheet(&path)?);
            }
        }
        sheet
            .rules
            .0
            .retain(|rule| !matches!(rule, CssRule::Import(_)));
        sheet.visit(&mut CssUrls {
            linker: self,
            source,
        })?;
        sheet
            .minify(lightningcss::stylesheet::MinifyOptions::default())
            .map_err(|e| anyhow::anyhow!("CSS minify error: {e}"))?;
        let code = sheet
            .to_css(PrinterOptions {
                minify: true,
                ..Default::default()
            })
            .map_err(|e| anyhow::anyhow!("CSS print error: {e}"))?
            .code;
        imported.push_str(&code);
        self.css_stack.remove(source);
        self.css_seen.insert(source.into());
        Ok(imported)
    }

    pub fn module(&mut self, source: &Path) -> Result<String> {
        let source = source.canonicalize()?;
        if let Some(module) = self.modules.get(&source) {
            return Ok(module.output.clone());
        }
        ensure!(
            self.visiting.insert(source.clone()),
            "Circular ESM imports require a reviewed prebundled ESM distribution: {}",
            source.display()
        );
        let text = String::from_utf8(inventory::read(&source)?)?;
        let references = crate::javascript::references(&text)
            .with_context(|| format!("Parse {}", source.display()))?;
        let mut replacements = Vec::new();
        let mut children = BTreeSet::new();
        for import in references.imports {
            let child = self.resolve(&source, &import.url)?;
            if child.extension().is_some_and(|v| v == "css") {
                ensure!(
                    !import.has_bindings,
                    "CSS imports cannot create JavaScript bindings"
                );
                let css = self.stylesheet(&child)?;
                self.css.push(css);
                let (start, end) = crate::javascript::range(import.statement);
                replacements.push((start, end, String::new()));
            } else if import.url.ends_with("?url") {
                let binding = import
                    .asset_binding
                    .context("?url requires a default import or export {default as name}")?;
                let output = self.asset(&child)?;
                let value = serde_json::to_string(&self.module_url(&output))?;
                let expression = if self.config.web.relative_urls {
                    format!("new URL({value},import.meta.url).href")
                } else {
                    value
                };
                let (start, end) = crate::javascript::range(import.statement);
                replacements.push((
                    start,
                    end,
                    format!(
                        "{}const {binding}={expression};",
                        if import.is_export { "export " } else { "" }
                    ),
                ));
            } else {
                ensure!(
                    matches!(
                        child.extension().and_then(|v| v.to_str()),
                        Some("js" | "mjs")
                    ),
                    "ESM module must be .js or .mjs"
                );
                let output = self.module(&child)?;
                children.insert(child);
                let (start, end) = crate::javascript::range(import.source);
                replacements.push((
                    start,
                    end,
                    serde_json::to_string(&self.module_url(&output))?,
                ));
            }
        }
        for (url, span) in references.assets {
            let specifier = if url.starts_with('.') || url.starts_with('/') {
                url
            } else {
                format!("./{url}")
            };
            let child = self.resolve(&source, &specifier)?;
            let output = self.asset(&child)?;
            let (start, end) = crate::javascript::range(span);
            replacements.push((
                start,
                end,
                serde_json::to_string(&self.module_url(&output))?,
            ));
        }
        for (url, span) in references.dynamic {
            let child = self.resolve(&source, &url)?;
            let output = self.module(&child)?;
            children.insert(child);
            let (start, end) = crate::javascript::range(span);
            replacements.push((
                start,
                end,
                serde_json::to_string(&self.module_url(&output))?,
            ));
        }
        replacements.sort_by_key(|(start, _, _)| *start);
        let mut transformed = String::new();
        let mut end = 0;
        for (start, finish, replacement) in replacements {
            ensure!(start >= end, "Ambiguous overlapping AST references");
            transformed.push_str(&text[end..start]);
            transformed.push_str(&replacement);
            end = finish;
        }
        transformed.push_str(&text[end..]);
        let bytes = crate::javascript::minify(&transformed)?;
        let output = self.output(
            source
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or("module"),
            "js",
            bytes,
        )?;
        self.modules.insert(
            source.clone(),
            Module {
                source: source.clone(),
                output: output.clone(),
                imports: children,
                facades: Vec::new(),
            },
        );
        self.visiting.remove(&source);
        Ok(output)
    }

    pub fn finish(mut self, provenance: &[u8]) -> Result<(BTreeMap<String, Vec<u8>>, Value)> {
        self.copy_public()?;
        let mut script = String::new();
        if self.config.web.kind == "embedded" {
            for (facade, input) in self.config.web.entries.clone() {
                let source = self.web.join(input).canonicalize()?;
                let output = self.module(&source)?;
                let depth = Path::new(&facade)
                    .parent()
                    .map(|p| p.components().count())
                    .unwrap_or(0);
                let target = format!("{}{output}", "../".repeat(depth));
                let wrapper = crate::javascript::minify(&format!(
                    "export * from {};",
                    serde_json::to_string(&if target.starts_with('.') {
                        target
                    } else {
                        format!("./{target}")
                    })?
                ))?;
                self.generated_output(facade.clone(), wrapper)?;
                self.modules
                    .get_mut(&source)
                    .context("Entry module")?
                    .facades
                    .push(facade);
            }
        } else {
            let entry = self.web.join(&self.config.web.entry);
            script = self.module(&entry)?;
        }
        ensure!(
            self.outputs.keys().any(|name| name.ends_with(".wasm")),
            "No independent WASM output was linked"
        );
        let css_bytes = self.css.join("\n").into_bytes();
        let css = if self.config.web.css_output.is_empty() {
            self.output("style", "css", css_bytes)?
        } else {
            let name = self.config.web.css_output.clone();
            self.generated_output(name.clone(), css_bytes)?;
            name
        };
        for (output, input) in self.config.web.copies.clone() {
            let source = self.web.join(input);
            let bytes = inventory::read(&source)?;
            self.generated_output(output, bytes)?;
        }
        for license in self.config.web.licenses.clone() {
            let path = self.web.join(&license);
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .context("License filename")?;
            let output = format!("assets/{name}");
            let bytes = inventory::read(&path)?;
            ensure!(!bytes.is_empty(), "Required license is empty");
            self.generated_output(output, bytes)?;
        }
        self.generated_output("assets/xcss-web-build.json".into(), provenance.into())?;
        let vendor_sources = inventory::walk(&self.web.join("vendor"))?
            .into_iter()
            .filter(|p| p.file_name().is_some_and(|v| v == "SOURCE.json"))
            .map(|p| {
                Ok((
                    inventory::relative(p.strip_prefix(&self.web)?)?,
                    serde_json::from_slice::<Value>(&inventory::read(&p)?)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        self.generated_output(
            "assets/vendor-sources.json".into(),
            serde_json::to_vec_pretty(&vendor_sources)?,
        )?;
        if self.config.web.kind == "spa" {
            let mut html = String::from_utf8(inventory::read(&self.web.join("index.html"))?)?;
            let script_tag = Regex::new(r#"<script\s+[^>]*type=["']module["'][^>]*>\s*</script>"#)?;
            ensure!(
                script_tag.find_iter(&html).count() == 1,
                "index.html must have exactly one external module entry"
            );
            html = script_tag
                .replace(
                    &html,
                    format!(
                        "<script type=\"module\" src=\"{}\"></script>",
                        self.url(&script)
                    ),
                )
                .into_owned();
            let node_fonts = Regex::new(
                r#"(?:\./|/)?(?:node_modules/@xcss/web/dist|vendor/xcss)/web-fonts/([A-Za-z0-9_.-]+)"#,
            )?;
            let mut rewritten = String::new();
            let mut end = 0;
            for capture in node_fonts.captures_iter(&html) {
                let found = capture.get(0).context("Font preload")?;
                rewritten.push_str(&html[end..found.start()]);
                let path = self.web.join("vendor/xcss/web-fonts").join(&capture[1]);
                let output = self.asset(&path)?;
                rewritten.push_str(&self.url(&output));
                end = found.end();
            }
            rewritten.push_str(&html[end..]);
            html = rewritten;
            ensure!(
                !html.contains("node_modules") && !html.contains("/src/"),
                "HTML still contains a development-only resource reference"
            );
            let mut preload = format!("<link rel=\"stylesheet\" href=\"{}\">\n", self.url(&css));
            for name in self
                .outputs
                .keys()
                .filter(|name| name.ends_with(".js") && **name != script)
            {
                preload.push_str(&format!(
                    "<link rel=\"modulepreload\" href=\"{}\">\n",
                    self.url(name)
                ));
            }
            for name in self.outputs.keys().filter(|name| name.ends_with(".wasm")) {
                preload.push_str(&format!("<link rel=\"preload\" href=\"{}\" as=\"fetch\" type=\"application/wasm\" crossorigin=\"anonymous\">\n", self.url(name)));
            }
            ensure!(html.contains("</head>"), "HTML is missing head terminator");
            html = html.replacen("</head>", &format!("{preload}</head>"), 1);
            self.generated_output("index.html".into(), html.into_bytes())?;
        }
        self.budgets()?;
        let ordered = self.modules.keys().cloned().collect::<Vec<_>>();
        let ids = ordered
            .iter()
            .enumerate()
            .map(|(id, path)| (path.clone(), id))
            .collect::<BTreeMap<_, _>>();
        let mut modules = Vec::new();
        for path in ordered {
            let module = &self.modules[&path];
            let relative = if module.source.starts_with(&self.pkg) {
                format!(
                    "pkg/{}",
                    inventory::relative(module.source.strip_prefix(&self.pkg)?)?
                )
            } else {
                inventory::relative(module.source.strip_prefix(&self.web)?)?
            };
            let relative = if self.config.web.runtime_root == "product" {
                format!("web/{relative}")
            } else {
                relative
            };
            let mut delivered = vec![module.output.clone()];
            delivered.extend(module.facades.clone());
            modules.push(json!({"path":relative,"sha256":inventory::sha(&inventory::read(&module.source)?),"files":delivered,"imports":module.imports.iter().map(|path|ids[path]).collect::<Vec<_>>()}));
        }
        let outputs = self
            .outputs
            .iter()
            .map(|(name, bytes)| json!({"path":name,"sha256":inventory::sha(bytes)}))
            .collect::<Vec<_>>();
        Ok((
            self.outputs,
            json!({"format_version":1,"distribution":"dist","modules":modules,"outputs":outputs}),
        ))
    }

    fn budgets(&self) -> Result<()> {
        let mut bundle = 0;
        let mut media = 0;
        for (name, bytes) in &self.outputs {
            ensure!(!name.ends_with(".map"), "Source maps cannot be published");
            if self.public_media.contains(name) {
                ensure!(
                    bytes.len() <= MAX_PUBLIC_MEDIA_BYTES,
                    "Public media {name} exceeds {MAX_PUBLIC_MEDIA_BYTES}-byte budget ({} bytes)",
                    bytes.len()
                );
                media += bytes.len();
                continue;
            }
            let limit = if name.ends_with(".woff") || name.ends_with(".woff2") {
                self.config.web.max_font_bytes
            } else if name.ends_with(".wasm") || name.ends_with(".wasm.gz") {
                self.config.web.max_wasm_bytes
            } else {
                self.config.web.max_asset_bytes
            };
            ensure!(
                bytes.len() <= limit,
                "Asset {name} exceeds {limit}-byte budget ({} bytes)",
                bytes.len()
            );
            bundle += bytes.len();
        }
        ensure!(
            bundle <= self.config.web.max_total_bytes,
            "Web bundle exceeds {}-byte total budget ({bundle} bytes)",
            self.config.web.max_total_bytes
        );
        ensure!(
            media <= MAX_PUBLIC_MEDIA_TOTAL_BYTES,
            "Public media exceed {MAX_PUBLIC_MEDIA_TOTAL_BYTES}-byte total budget ({media} bytes)"
        );
        Ok(())
    }
}

pub fn write_outputs(root: &Path, outputs: &BTreeMap<String, Vec<u8>>) -> Result<Files> {
    for (name, bytes) in outputs {
        inventory::relative(Path::new(name))?;
        let path = root.join(name);
        fs::create_dir_all(path.parent().context("Output parent")?)?;
        fs::write(path, bytes)?;
    }
    inventory::files(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public_fixture() -> Result<(tempfile::TempDir, Config)> {
        let dir = tempfile::tempdir()?;
        let web = dir.path();
        fs::create_dir(web.join("src"))?;
        fs::create_dir(web.join("pkg"))?;
        fs::create_dir_all(web.join("public/legacy"))?;
        fs::write(
            web.join("index.html"),
            "<html><head></head><body><script type=\"module\" src=\"/src/main.js\"></script></body></html>",
        )?;
        fs::write(
            web.join("src/main.js"),
            "import './public.css';import init from '../pkg/app.js';await init();",
        )?;
        fs::write(
            web.join("src/public.css"),
            include_str!("../tests/fixtures/public-urls.css"),
        )?;
        fs::write(
            web.join("pkg/app.js"),
            "export default function init(){return new URL('./app_bg.wasm',import.meta.url)}",
        )?;
        fs::write(web.join("pkg/app_bg.wasm"), b"\0asm\x01\0\0\0")?;
        for name in [
            "bannerWave1.png",
            "bannerWave2.png",
            "backgroundPicture.jpg",
        ] {
            fs::write(web.join("public/legacy").join(name), name)?;
        }
        let mut config = Config::default();
        config.web.entry = "src/main.js".into();
        config.web.licenses.clear();
        Ok((dir, config))
    }

    #[test]
    fn spa_relative_resources_mount_under_each_instance() -> Result<()> {
        let (dir, mut config) = public_fixture()?;
        config.web.relative_urls = true;
        let web = dir.path();
        let (outputs, _) = Linker::new(web, &web.join("pkg"), config).finish(b"{}")?;
        let html = String::from_utf8(outputs["index.html"].clone())?;
        assert!(html.contains("src=\"./assets/"));
        assert!(html.contains("href=\"./assets/"));
        assert!(!html.contains("src=\"/assets/"));
        assert!(!html.contains("href=\"/assets/"));
        let module = outputs
            .iter()
            .find(|(name, _)| name.starts_with("assets/main-") && name.ends_with(".js"))
            .context("linked entry")?;
        let references = crate::javascript::references(std::str::from_utf8(module.1)?)?;
        assert!(!references.imports.is_empty());
        assert!(
            references
                .imports
                .iter()
                .all(|import| import.url.starts_with("./"))
        );
        Ok(())
    }

    fn public_manifest(web: &Path, config: &mut Config) -> Result<()> {
        config.web.public_assets_manifest = "public-assets.json".into();
        fs::write(
            web.join(&config.web.public_assets_manifest),
            serde_json::to_vec(&json!({
                "format": 1,
                "files": inventory::files(&web.join("public"))?,
            }))?,
        )?;
        Ok(())
    }

    fn svg_bytes(length: usize) -> Vec<u8> {
        let prefix = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><!--";
        let suffix = b"--></svg>";
        assert!(length >= prefix.len() + suffix.len());
        let mut bytes = vec![b'x'; length];
        bytes[..prefix.len()].copy_from_slice(prefix);
        bytes[length - suffix.len()..].copy_from_slice(suffix);
        bytes
    }

    #[test]
    fn public_manifest_requires_exact_media_and_nonmedia_source_bytes() -> Result<()> {
        let (dir, mut config) = public_fixture()?;
        let web = dir.path();
        fs::write(
            web.join("public/legacy/README.md"),
            "Reviewed media sources",
        )?;
        public_manifest(web, &mut config)?;
        Linker::new(web, &web.join("pkg"), config.clone()).finish(b"{}")?;
        let original = fs::read(web.join("public/legacy/bannerWave1.png"))?;
        fs::write(web.join("public/legacy/bannerWave1.png"), "changed bytes")?;
        assert!(
            Linker::new(web, &web.join("pkg"), config.clone())
                .finish(b"{}")
                .unwrap_err()
                .to_string()
                .contains("Public assets differ from source inventory")
        );
        fs::write(web.join("public/legacy/bannerWave1.png"), original)?;
        fs::write(web.join("public/added.svg"), "<svg/>")?;
        assert!(
            Linker::new(web, &web.join("pkg"), config.clone())
                .finish(b"{}")
                .is_err()
        );
        fs::remove_file(web.join("public/added.svg"))?;
        fs::remove_file(web.join("public/legacy/README.md"))?;
        assert!(
            Linker::new(web, &web.join("pkg"), config.clone())
                .finish(b"{}")
                .is_err()
        );
        fs::write(
            web.join("public/legacy/README.md"),
            "Reviewed media sources",
        )?;
        let mut files = inventory::files(&web.join("public"))?;
        files.remove("legacy/README.md");
        fs::write(
            web.join("public-assets.json"),
            serde_json::to_vec(&json!({"format":1,"files":files}))?,
        )?;
        assert!(
            Linker::new(web, &web.join("pkg"), config)
                .finish(b"{}")
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn registered_public_media_have_independent_budgets_and_exact_receipts() -> Result<()> {
        let (dir, mut config) = public_fixture()?;
        let web = dir.path();
        let image = svg_bytes(512 * 1024 + 1);
        fs::write(web.join("public/hero.svg"), &image)?;
        fs::write(
            web.join("public/clip.mp4"),
            b"\0\0\0\x18ftypisom\0\0\0\0isommp42",
        )?;
        fs::write(web.join("public/sound.mp3"), b"ID3\x04\0\0\0\0\0\0")?;
        public_manifest(web, &mut config)?;
        config.web.max_total_bytes = 4096;
        let mut linker = Linker::new(web, &web.join("pkg"), config.clone());
        linker.copy_public()?;
        assert!(linker.public_media.contains("clip.mp4"));
        assert!(linker.public_media.contains("sound.mp3"));
        assert!(linker.public_media.contains("legacy/bannerWave1.png"));
        assert!(linker.public_media.contains("legacy/backgroundPicture.jpg"));
        let (outputs, receipt) = Linker::new(web, &web.join("pkg"), config).finish(b"{}")?;
        assert_eq!(outputs["hero.svg"], image);
        assert_eq!(outputs.values().filter(|bytes| *bytes == &image).count(), 1);
        let delivered = receipt["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["path"] == "hero.svg")
            .collect::<Vec<_>>();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0]["sha256"], inventory::sha(&image));
        Ok(())
    }

    #[test]
    fn registered_public_media_enforce_four_mib_files_and_eight_mib_total() -> Result<()> {
        let (dir, mut config) = public_fixture()?;
        let web = dir.path();
        fs::write(
            web.join("public/hero.svg"),
            svg_bytes(MAX_PUBLIC_MEDIA_BYTES + 1),
        )?;
        public_manifest(web, &mut config)?;
        let mut linker = Linker::new(web, &web.join("pkg"), config.clone());
        linker.copy_public()?;
        assert!(
            linker
                .budgets()
                .unwrap_err()
                .to_string()
                .contains("Public media hero.svg exceeds 4194304-byte budget")
        );
        fs::write(
            web.join("public/hero.svg"),
            svg_bytes(MAX_PUBLIC_MEDIA_BYTES),
        )?;
        fs::write(
            web.join("public/second.svg"),
            svg_bytes(MAX_PUBLIC_MEDIA_BYTES),
        )?;
        public_manifest(web, &mut config)?;
        let mut linker = Linker::new(web, &web.join("pkg"), config);
        linker.copy_public()?;
        assert!(
            linker
                .budgets()
                .unwrap_err()
                .to_string()
                .contains("Public media exceed 8388608-byte total budget")
        );
        Ok(())
    }

    #[test]
    fn registered_javascript_and_unregistered_images_keep_original_asset_limits() -> Result<()> {
        let (dir, mut config) = public_fixture()?;
        let web = dir.path();
        fs::write(web.join("public/app.js"), vec![b' '; 512 * 1024 + 1])?;
        public_manifest(web, &mut config)?;
        let error = Linker::new(web, &web.join("pkg"), config.clone())
            .finish(b"{}")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Asset app.js exceeds 524288-byte budget")
        );
        fs::remove_file(web.join("public/app.js"))?;
        fs::write(web.join("public/hero.svg"), svg_bytes(512 * 1024 + 1))?;
        config.web.public_assets_manifest.clear();
        let error = Linker::new(web, &web.join("pkg"), config.clone())
            .finish(b"{}")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Asset hero.svg exceeds 524288-byte budget")
        );
        Ok(())
    }

    #[test]
    fn media_inventory_does_not_relax_bundle_total_or_reclassify_text_and_fonts() -> Result<()> {
        let (dir, mut config) = public_fixture()?;
        let web = dir.path();
        fs::write(web.join("public/hero.svg"), svg_bytes(512 * 1024 + 1))?;
        for name in ["README.md", "app.js", "app.css", "font.woff2"] {
            fs::write(web.join("public").join(name), "bundle")?;
        }
        public_manifest(web, &mut config)?;
        config.web.max_total_bytes = 1;
        let mut linker = Linker::new(web, &web.join("pkg"), config.clone());
        linker.copy_public()?;
        for name in ["README.md", "app.js", "app.css", "font.woff2"] {
            assert!(!linker.public_media.contains(name));
        }
        let error = Linker::new(web, &web.join("pkg"), config.clone())
            .finish(b"{}")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Web bundle exceeds 1-byte total budget")
        );
        config.web.max_total_bytes = 16 * 1024 * 1024;
        fs::write(web.join("public/font.woff2"), vec![0; 16 * 1024 * 1024])?;
        public_manifest(web, &mut config)?;
        let error = Linker::new(web, &web.join("pkg"), config)
            .finish(b"{}")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Web bundle exceeds 16777216-byte total budget")
        );
        Ok(())
    }

    #[test]
    fn public_manifest_schema_and_location_cannot_hide_ambiguous_or_untracked_inputs() -> Result<()>
    {
        let (dir, mut config) = public_fixture()?;
        let web = dir.path();
        config.web.public_assets_manifest = "public-assets.json".into();
        for invalid in [
            r#"{"format":2,"files":{}}"#,
            r#"{"format":1,"files":{},"ignored":true}"#,
            r#"{"format":1,"files":{"a":"x","a":"y"}}"#,
            r#"{"format":1,"files":{"../outside.svg":"0000000000000000000000000000000000000000000000000000000000000000"}}"#,
        ] {
            fs::write(web.join("public-assets.json"), invalid)?;
            assert!(
                Linker::new(web, &web.join("pkg"), config.clone())
                    .finish(b"{}")
                    .is_err(),
                "accepted {invalid}"
            );
        }
        for invalid in [
            "../public-assets.json",
            "src/public-assets.json",
            "/tmp/public-assets.json",
        ] {
            config.web.public_assets_manifest = invalid.into();
            assert!(
                Linker::new(web, &web.join("pkg"), config.clone())
                    .finish(b"{}")
                    .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn public_css_urls_keep_fixed_paths_without_duplicate_hashed_assets() -> Result<()> {
        let (dir, config) = public_fixture()?;
        let web = dir.path();
        let provenance = serde_json::to_vec(&inventory::files(&web.join("public"))?)?;
        let (outputs, receipt) = Linker::new(web, &web.join("pkg"), config).finish(&provenance)?;
        let css = outputs
            .iter()
            .find(|(name, _)| name.ends_with(".css"))
            .unwrap()
            .1;
        let css = String::from_utf8_lossy(css);
        for name in [
            "bannerWave1.png",
            "bannerWave2.png",
            "backgroundPicture.jpg",
        ] {
            let output = format!("legacy/{name}");
            assert!(css.contains(&format!("/{output}")));
            assert_eq!(
                outputs[&output],
                fs::read(web.join("public").join(&output))?
            );
            assert_eq!(
                outputs
                    .values()
                    .filter(|bytes| bytes.as_slice() == name.as_bytes())
                    .count(),
                1
            );
            let delivered = receipt["outputs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["path"] == output)
                .collect::<Vec<_>>();
            assert_eq!(delivered.len(), 1);
            assert_eq!(delivered[0]["sha256"], inventory::sha(&outputs[&output]));
        }
        assert_eq!(outputs["assets/xcss-web-build.json"], provenance);
        Ok(())
    }

    #[test]
    fn absolute_imports_prefer_web_files_and_public_fallback_is_bounded() -> Result<()> {
        let (dir, config) = public_fixture()?;
        let web = dir.path();
        fs::write(web.join("priority.svg"), "<svg>web</svg>")?;
        fs::write(web.join("public/priority.svg"), "<svg>public</svg>")?;
        let parent = web.join("src/main.js");
        let linker = Linker::new(web, &web.join("pkg"), config.clone());
        assert_eq!(
            linker.resolve(&parent, "/priority.svg?url")?,
            web.join("priority.svg")
        );
        assert_eq!(
            linker.resolve_with_public_priority(&parent, "/priority.svg", true)?,
            web.join("public/priority.svg")
        );
        assert_eq!(
            linker.resolve(&parent, "/legacy/bannerWave1.png?url")?,
            web.join("public/legacy/bannerWave1.png")
        );
        for url in [
            "/../outside.svg",
            "/legacy/../../outside.svg",
            "//example.org/image.svg",
        ] {
            assert!(linker.resolve(&parent, url).is_err(), "accepted {url}");
        }
        fs::write(
            web.join("src/main.js"),
            "import picture from '/legacy/bannerWave1.png?url';import init from '/pkg/app.js';await init();console.log(picture);",
        )?;
        let stage = tempfile::tempdir()?;
        let pkg = stage.path().join("pkg");
        fs::rename(web.join("pkg"), &pkg)?;
        let (outputs, _) = Linker::new(web, &pkg, config).finish(b"{}")?;
        assert!(
            outputs
                .iter()
                .any(|(name, bytes)| name.starts_with("assets/main-")
                    && String::from_utf8_lossy(bytes).contains("/legacy/bannerWave1.png"))
        );
        assert!(
            !outputs
                .keys()
                .any(|name| name.starts_with("assets/bannerWave1-"))
        );
        Ok(())
    }

    #[test]
    fn public_assets_remain_budgeted_and_cannot_shadow_provenance() -> Result<()> {
        let (dir, config) = public_fixture()?;
        let web = dir.path();
        fs::write(
            web.join("public/oversized.bin"),
            vec![0; config.web.max_asset_bytes + 1],
        )?;
        let error = Linker::new(web, &web.join("pkg"), config.clone())
            .finish(b"{}")
            .unwrap_err();
        assert!(error.to_string().contains("Asset oversized.bin exceeds"));
        fs::remove_file(web.join("public/oversized.bin"))?;
        fs::create_dir(web.join("public/assets"))?;
        fs::write(web.join("public/assets/xcss-web-build.json"), "{}")?;
        let error = Linker::new(web, &web.join("pkg"), config)
            .finish(b"{}")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Public asset shadows a generated resource: assets/xcss-web-build.json")
        );
        Ok(())
    }

    #[test]
    fn embedded_public_urls_are_relative_to_the_css_and_module_outputs() -> Result<()> {
        let (dir, mut config) = public_fixture()?;
        let web = dir.path();
        config.web.kind = "embedded".into();
        config.web.relative_urls = true;
        config.web.css_output = "dist/platform.css".into();
        config
            .web
            .entries
            .insert("index.js".into(), "src/main.js".into());
        fs::write(
            web.join("src/main.js"),
            "import './public.css';import picture from '/legacy/bannerWave1.png?url';import init from '../pkg/app.js';await init();console.log(picture);",
        )?;
        let (outputs, _) = Linker::new(web, &web.join("pkg"), config).finish(b"{}")?;
        let css = String::from_utf8_lossy(&outputs["dist/platform.css"]);
        assert!(css.contains("../legacy/bannerWave1.png"));
        assert!(
            outputs
                .iter()
                .any(|(name, bytes)| name.starts_with("assets/main-")
                    && String::from_utf8_lossy(bytes).contains("../legacy/bannerWave1.png"))
        );
        assert!(
            !outputs
                .keys()
                .any(|name| name.starts_with("assets/bannerWave1-"))
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn public_fallback_rejects_symlinks_instead_of_reading_outside() -> Result<()> {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir()?;
        fs::write(outside.path().join("outside.svg"), "<svg/>")?;
        let (dir, config) = public_fixture()?;
        let web = dir.path();
        symlink(
            outside.path().join("outside.svg"),
            web.join("public/escape.svg"),
        )?;
        let linker = Linker::new(web, &web.join("pkg"), config);
        let error = linker
            .resolve(&web.join("src/main.js"), "/escape.svg")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Symlink input/output is forbidden")
        );
        Ok(())
    }

    #[test]
    fn esm_css_and_wasm_urls_are_hashed_with_matching_preload() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let web = dir.path();
        fs::create_dir(web.join("src"))?;
        fs::create_dir(web.join("pkg"))?;
        fs::write(
            web.join("index.html"),
            "<html><head></head><body><script type=\"module\" src=\"/src/main.js\"></script></body></html>",
        )?;
        fs::write(
            web.join("src/main.js"),
            "import './style.css';\nimport init from '../pkg/app.js';\nawait init();",
        )?;
        fs::write(web.join("src/style.css"), "body{background:url('./a.svg')}")?;
        fs::write(web.join("src/a.svg"), "<svg/>")?;
        fs::write(
            web.join("pkg/app.js"),
            "export default function init(){return new URL('app_bg.wasm',import.meta.url)}",
        )?;
        fs::write(web.join("pkg/app_bg.wasm"), b"\0asm\x01\0\0\0")?;
        let mut config = Config::default();
        config.web.entry = "src/main.js".into();
        config.web.licenses.clear();
        let (outputs, receipt) = Linker::new(web, &web.join("pkg"), config).finish(b"{}")?;
        let html = String::from_utf8(outputs["index.html"].clone())?;
        let wasm = outputs.keys().find(|v| v.ends_with(".wasm")).unwrap();
        assert!(html.contains(wasm));
        assert!(outputs.contains_key(&format!("{wasm}.gz")));
        assert_eq!(receipt["modules"].as_array().unwrap().len(), 2);
        let all_js = outputs
            .iter()
            .filter(|(n, _)| n.ends_with(".js"))
            .map(|(_, b)| String::from_utf8_lossy(b))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all_js.contains(wasm));
        assert!(!all_js.contains("../pkg/"));
        assert!(!all_js.contains("./style.css"));
        Ok(())
    }
    #[test]
    fn budgets_cannot_be_relaxed_and_bare_imports_fail() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(
            dir.path().join("rust-web.toml"),
            "[web]\nmax_asset_bytes=1048576",
        )?;
        assert!(Config::read(dir.path()).is_err());
        fs::write(
            dir.path().join("main.js"),
            "import x from 'unreviewed-package';",
        )?;
        let mut linker = Linker::new(dir.path(), dir.path(), Config::default());
        assert!(linker.module(&dir.path().join("main.js")).is_err());
        Ok(())
    }

    #[test]
    fn embedded_entries_keep_templates_fixed_fonts_and_same_origin_svg() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let web = dir.path().join("web");
        let pkg = dir.path().join("staged-pkg");
        fs::create_dir_all(web.join("vendor/test"))?;
        fs::create_dir_all(&pkg)?;
        fs::write(
            web.join("index.html"),
            "__ASSETS_PREFIX__ __INDEX_DATA__ __WASM_PRELOAD__",
        )?;
        fs::write(
            web.join("index.js"),
            "import './platform.js';import init from './pkg/app.js';await init();",
        )?;
        fs::write(
            web.join("platform.js"),
            "import './vendor/test/font.css';export const bridge=1;",
        )?;
        fs::write(
            web.join("vendor/test/font.css"),
            "@font-face{src:url('./font.woff2')}a{mask-image:url(\"data:image/svg+xml,%3Csvg/%3E\")}",
        )?;
        fs::write(web.join("vendor/test/font.woff2"), "font")?;
        fs::write(
            pkg.join("app.js"),
            "export default function init(){return new URL('./app_bg.wasm',import.meta.url)}",
        )?;
        fs::write(pkg.join("app_bg.wasm"), b"\0asm\x01\0\0\0")?;
        let mut config = Config::default();
        config.web.kind = "embedded".into();
        config.web.relative_urls = true;
        config.web.runtime_root = "product".into();
        config.web.licenses.clear();
        config.web.css_output = "dist/platform.css".into();
        config
            .web
            .entries
            .insert("index.js".into(), "index.js".into());
        config
            .web
            .entries
            .insert("dist/platform.js".into(), "platform.js".into());
        config
            .web
            .copies
            .insert("dist/font.woff2".into(), "vendor/test/font.woff2".into());
        let (outputs, receipt) = Linker::new(&web, &pkg, config).finish(b"{}")?;
        assert!(!outputs.contains_key("index.html"));
        assert!(outputs.contains_key("index.js"));
        assert!(outputs.contains_key("dist/platform.js"));
        assert_eq!(
            outputs
                .keys()
                .filter(|name| name.ends_with(".woff2"))
                .count(),
            1
        );
        let css = String::from_utf8(outputs["dist/platform.css"].clone())?;
        assert!(!css.contains("data:"));
        assert!(css.contains("../assets/xcss-icon-"));
        assert!(css.contains("../dist/font.woff2"));
        assert!(
            receipt["modules"]
                .as_array()
                .unwrap()
                .iter()
                .all(|module| module["path"].as_str().unwrap().starts_with("web/"))
        );
        assert_eq!(
            fs::read_to_string(web.join("index.html"))?,
            "__ASSETS_PREFIX__ __INDEX_DATA__ __WASM_PRELOAD__"
        );
        Ok(())
    }
}
