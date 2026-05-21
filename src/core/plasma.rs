//! KDE Plasma (X11) configuration helpers used by the proot setup pipeline.
//!
//! These functions render the small set of KDE/X11 config files we drop into
//! the Arch rootfs the first time Local Desktop boots, so a freshly-installed
//! Plasma session starts up readable and well-behaved on Android out of the
//! box. The logic here is deliberately host-target-clean (no `jni`, no Android
//! framework calls) so it can be unit-tested on the developer's machine
//! against a temporary directory; the Android-specific glue lives in
//! `android::proot::setup`.

use std::fs;
use std::path::Path;

#[derive(Debug)]
enum KvLine {
    Entry {
        key: String,
        value: String,
        prefix: String,
        delimiter: char,
    },
    Other(String),
}

fn parse_kv_lines(content: &str, delimiter: char) -> Vec<KvLine> {
    content
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
                return KvLine::Other(line.to_string());
            }
            if let Some((left, right)) = line.split_once(delimiter) {
                let key = left.trim().to_string();
                if key.is_empty() {
                    return KvLine::Other(line.to_string());
                }
                let prefix_len = line.len() - trimmed.len();
                let prefix = line[..prefix_len].to_string();
                let value = right.trim().to_string();
                KvLine::Entry {
                    key,
                    value,
                    prefix,
                    delimiter,
                }
            } else {
                KvLine::Other(line.to_string())
            }
        })
        .collect()
}

fn set_kv_value(lines: &mut Vec<KvLine>, key: &str, value: &str, delimiter: char) {
    let mut updated = false;
    for line in lines.iter_mut() {
        if let KvLine::Entry {
            key: entry_key,
            value: entry_value,
            ..
        } = line
        {
            if entry_key == key {
                *entry_value = value.to_string();
                updated = true;
            }
        }
    }
    if !updated {
        lines.push(KvLine::Entry {
            key: key.to_string(),
            value: value.to_string(),
            prefix: String::new(),
            delimiter,
        });
    }
}

fn render_kv_lines(lines: &[KvLine]) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        match line {
            KvLine::Entry {
                key,
                value,
                prefix,
                delimiter,
            } => out.push(format!("{}{}{} {}", prefix, key, delimiter, value)),
            KvLine::Other(raw) => out.push(raw.to_string()),
        }
    }
    let mut content = out.join("\n");
    content.push('\n');
    content
}

/// Read `path` (or treat it as empty if missing), apply each `(key, value)`
/// `update` to the parsed key/value content separated by `delimiter`, and
/// write the result back. Existing entries are updated in place; missing
/// entries are appended at the end of the file.
pub(crate) fn upsert_kv_file(path: &Path, delimiter: char, updates: &[(&str, String)]) {
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut lines = parse_kv_lines(&content, delimiter);
    for (key, value) in updates {
        set_kv_value(&mut lines, key, value, delimiter);
    }
    let content = render_kv_lines(&lines);
    fs::write(path, content).expect("Failed to write key/value file");
}

/// Apply the supplied `(key, value)` `updates` inside the `[section]` of the
/// supplied INI-style `content`. Existing values are replaced, missing keys
/// are appended at the end of the section, and the section itself is
/// appended (with a leading blank line) when it isn't present yet.
pub(crate) fn update_ini_section(
    content: &str,
    section: &str,
    updates: &[(&str, String)],
) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut seen_section = false;
    let mut seen_keys = vec![false; updates.len()];

    for raw_line in content.lines() {
        let trimmed = raw_line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_section {
                for (idx, (key, value)) in updates.iter().enumerate() {
                    if !seen_keys[idx] {
                        out.push(format!("{}={}", key, value));
                    }
                }
            }
            let name = trimmed[1..trimmed.len() - 1].trim();
            in_section = name.eq_ignore_ascii_case(section);
            if in_section {
                seen_section = true;
            }
            out.push(raw_line.to_string());
            continue;
        }

        if in_section
            && !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && !trimmed.starts_with(';')
            && raw_line.contains('=')
        {
            if let Some((left, _)) = raw_line.split_once('=') {
                let key = left.trim();
                let mut replaced = false;
                for (idx, (target_key, value)) in updates.iter().enumerate() {
                    if key.eq_ignore_ascii_case(target_key) {
                        let indent: String =
                            raw_line.chars().take_while(|c| c.is_whitespace()).collect();
                        out.push(format!("{}{}={}", indent, key, value));
                        seen_keys[idx] = true;
                        replaced = true;
                        break;
                    }
                }
                if replaced {
                    continue;
                }
            }
        }

        out.push(raw_line.to_string());
    }

    if in_section {
        for (idx, (key, value)) in updates.iter().enumerate() {
            if !seen_keys[idx] {
                out.push(format!("{}={}", key, value));
            }
        }
    } else if !seen_section {
        if !out.is_empty() {
            out.push(String::new());
        }
        out.push(format!("[{}]", section));
        for (key, value) in updates {
            out.push(format!("{}={}", key, value));
        }
    }

    let mut content = out.join("\n");
    content.push('\n');
    content
}

/// Compute the integer DPI scale factor we apply for an Android display of the
/// supplied `density_dpi`. The `* 1.1` bias mirrors the heuristic used by the
/// previous LXQt scaling stage and gives slightly larger fonts than a strict
/// 1x mapping at low DPIs (which is desirable on small Android screens).
fn compute_scale(density_dpi: i32) -> i32 {
    ((density_dpi as f32) / 160.0 * 1.1).max(1.0).round() as i32
}

/// Render the small set of config files KDE Plasma needs for a usable first
/// launch on Android, all rooted at `fs_root` (typically the Arch FS root,
/// e.g. `/data/data/app.polarbear/files/arch`).
///
/// Specifically:
///   1. `~/.Xresources` — sets `Xft.dpi` so X11/XWayland-only apps pick up
///      HiDPI fonts even before `kdeglobals` is honoured.
///   2. `~/.config/kdeglobals` — sets `[General].forceFontDPI` which is
///      Plasma's authoritative DPI for the X11 session.
///   3. `~/.config/baloofilerc` — disables the Baloo file indexer; its
///      background scanning of `/sdcard`-style mounts wedges the touch UI
///      and burns battery on Android.
///   4. `~/.config/drkonqirc` — disables the KDE crash dialog so a single
///      component crash doesn't pop up an undismissable modal on touch-only
///      first launch. Crashes still flow into Sentry via our log filter.
pub fn apply_plasma_scaling(fs_root: &Path, density_dpi: i32) {
    let scale = compute_scale(density_dpi);
    let xft_dpi = scale * 96;

    // Make sure both `~/` and `~/.config/` exist before any writes; on a real
    // proot Arch rootfs they do, but exercising this code from the host
    // (tests, future tooling) starts from an empty fs_root.
    let _ = fs::create_dir_all(fs_root.join("root"));
    let _ = fs::create_dir_all(fs_root.join("root/.config"));

    // 1. Xft.dpi — X11 DPI hint
    let xresources_path = fs_root.join("root/.Xresources");
    upsert_kv_file(&xresources_path, ':', &[("Xft.dpi", xft_dpi.to_string())]);

    // 2. kdeglobals[General].forceFontDPI — Plasma's authoritative DPI
    let kdeglobals_path = fs_root.join("root/.config/kdeglobals");
    let kdeglobals_content = fs::read_to_string(&kdeglobals_path).unwrap_or_default();
    let kdeglobals_out = update_ini_section(
        &kdeglobals_content,
        "General",
        &[("forceFontDPI", xft_dpi.to_string())],
    );
    fs::write(&kdeglobals_path, kdeglobals_out).expect("Failed to write kdeglobals");

    // 3. baloofilerc[Basic Settings].Indexing-Enabled — disable file indexer
    let baloo_path = fs_root.join("root/.config/baloofilerc");
    let baloo_content = fs::read_to_string(&baloo_path).unwrap_or_default();
    let baloo_out = update_ini_section(
        &baloo_content,
        "Basic Settings",
        &[("Indexing-Enabled", "false".to_string())],
    );
    fs::write(&baloo_path, baloo_out).expect("Failed to write baloofilerc");

    // 4. drkonqirc[General].Enabled — disable interactive crash dialog
    let drkonqi_path = fs_root.join("root/.config/drkonqirc");
    let drkonqi_content = fs::read_to_string(&drkonqi_path).unwrap_or_default();
    let drkonqi_out = update_ini_section(
        &drkonqi_content,
        "General",
        &[("Enabled", "false".to_string())],
    );
    fs::write(&drkonqi_path, drkonqi_out).expect("Failed to write drkonqirc");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn upsert_kv_file_creates_file_with_appended_entry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("Xresources");
        upsert_kv_file(&path, ':', &[("Xft.dpi", "192".to_string())]);
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("Xft.dpi: 192"),
            "expected Xft.dpi entry, got: {content}"
        );
    }

    #[test]
    fn upsert_kv_file_replaces_existing_entry_in_place() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("Xresources");
        fs::write(&path, "! header comment\nXft.dpi: 96\nXft.antialias: true\n").unwrap();
        upsert_kv_file(&path, ':', &[("Xft.dpi", "192".to_string())]);
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("! header comment"),
            "comment must be preserved, got: {content}"
        );
        assert!(
            content.contains("Xft.dpi: 192"),
            "Xft.dpi must be updated, got: {content}"
        );
        assert!(
            content.contains("Xft.antialias: true"),
            "other entries must be preserved, got: {content}"
        );
        assert_eq!(
            content.matches("Xft.dpi").count(),
            1,
            "Xft.dpi must appear exactly once, got: {content}"
        );
    }

    #[test]
    fn update_ini_section_replaces_existing_key_and_appends_section_when_missing() {
        let starting = "[General]\nfontDpi=96\nname=KDE\n";
        let updated = update_ini_section(
            starting,
            "General",
            &[("fontDpi", "144".to_string()), ("forceFontDPI", "144".to_string())],
        );
        assert!(updated.contains("fontDpi=144"));
        assert!(updated.contains("forceFontDPI=144"));
        assert!(updated.contains("name=KDE"));
        // Append a brand-new section with both keys
        let updated2 = update_ini_section(
            &updated,
            "Basic Settings",
            &[("Indexing-Enabled", "false".to_string())],
        );
        assert!(updated2.contains("[Basic Settings]"));
        assert!(updated2.contains("Indexing-Enabled=false"));
    }

    #[test]
    fn compute_scale_matches_expected_density_brackets() {
        // 160dpi (mdpi)         → scale 1
        // 240dpi (hdpi)         → scale 2 (240/160*1.1 = 1.65 → round → 2)
        // 320dpi (xhdpi)        → scale 2 (320/160*1.1 = 2.20 → round → 2)
        // 480dpi (xxhdpi)       → scale 3
        // 640dpi (xxxhdpi)      → scale 4
        assert_eq!(compute_scale(160), 1);
        assert_eq!(compute_scale(240), 2);
        assert_eq!(compute_scale(320), 2);
        assert_eq!(compute_scale(480), 3);
        assert_eq!(compute_scale(640), 4);
        // Lower density still clamps to 1
        assert_eq!(compute_scale(120), 1);
    }

    #[test]
    fn apply_plasma_scaling_writes_all_expected_config_files() {
        let dir = tempdir().unwrap();
        let fs_root = dir.path();

        apply_plasma_scaling(fs_root, 480);

        let xresources = fs::read_to_string(fs_root.join("root/.Xresources")).unwrap();
        let kdeglobals = fs::read_to_string(fs_root.join("root/.config/kdeglobals")).unwrap();
        let baloo = fs::read_to_string(fs_root.join("root/.config/baloofilerc")).unwrap();
        let drkonqi = fs::read_to_string(fs_root.join("root/.config/drkonqirc")).unwrap();

        // 480dpi → scale 3 → forceFontDPI / Xft.dpi = 288
        assert!(
            xresources.contains("Xft.dpi: 288"),
            "Xft.dpi mismatch, got: {xresources}"
        );
        assert!(
            kdeglobals.contains("[General]") && kdeglobals.contains("forceFontDPI=288"),
            "kdeglobals mismatch, got: {kdeglobals}"
        );
        assert!(
            baloo.contains("[Basic Settings]") && baloo.contains("Indexing-Enabled=false"),
            "baloofilerc mismatch, got: {baloo}"
        );
        assert!(
            drkonqi.contains("[General]") && drkonqi.contains("Enabled=false"),
            "drkonqirc mismatch, got: {drkonqi}"
        );
    }

    #[test]
    fn apply_plasma_scaling_is_idempotent_and_preserves_user_keys() {
        let dir = tempdir().unwrap();
        let fs_root = dir.path();

        // Pre-seed kdeglobals with an unrelated user setting we must preserve.
        let kdeglobals_path = fs_root.join("root/.config/kdeglobals");
        fs::create_dir_all(kdeglobals_path.parent().unwrap()).unwrap();
        fs::write(
            &kdeglobals_path,
            "[General]\nfixed=Hack,10,-1,5,50,0,0,0,0,0\nforceFontDPI=72\n",
        )
        .unwrap();

        apply_plasma_scaling(fs_root, 320); // scale 2 → DPI 192
        apply_plasma_scaling(fs_root, 320); // running twice must not duplicate

        let kdeglobals = fs::read_to_string(&kdeglobals_path).unwrap();
        assert!(
            kdeglobals.contains("fixed=Hack,10,-1,5,50,0,0,0,0,0"),
            "user setting must survive, got: {kdeglobals}"
        );
        assert!(
            kdeglobals.contains("forceFontDPI=192"),
            "forceFontDPI must be set to scaled value, got: {kdeglobals}"
        );
        assert_eq!(
            kdeglobals.matches("forceFontDPI").count(),
            1,
            "forceFontDPI must appear exactly once after a second apply, got: {kdeglobals}"
        );
    }
}
