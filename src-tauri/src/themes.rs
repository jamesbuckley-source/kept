//! Named palettes: the ones bundled with Kept and the ones imported from a file.
//!
//! Dark and light are not here — they are the two modes the interface already
//! has. Everything else is a preset: a whole palette chosen at once, plus the
//! colour the capture overlays draw with. Imported presets are kept as JSON files
//! under the app data folder, not in the (movable) data folder, so a theme
//! survives a data folder change and can be shared by copying one file.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AppError, Result};
use crate::settings::ThemeColors;

/// A theme file is a few hundred bytes; these limits keep a hostile archive from
/// being interesting.
const MAX_JSON_BYTES: u64 = 32 * 1024;
const MAX_ENTRIES: usize = 64;
const MAX_NAME_CHARS: usize = 60;
const MAX_ID_CHARS: usize = 60;
const THEME_FILE: &str = "theme.json";

/// One palette as the picker needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemePreset {
    pub id: String,
    pub name: String,
    pub colors: ThemeColors,
    /// Colour of the capture overlays; the global setting when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_outline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Bundled presets can be applied but not removed.
    pub builtin: bool,
}

/// What a theme file holds.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThemeDoc {
    name: String,
    colors: ThemeColors,
    #[serde(default)]
    capture_outline: Option<String>,
    #[serde(default)]
    author: Option<String>,
}

/// The result of reading a theme file, as the picker needs it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeImport {
    pub theme: ThemePreset,
    /// True when an import landed on a name that already existed.
    pub replaced: bool,
}

/// The palettes Kept ships with.
///
/// Each one is answerable to the contrast the interface needs: body text on the
/// panel colour, dim text on the canvas, and the accent on the panel.
pub fn builtins() -> Vec<ThemePreset> {
    let preset = |id: &str, name: &str, colors: ThemeColors, outline: &str| ThemePreset {
        id: id.to_string(),
        name: name.to_string(),
        colors,
        capture_outline: Some(outline.to_string()),
        author: None,
        builtin: true,
    };

    vec![
        preset(
            "midnight",
            "Midnight",
            ThemeColors {
                canvas: "#060913".into(),
                panel: "#0c1424".into(),
                panel2: "#121d33".into(),
                panel3: "#1a2742".into(),
                edge: "#22304d".into(),
                edge_soft: "#17233a".into(),
                ink: "#e6edfa".into(),
                ink_dim: "#a9b8d4".into(),
                ink_faint: "#71829f".into(),
                accent: "#4f8cff".into(),
                accent_hi: "#85b1ff".into(),
                danger: "#ff6b6b".into(),
            },
            "#4f8cff",
        ),
        preset(
            "slate",
            "Slate",
            ThemeColors {
                canvas: "#171a1d".into(),
                panel: "#1f2429".into(),
                panel2: "#262c33".into(),
                panel3: "#2f3740".into(),
                edge: "#3a434d".into(),
                edge_soft: "#2a3138".into(),
                ink: "#e8ecef".into(),
                ink_dim: "#b2bcc4".into(),
                ink_faint: "#8b959e".into(),
                accent: "#4fc3bb".into(),
                accent_hi: "#7fd8d2".into(),
                danger: "#ff7a7a".into(),
            },
            "#4fc3bb",
        ),
        preset(
            "sandstone",
            "Sandstone",
            ThemeColors {
                canvas: "#f4efe7".into(),
                panel: "#fffdf9".into(),
                panel2: "#f7f1e7".into(),
                panel3: "#eee6d8".into(),
                edge: "#ddd2c0".into(),
                edge_soft: "#e8e0d2".into(),
                ink: "#2b2418".into(),
                ink_dim: "#6b5f4c".into(),
                ink_faint: "#8a7c63".into(),
                accent: "#a75f24".into(),
                accent_hi: "#84491a".into(),
                danger: "#b3261e".into(),
            },
            "#a75f24",
        ),
        preset(
            "forest",
            "Forest",
            ThemeColors {
                canvas: "#0a1310".into(),
                panel: "#101d18".into(),
                panel2: "#16271f".into(),
                panel3: "#1d3228".into(),
                edge: "#26412f".into(),
                edge_soft: "#1a2c23".into(),
                ink: "#e3f2e8".into(),
                ink_dim: "#a8c6b4".into(),
                ink_faint: "#789a86".into(),
                accent: "#3fbf7f".into(),
                accent_hi: "#6cd9a2".into(),
                danger: "#ff6b6b".into(),
            },
            "#3fbf7f",
        ),
        preset(
            "mono",
            "Mono",
            ThemeColors {
                canvas: "#0a0a0a".into(),
                panel: "#141414".into(),
                panel2: "#1c1c1c".into(),
                panel3: "#262626".into(),
                edge: "#333333".into(),
                edge_soft: "#202020".into(),
                ink: "#f2f2f2".into(),
                ink_dim: "#b8b8b8".into(),
                ink_faint: "#8a8a8a".into(),
                accent: "#d8d8d8".into(),
                accent_hi: "#ffffff".into(),
                danger: "#ff5c5c".into(),
            },
            "#d8d8d8",
        ),
        preset(
            "high-contrast",
            "High contrast",
            ThemeColors {
                canvas: "#000000".into(),
                panel: "#0a0a0a".into(),
                panel2: "#141414".into(),
                panel3: "#1e1e1e".into(),
                edge: "#6b6b6b".into(),
                edge_soft: "#3a3a3a".into(),
                ink: "#ffffff".into(),
                ink_dim: "#e0e0e0".into(),
                ink_faint: "#b0b0b0".into(),
                accent: "#ffd400".into(),
                accent_hi: "#ffe95c".into(),
                danger: "#ff5252".into(),
            },
            "#ffd400",
        ),
    ]
}

/// A bundled preset by id, used by the tests that check a preset applies.
#[cfg(test)]
pub fn builtin(id: &str) -> Option<ThemePreset> {
    builtins().into_iter().find(|preset| preset.id == id)
}

/// Bundled presets followed by the imported ones, in the order they are shown.
pub fn list(dir: &Path) -> Vec<ThemePreset> {
    let mut presets = builtins();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return presets;
    };
    let mut imported: Vec<ThemePreset> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let path = entry.path();
            match read_theme_file(&path) {
                Ok(doc) => {
                    let id = path.file_stem()?.to_string_lossy().to_string();
                    Some(preset_from_doc(id, doc, false))
                }
                Err(e) => {
                    // One broken file must not hide the rest of the list.
                    log::warn!("skipping {}: {e}", path.display());
                    None
                }
            }
        })
        .collect();
    imported.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    presets.extend(imported);
    presets
}

/// Reads a theme from a `.zip` (a `theme.json` inside it) or a bare `.json`.
///
/// Returns the preset and whether an existing theme of the same id was replaced.
pub fn import(dir: &Path, source: &Path) -> Result<(ThemePreset, bool)> {
    let bytes = std::fs::read(source)
        .map_err(|e| AppError::msg(format!("could not read {}: {e}", source.display())))?;
    let json = if source
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        if bytes.len() as u64 > MAX_JSON_BYTES {
            return Err(AppError::msg("that theme file is too large to be a theme"));
        }
        String::from_utf8(bytes)
            .map_err(|_| AppError::msg("that theme file is not readable text"))?
    } else {
        extract_theme_json(&bytes)?
    };
    let doc = parse_theme(&json)?;
    let id = unique_id(dir, &doc.name);
    let replaced = dir.join(format!("{id}.json")).exists();
    save(dir, &id, &json)?;
    Ok((preset_from_doc(id, doc, false), replaced))
}

/// Deletes an imported theme. Bundled ones cannot be deleted.
pub fn remove(dir: &Path, id: &str) -> Result<()> {
    if builtins().iter().any(|preset| preset.id == id) {
        return Err(AppError::msg("that theme is built in, so it cannot be removed"));
    }
    if !is_plain_id(id) {
        return Err(AppError::msg("that is not a theme Kept knows about"));
    }
    let path = dir.join(format!("{id}.json"));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(AppError::msg("that theme is already gone"))
        }
        Err(e) => Err(AppError::msg(format!("could not remove the theme: {e}"))),
    }
}

fn preset_from_doc(id: String, doc: ThemeDoc, builtin: bool) -> ThemePreset {
    ThemePreset {
        id,
        name: doc.name,
        colors: doc.colors.sanitized(),
        capture_outline: doc.capture_outline,
        author: doc.author,
        builtin,
    }
}

/// The `theme.json` inside an archive.
///
/// Nothing is extracted: the one file is read into memory and everything else in
/// the archive is ignored, so a stray path in an entry name never reaches the
/// filesystem.
fn extract_theme_json(bytes: &[u8]) -> Result<String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| AppError::msg(format!("that file is not a readable zip: {e}")))?;
    if archive.len() > MAX_ENTRIES {
        return Err(AppError::msg(format!(
            "that archive holds {} files, which is more than a theme needs",
            archive.len()
        )));
    }
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| AppError::msg(format!("could not read the archive: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().unwrap_or_default().to_string();
        if !name.eq_ignore_ascii_case(THEME_FILE)
            && !name
                .rsplit(['/', '\\'])
                .next()
                .is_some_and(|leaf| leaf.eq_ignore_ascii_case(THEME_FILE))
        {
            continue;
        }
        if entry.size() > MAX_JSON_BYTES {
            return Err(AppError::msg("the theme inside that archive is too large"));
        }
        let mut json = String::new();
        entry
            .take(MAX_JSON_BYTES)
            .read_to_string(&mut json)
            .map_err(|e| AppError::msg(format!("could not read {THEME_FILE}: {e}")))?;
        return Ok(json);
    }
    Err(AppError::msg(format!(
        "that archive has no {THEME_FILE} in it, so it is not a theme"
    )))
}

/// Checks a theme file before anything is written.
///
/// Every colour is required: filling the gaps silently would hand back a theme
/// that looks nothing like the one that was shared.
fn parse_theme(json: &str) -> Result<ThemeDoc> {
    let value: Value = serde_json::from_str(json)
        .map_err(|e| AppError::msg(format!("that theme is not valid JSON: {e}")))?;
    let name = value["name"]
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| AppError::msg("that theme has no name"))?;
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::msg(format!(
            "that theme's name is longer than {MAX_NAME_CHARS} characters"
        )));
    }
    let colors = value
        .get("colors")
        .filter(|colors| colors.is_object())
        .ok_or_else(|| AppError::msg("that theme has no colors"))?;
    for slot in [
        "canvas", "panel", "panel2", "panel3", "edge", "edgeSoft", "ink", "inkDim", "inkFaint",
        "accent", "accentHi", "danger",
    ] {
        let hex = colors[slot].as_str().unwrap_or_default();
        if !is_hex_color(hex) {
            return Err(AppError::msg(format!(
                "the {slot} color is not a hex color like #1a2b3c"
            )));
        }
    }
    if let Some(outline) = value.get("captureOutline").and_then(Value::as_str)
        && !is_hex_color(outline)
    {
        return Err(AppError::msg(
            "the captureOutline color is not a hex color like #1a2b3c",
        ));
    }
    serde_json::from_value(value)
        .map_err(|e| AppError::msg(format!("that theme could not be read: {e}")))
}

fn is_hex_color(text: &str) -> bool {
    let hex = text.trim().strip_prefix('#').unwrap_or(text.trim());
    matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
}

/// The id a theme is filed under: its name as a slug, kept clear of the bundled
/// ids and of any theme already imported under the same slug.
fn unique_id(dir: &Path, name: &str) -> String {
    let mut slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-').to_string();
    let base = if !is_plain_id(&slug) {
        "imported-theme".to_string()
    } else if builtins().iter().any(|preset| preset.id == slug) {
        // A shared "Midnight" must not shadow the bundled one.
        format!("{slug}-custom")
    } else {
        slug
    };
    if !dir.join(format!("{base}.json")).exists() {
        return base;
    }
    for suffix in 2..100 {
        let candidate = format!("{base}-{suffix}");
        if !dir.join(format!("{candidate}.json")).exists() {
            return candidate;
        }
    }
    format!("{base}-{}", std::process::id())
}

/// Writes the file atomically: a crash mid-write must not leave half a theme.
fn save(dir: &Path, id: &str, json: &str) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::msg(format!("could not prepare {}: {e}", dir.display())))?;
    let path = dir.join(format!("{id}.json"));
    let staging = path.with_extension("json.part");
    std::fs::write(&staging, json)
        .map_err(|e| AppError::msg(format!("could not save the theme: {e}")))?;
    std::fs::rename(&staging, &path)
        .map_err(|e| AppError::msg(format!("could not finish saving the theme: {e}")))
}

fn read_theme_file(path: &PathBuf) -> Result<ThemeDoc> {
    let bytes = std::fs::read(path)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(AppError::msg("the file is too large to be a theme"));
    }
    let json = String::from_utf8(bytes)
        .map_err(|_| AppError::msg("the file is not readable text"))?;
    parse_theme(&json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kept-themes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn theme_json(name: &str) -> String {
        serde_json::json!({
            "name": name,
            "colors": {
                "canvas": "#101010",
                "panel": "#181818",
                "panel2": "#202020",
                "panel3": "#282828",
                "edge": "#303030",
                "edgeSoft": "#242424",
                "ink": "#f0f0f0",
                "inkDim": "#c0c0c0",
                "inkFaint": "#909090",
                "accent": "#3399ff",
                "accentHi": "#66bbff",
                "danger": "#ff5555",
            },
            "captureOutline": "#3399ff",
            "author": "someone",
        })
        .to_string()
    }

    fn zip_with(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options: zip::write::FileOptions<'_, '_, ()> = zip::write::SimpleFileOptions::default();
            for (name, body) in entries {
                use std::io::Write as _;
                writer.start_file(*name, options).expect("entry");
                writer.write_all(body.as_bytes()).expect("write");
            }
            writer.finish().expect("finish");
        }
        buffer.into_inner()
    }

    #[test]
    fn bundled_presets_have_valid_colors_and_outlines() {
        let presets = builtins();
        assert!(presets.len() >= 6);
        for preset in presets {
            assert!(preset.builtin);
            for hex in [
                &preset.colors.canvas,
                &preset.colors.panel,
                &preset.colors.ink,
                &preset.colors.accent,
            ] {
                assert!(is_hex_color(hex), "{} has {hex}", preset.id);
            }
            assert!(!preset.name.is_empty());
            assert!(preset.capture_outline.is_some());
        }
    }

    #[test]
    fn a_zip_with_one_theme_is_imported_and_listed() {
        let dir = temp_dir();
        let archive = zip_with(&[("theme.json", &theme_json("Paper Cut"))]);
        let source = dir.join("paper.zip");
        std::fs::write(&source, &archive).unwrap();

        let (preset, replaced) = import(&dir, &source).expect("import");
        assert_eq!(preset.name, "Paper Cut");
        assert_eq!(preset.id, "paper-cut");
        assert!(!replaced);
        assert!(!preset.builtin);
        assert_eq!(preset.author.as_deref(), Some("someone"));
        assert_eq!(preset.colors.canvas, "#101010");

        let listed = list(&dir);
        assert!(listed.iter().any(|entry| entry.id == "paper-cut"));
        assert!(listed.iter().any(|entry| entry.id == "midnight"));
        assert_eq!(listed.last().map(|entry| entry.id.as_str()), Some("paper-cut"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_bare_json_file_is_imported_too() {
        let dir = temp_dir();
        // The picked file is staged outside the themes folder: a source sitting in
        // there would look like a theme already imported.
        let inbox = dir.join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let source = inbox.join("plain.json");
        std::fs::write(&source, theme_json("Plain")).unwrap();
        let (preset, _) = import(&dir, &source).expect("import");
        assert_eq!(preset.id, "plain");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_zip_without_a_theme_is_refused() {
        let dir = temp_dir();
        let archive = zip_with(&[("readme.txt", "hello")]);
        let source = dir.join("empty.zip");
        std::fs::write(&source, &archive).unwrap();
        let error = import(&dir, &source).expect_err("no theme inside");
        assert!(error.to_string().contains("no theme.json"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_theme_missing_a_color_is_refused_by_name() {
        let mut value: Value = serde_json::from_str(&theme_json("Half")).unwrap();
        value["colors"].as_object_mut().unwrap().remove("ink");
        let error = parse_theme(&value.to_string()).expect_err("no ink");
        assert!(error.to_string().contains("ink"), "got {error}");

        value["colors"]["ink"] = serde_json::json!("not-a-color");
        let error = parse_theme(&value.to_string()).expect_err("bad ink");
        assert!(error.to_string().contains("ink"), "got {error}");
    }

    #[test]
    fn a_theme_without_a_name_is_refused() {
        let value: Value = serde_json::from_str(&theme_json("  ")).unwrap();
        assert!(parse_theme(&value.to_string()).is_err());
    }

    #[test]
    fn a_theme_named_like_a_bundled_one_gets_its_own_id() {
        let dir = temp_dir();
        let source = dir.join("mine.json");
        std::fs::write(&source, theme_json("Midnight")).unwrap();
        let (preset, _) = import(&dir, &source).expect("import");
        assert_eq!(preset.id, "midnight-custom");
        // The bundled preset is untouched.
        assert_eq!(builtin("midnight").unwrap().name, "Midnight");
        assert!(list(&dir).iter().any(|entry| entry.id == "midnight" && entry.builtin));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn importing_the_same_name_twice_keeps_both() {
        let dir = temp_dir();
        let inbox = dir.join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let source = inbox.join("twice.json");
        std::fs::write(&source, theme_json("Twice")).unwrap();
        let (first, _) = import(&dir, &source).expect("first");
        let (second, _) = import(&dir, &source).expect("second");
        assert_eq!(first.id, "twice");
        assert_eq!(second.id, "twice-2");
        assert_eq!(list(&dir).iter().filter(|entry| entry.name == "Twice").count(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn removing_works_for_imports_and_is_refused_for_bundled_themes() {
        let dir = temp_dir();
        let source = dir.join("gone.json");
        std::fs::write(&source, theme_json("Gone Soon")).unwrap();
        let (preset, _) = import(&dir, &source).expect("import");
        remove(&dir, &preset.id).expect("remove");
        assert!(!list(&dir).iter().any(|entry| entry.id == preset.id));
        assert!(remove(&dir, "midnight").is_err());
        assert!(remove(&dir, "../secrets").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
