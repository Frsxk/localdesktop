use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(not(test))]
pub const ARCH_FS_ROOT: &str = "/data/data/app.polarbear/files/arch";
#[cfg(test)]
pub const ARCH_FS_ROOT: &str = "/data/local/tmp/arch";

pub const ARCH_FS_ARCHIVE: &str = "https://github.com/termux/proot-distro/releases/download/v4.29.0/archlinux-aarch64-pd-v4.29.0.tar.xz";

pub const WAYLAND_SOCKET_NAME: &str = "wayland-0";

pub const MAX_PANEL_LOG_ENTRIES: usize = 100;

pub const SENTRY_DSN: &str = "https://d8af27f864ade027ff81ecadea91b02e@o4509548388417536.ingest.de.sentry.io/4509548392480848";

/// Make sure the config keys are all lowercase, and config values are single-line. Use \n for multi-line config values if needed
/// If a key exists multiple time, the first entry is applied
/// If a `try_` config exsists multiple time, the last entry is applied
/// But in general, it is **invalid** to have duplicated config keys inside a TOML file
pub const CONFIG_FILE: &str = "/etc/localdesktop/localdesktop.toml";

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct LocalConfig {
    #[serde(default)]
    pub user: UserConfig,

    /// What happens if we don't assign this `#[serde(default)]` attribute?
    /// The answer: If the user omits the `[command]` group, the WHOLE config fails to parse
    /// => The default `[user]` group is applied (with `username=root`) even if the `[user]` settings are completely valid.
    /// => So make sure that every config group has a `#[serde(default)]` attribute to avoid invalid sections breaking unrelated parts of the config.
    #[serde(default)]
    pub command: CommandConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserConfig {
    pub username: String,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            username: "root".to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommandConfig {
    #[serde(default = "default_check")]
    pub check: String,
    #[serde(default = "default_install")]
    pub install: String,
    #[serde(default = "default_launch")]
    pub launch: String,
}

// Default desktop environment: KDE Plasma X11 session via Xwayland.
//
// Rationale (see also `gh-pages/docs/user/3-custom-de.md`):
//   * KDE Plasma offers a richer, more polished out-of-the-box experience on
//     Android-sized screens than LXQt and is the DE recommended in the project's
//     own user-facing docs and the 2025-07-30 KDE-support blog post.
//   * `pacman -Qg plasma` succeeds when at least one package from the Plasma
//     group is installed; together with the explicit checks for the auxiliary
//     packages we install, this gives us a reliable readiness signal.
//   * `startplasma-x11` is provided by `plasma-workspace` (member of the
//     `plasma` group). We launch it under `dbus-launch` so KDE gets a proper
//     session bus, matching the official KDE template in our docs.
//   * `xorg-xwayland` and `noto-fonts` are listed explicitly so the install
//     succeeds even on minimal Arch images that don't pull them in.
//   * `onboard` provides the on-screen keyboard wrapper that `setup.rs`
//     patches for proot compatibility, ensuring users without a physical
//     keyboard can type from the very first launch.

fn default_check() -> String {
    "pacman -Qg plasma && pacman -Q noto-fonts && pacman -Q xorg-xwayland && pacman -Q onboard"
        .to_string()
}

fn default_install() -> String {
    "stdbuf -oL pacman -Syu --needed --noconfirm --noprogressbar plasma noto-fonts xorg-xwayland onboard"
        .to_string()
}

fn default_launch() -> String {
    "XDG_RUNTIME_DIR=/tmp Xwayland -hidpi :1 2>&1 & while [ ! -e /tmp/.X11-unix/X1 ]; do sleep 0.1; done; XDG_SESSION_TYPE=x11 DISPLAY=:1 dbus-launch startplasma-x11 2>&1"
        .to_string()
}

impl Default for CommandConfig {
    fn default() -> Self {
        Self {
            check: default_check(),
            install: default_install(),
            launch: default_launch(),
        }
    }
}

/// This function does 2 major tasks:
/// - Read config from `CONFIG_FILE`, and override configs with their `try_*` versions, and return the configs line by line
/// - Write back to the config file, with `try_*` configs commented out
///
/// **Important**: As each call to this function will comment out the `try_*` config, it is **non-idempotent**.
fn process_config_file(full_config_path: String) -> Vec<String> {
    let mut write_back_lines: Vec<String> = vec![];
    let mut effective_config: Vec<String> = vec![];

    if let Ok(content) = fs::read_to_string(&full_config_path) {
        for line in content.lines() {
            let trimmed = line.trim();

            if let Some((key, value)) = trimmed.split_once('=') {
                let key = key.trim();
                let value = value.trim();

                if key.starts_with("try_") {
                    // Comment out the `try_*` configs
                    write_back_lines.push(format!("# {}", trimmed));

                    // Prefer the `try_*` configs
                    let actual_key = key.trim_start_matches("try_");
                    if let Some(line_index) = effective_config
                        .iter()
                        .position(|line| line.starts_with(&format!("{}=", actual_key)))
                    {
                        // Config exists, overriding
                        effective_config[line_index] = format!("{}={}", actual_key, value);
                    } else {
                        // Config does not exist, appending
                        effective_config.push(format!("{}={}", actual_key, value));
                        // Make sure there are no spaces around = so that the check existing key logic works
                    }
                } else {
                    // Keep the config as is
                    write_back_lines.push(trimmed.to_string());

                    if effective_config
                        .iter()
                        .any(|line| line.starts_with(&format!("{}=", key)))
                    {
                        // If already overridden by try_ version, skip inserting
                    } else {
                        // Config does not exist, appending
                        effective_config.push(format!("{}={}", key, value)); // Make sure there are no spaces around = so that the check existing key logic works
                    }
                }
            } else {
                // Keep the line as is
                write_back_lines.push(trimmed.to_string());
                effective_config.push(trimmed.to_string());
            }
        }

        // Rewrite config with try_* lines commented out
        let _ = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&full_config_path)
            .and_then(|mut file| {
                for line in &write_back_lines {
                    writeln!(file, "{}", line)?;
                }
                Ok(())
            });
    }

    // Convert effective config back to lines
    effective_config
}

pub fn parse_config(full_config_path: String) -> LocalConfig {
    let lines = process_config_file(full_config_path);
    let content = lines.join("\n");
    if let Ok(config) = toml::from_str::<LocalConfig>(&content) {
        return config;
    }
    // Config malformed, use the default config and the user can modify it again
    let default_config = LocalConfig::default();
    default_config
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn with_config_file(content: &str, f: impl Fn(String)) -> () {
        let dir = tempdir().unwrap();
        let base_dir = dir.path().to_str().unwrap();
        let path = format!("{}/etc/localdesktop", base_dir);
        fs::create_dir_all(&path).unwrap();
        let file_path = format!("{}/localdesktop.toml", path);
        fs::write(&file_path, content).unwrap();
        f(file_path)
    }

    #[test]
    fn should_handle_configs_without_try() {
        with_config_file(
            r#"
                [user]
                username = "alice"

                [command]
                check = "check-cmd"
                install = "install-cmd"
                launch = "launch-cmd"
            "#,
            |full_config_path| {
                let config = parse_config(full_config_path);
                assert_eq!(config.user.username, "alice");
                assert_eq!(config.command.check, "check-cmd");
                assert_eq!(config.command.install, "install-cmd");
                assert_eq!(config.command.launch, "launch-cmd");
            },
        );
    }

    #[test]
    fn should_handle_configs_with_try() {
        with_config_file(
            r#"
                [user]
                username = "root"
                try_username = "testuser"

                [command]
                check = "check-cmd"
                try_check = "try-check"
                install = "install-cmd"
                launch = "launch-cmd"
            "#,
            |full_config_path| {
                let config = parse_config(full_config_path);
                assert_eq!(config.user.username, "testuser");
                assert_eq!(config.command.check, "try-check");
                assert_eq!(config.command.install, "install-cmd")
            },
        );
    }

    #[test]
    fn should_comment_out_try_configs() {
        with_config_file(
            r#"
                username = "root"
                try_username = "commented"

                check = "normal"
                try_check = "try"
            "#,
            |full_config_path| {
                let _ = parse_config(full_config_path.clone()); // This triggers rewriting the config file
                let content = fs::read_to_string(full_config_path).unwrap();

                assert!(
                    content.contains("# try_username = \"commented\""),
                    "❌ `try_username` is not commented out after being applied"
                );
                assert!(
                    content.contains("# try_check = \"try\""),
                    "❌ `try_check` is not commented out after being  applied"
                );
            },
        );
    }

    #[test]
    fn default_config_targets_kde_plasma_x11_session() {
        // The default desktop environment must be KDE Plasma X11 (via Xwayland).
        // These assertions guard against accidental regressions in `default_check`,
        // `default_install`, and `default_launch` which would silently change which
        // DE a fresh install boots into.
        let default_command = CommandConfig::default();

        // The check must verify the Plasma group plus the auxiliary packages we
        // install (fonts, Xwayland, the on-screen keyboard wrapper target).
        assert!(
            default_command.check.contains("pacman -Qg plasma"),
            "default check must verify the `plasma` package group, got: {}",
            default_command.check
        );
        for pkg in ["noto-fonts", "xorg-xwayland", "onboard"] {
            assert!(
                default_command.check.contains(&format!("pacman -Q {}", pkg)),
                "default check must verify presence of `{}`, got: {}",
                pkg,
                default_command.check
            );
        }
        assert!(
            !default_command.check.contains("lxqt"),
            "default check must not mention LXQt anymore, got: {}",
            default_command.check
        );

        // The install command must use `pacman -Syu` with `--noconfirm` and pull
        // the Plasma group plus our auxiliary packages.
        assert!(default_command.install.starts_with("stdbuf -oL "));
        assert!(default_command.install.contains("--noconfirm"));
        assert!(default_command.install.contains("--noprogressbar"));
        for pkg in ["plasma", "noto-fonts", "xorg-xwayland", "onboard"] {
            assert!(
                default_command.install.contains(pkg),
                "default install must include `{}`, got: {}",
                pkg,
                default_command.install
            );
        }
        assert!(
            !default_command.install.contains("liblxqt"),
            "default install must not mention LXQt anymore, got: {}",
            default_command.install
        );

        // The launch command must start Xwayland on display :1 and then exec
        // `startplasma-x11` under `dbus-launch`.
        assert!(default_command.launch.contains("Xwayland -hidpi :1"));
        assert!(default_command.launch.contains("DISPLAY=:1"));
        assert!(default_command.launch.contains("dbus-launch startplasma-x11"));
        assert!(
            !default_command.launch.contains("startlxqt"),
            "default launch must not invoke startlxqt anymore, got: {}",
            default_command.launch
        );
    }
}

