use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorOverride {
    Default,
    None,
    Custom(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModOverride {
    Default,
    Force,
    Suppress,
}

#[derive(Debug, Clone)]
pub struct Remap {
    pub display: Option<String>,
    pub color: ColorOverride,
    pub modifier: ModOverride,
}

impl Default for Remap {
    fn default() -> Self {
        Self {
            display: None,
            color: ColorOverride::Default,
            modifier: ModOverride::Default,
        }
    }
}

pub struct Config {
    remaps: HashMap<String, Remap>,
}

impl Config {
    pub fn load() -> Self {
        match Self::try_load() {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Warning: {e:#}");
                Self::empty()
            }
        }
    }

    pub fn empty() -> Self {
        Self {
            remaps: HashMap::new(),
        }
    }

    pub fn get(&self, keysym: &str) -> Option<&Remap> {
        self.remaps.get(keysym)
    }

    fn try_load() -> Result<Self> {
        let path = config_path();
        let contents = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::empty()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };

        let mut remaps: HashMap<String, Remap> = HashMap::new();
        let mut errors = 0;

        for (lineno, line) in contents.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            if let Err(e) = parse_line(trimmed, &mut remaps) {
                eprintln!("keymap.conf:{}: {e}", lineno + 1);
                errors += 1;
            }
        }

        if errors > 0 {
            eprintln!("keymap.conf: {errors} error(s), partially loaded");
        }

        let count = remaps.len();
        if count > 0 {
            eprintln!("Loaded {count} keymap remap(s) from {}", path.display());
        }

        Ok(Self { remaps })
    }
}

fn config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("wshowkeys/keymap.conf");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config/wshowkeys/keymap.conf");
    }
    PathBuf::from("/etc/wshowkeys/keymap.conf")
}

fn parse_line(line: &str, remaps: &mut HashMap<String, Remap>) -> Result<()> {
    let (lhs, rhs) = line.split_once('=').context("missing '='")?;
    let lhs = lhs.trim();
    let rhs = rhs.trim();

    if let Some(keysym) = lhs.strip_suffix(":fmt") {
        let keysym = keysym.trim();
        let entry = remaps.entry(keysym.to_string()).or_default();
        parse_fmt(rhs, entry)?;
    } else {
        let entry = remaps.entry(lhs.to_string()).or_default();
        entry.display = Some(rhs.to_string());
    }

    Ok(())
}

fn parse_fmt(value: &str, entry: &mut Remap) -> Result<()> {
    let (color_str, mod_str) = match value.split_once(',') {
        Some((c, m)) => (c.trim(), Some(m.trim())),
        None => (value.trim(), None),
    };

    if !color_str.is_empty() {
        entry.color = match color_str {
            "default" => ColorOverride::Default,
            "none" => ColorOverride::None,
            s if s.starts_with('#') => ColorOverride::Custom(parse_color(s)?),
            other => bail!("invalid color '{other}' (expected default|none|#rrggbb[aa])"),
        };
    }

    if let Some(m) = mod_str {
        entry.modifier = match m {
            "m" => ModOverride::Force,
            "!m" => ModOverride::Suppress,
            other => bail!("invalid modifier '{other}' (expected 'm' or '!m')"),
        };
    }

    Ok(())
}

pub fn parse_color(color: &str) -> Result<u32> {
    let hex = color.strip_prefix('#').unwrap_or(color);
    match hex.len() {
        6 => {
            let v = u32::from_str_radix(hex, 16).context("invalid hex color")?;
            Ok((v << 8) | 0xFF)
        }
        8 => {
            let v = u32::from_str_radix(hex, 16).context("invalid hex color")?;
            Ok(v)
        }
        _ => bail!("invalid color '{color}', expected 6 or 8 hex digits"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_color_6digit() {
        assert_eq!(parse_color("#FF0000").unwrap(), 0xFF0000FF);
    }

    #[test]
    fn parse_color_8digit() {
        assert_eq!(parse_color("#FF0000CC").unwrap(), 0xFF0000CC);
    }

    #[test]
    fn parse_color_no_hash() {
        assert_eq!(parse_color("00FF00").unwrap(), 0x00FF00FF);
    }

    #[test]
    fn parse_line_display_remap() {
        let mut remaps = HashMap::new();
        parse_line("Return = ⏎", &mut remaps).unwrap();
        assert_eq!(remaps["Return"].display.as_deref(), Some("⏎"));
        assert_eq!(remaps["Return"].color, ColorOverride::Default);
        assert_eq!(remaps["Return"].modifier, ModOverride::Default);
    }

    #[test]
    fn parse_line_fmt_color_and_mod() {
        let mut remaps = HashMap::new();
        parse_line("Shift_L:fmt = #ff8800,m", &mut remaps).unwrap();
        assert_eq!(remaps["Shift_L"].display, None);
        assert!(matches!(remaps["Shift_L"].color, ColorOverride::Custom(_)));
        assert_eq!(remaps["Shift_L"].modifier, ModOverride::Force);
    }

    #[test]
    fn parse_line_fmt_suppress() {
        let mut remaps = HashMap::new();
        parse_line("space:fmt = none,!m", &mut remaps).unwrap();
        assert_eq!(remaps["space"].color, ColorOverride::None);
        assert_eq!(remaps["space"].modifier, ModOverride::Suppress);
    }

    #[test]
    fn parse_line_display_and_fmt_merge() {
        let mut remaps = HashMap::new();
        parse_line("Return = ⏎", &mut remaps).unwrap();
        parse_line("Return:fmt = #aabbcc", &mut remaps).unwrap();
        assert_eq!(remaps["Return"].display.as_deref(), Some("⏎"));
        assert!(matches!(remaps["Return"].color, ColorOverride::Custom(_)));
    }
}
