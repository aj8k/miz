//! Configuration file (`~/.config/miz/config.toml`).
//!
//! On first run the commented template below is written, so the file
//! documents itself. Values are validated and clamped; safety limits can
//! be made stricter but never looser than the built-in ceiling.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

use crate::analysis::AnalyzerConfig;
use crate::analysis::smooth::AttackRelease;
use crate::audio::SourceKind;
use crate::scenes;

/// Written on first run. Must parse to exactly `Config::default()` (tested).
pub const TEMPLATE: &str = r#"# miz configuration. Changes are applied live while miz is running
# (except where noted). Delete this file to get the defaults back.

[audio]
# "pipewire" = what's playing on the default output; "test" = synthetic signal.
source = "pipewire"
# Number of log-spaced frequency bands (8..128).
bands = 64
# Compensates music's natural bass-heavy slope, in dB per octave (0 = off).
tilt_db_per_octave = 4.5
# Dynamic range shown, in dB below the auto-gain reference. Smaller = punchier.
range_db = 32.0
# Band smoothing: how fast levels rise and fall, in milliseconds.
attack_ms = 15.0
release_ms = 250.0
# How fast auto-gain recovers after loud passages, dB per second.
gain_decay_db_per_s = 2.0

[visual]
# Calm mode: slower, softer, dimmer. Toggle at runtime with C.
calm = true
# Scene at startup: feedback, lissajous, radial, nebula (or bass-pulse).
scene = "feedback"
# Switch to the next scene every N minutes (0 = never).
rotate_minutes = 5.0
# Stay on the current scene (no auto-rotation). Toggle at runtime with L.
lock = false
crossfade_seconds = 4.0
# Internal resolution relative to the window (0.25..1.0); lower = lighter GPU load.
render_scale = 1.0
# Frame-rate target; the actual rate is refresh / N closest to it (60 on a
# 120 Hz screen, 72 on 144 Hz).
target_fps = 60.0
# "low-power" (integrated GPU) or "high-performance". Needs a restart.
gpu = "low-power"
# Start in fullscreen. Toggle at runtime with F.
fullscreen = false

[safety]
# Maximum change of the picture's average brightness, as a fraction of
# full white per second. Prevents flashing. Values above the built-in
# ceiling (0.25) are clamped; lower values are stricter.
max_brightness_rate = 0.25
calm_max_brightness_rate = 0.12

[dev]
# Load shaders from this directory and reload them on save (also: --dev).
# Empty = use the shaders built into the binary.
shader_dir = ""
"#;

/// Hard ceiling for the brightness rate (see `[safety]` in the template).
pub const MAX_BRIGHTNESS_RATE_CEILING: f32 = 0.25;

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub audio: Audio,
    pub visual: Visual,
    pub safety: Safety,
    pub dev: Dev,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Audio {
    pub source: String,
    pub bands: usize,
    pub tilt_db_per_octave: f32,
    pub range_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub gain_decay_db_per_s: f32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Visual {
    pub calm: bool,
    pub scene: String,
    pub rotate_minutes: f32,
    pub lock: bool,
    pub crossfade_seconds: f32,
    pub render_scale: f32,
    pub target_fps: f32,
    pub gpu: String,
    pub fullscreen: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Safety {
    pub max_brightness_rate: f32,
    pub calm_max_brightness_rate: f32,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Dev {
    pub shader_dir: String,
}

impl Default for Audio {
    fn default() -> Self {
        let a = AnalyzerConfig::default();
        Self {
            source: "pipewire".into(),
            bands: a.bands,
            tilt_db_per_octave: a.tilt_db_per_octave,
            range_db: a.range_db,
            attack_ms: a.band_smoothing.attack_s * 1000.0,
            release_ms: a.band_smoothing.release_s * 1000.0,
            gain_decay_db_per_s: a.gain_decay_db_per_s,
        }
    }
}

impl Default for Visual {
    fn default() -> Self {
        Self {
            calm: true,
            scene: "feedback".into(),
            rotate_minutes: 5.0,
            lock: false,
            crossfade_seconds: 4.0,
            render_scale: 1.0,
            target_fps: 60.0,
            gpu: "low-power".into(),
            fullscreen: false,
        }
    }
}

impl Default for Safety {
    fn default() -> Self {
        Self {
            max_brightness_rate: 0.25,
            calm_max_brightness_rate: 0.12,
        }
    }
}

impl Config {
    /// Parses and validates; returns warnings for values that were fixed up.
    pub fn parse(text: &str) -> anyhow::Result<(Self, Vec<String>)> {
        let cfg: Config = toml::from_str(text)?;
        Ok(cfg.sanitized())
    }

    fn sanitized(mut self) -> (Self, Vec<String>) {
        let mut warn = Vec::new();
        let mut clamp = |name: &str, v: &mut f32, lo: f32, hi: f32| {
            if !(lo..=hi).contains(v) || v.is_nan() {
                let fixed = if v.is_nan() { lo } else { v.clamp(lo, hi) };
                warn.push(format!(
                    "{name} = {v} is out of range {lo}..{hi}; using {fixed}"
                ));
                *v = fixed;
            }
        };
        let a = &mut self.audio;
        clamp(
            "audio.tilt_db_per_octave",
            &mut a.tilt_db_per_octave,
            -6.0,
            12.0,
        );
        clamp("audio.range_db", &mut a.range_db, 10.0, 90.0);
        clamp("audio.attack_ms", &mut a.attack_ms, 0.0, 2000.0);
        clamp("audio.release_ms", &mut a.release_ms, 0.0, 5000.0);
        clamp(
            "audio.gain_decay_db_per_s",
            &mut a.gain_decay_db_per_s,
            0.1,
            30.0,
        );
        let v = &mut self.visual;
        clamp(
            "visual.rotate_minutes",
            &mut v.rotate_minutes,
            0.0,
            24.0 * 60.0,
        );
        clamp(
            "visual.crossfade_seconds",
            &mut v.crossfade_seconds,
            0.5,
            30.0,
        );
        clamp("visual.render_scale", &mut v.render_scale, 0.25, 1.0);
        clamp("visual.target_fps", &mut v.target_fps, 20.0, 240.0);
        let s = &mut self.safety;
        let ceil = MAX_BRIGHTNESS_RATE_CEILING;
        clamp(
            "safety.max_brightness_rate",
            &mut s.max_brightness_rate,
            0.01,
            ceil,
        );
        clamp(
            "safety.calm_max_brightness_rate",
            &mut s.calm_max_brightness_rate,
            0.01,
            ceil,
        );

        if !(8..=128).contains(&self.audio.bands) {
            let fixed = self.audio.bands.clamp(8, 128);
            warn.push(format!(
                "audio.bands = {} is out of range 8..128; using {fixed}",
                self.audio.bands
            ));
            self.audio.bands = fixed;
        }
        if self.source_kind().is_none() {
            warn.push(format!(
                "audio.source = {:?} is unknown; using \"pipewire\"",
                self.audio.source
            ));
            self.audio.source = "pipewire".into();
        }
        let scene_ok = scenes::ROTATION.contains(&self.visual.scene.as_str())
            || scenes::EXTRA.contains(&self.visual.scene.as_str());
        if !scene_ok {
            warn.push(format!(
                "visual.scene = {:?} is unknown; using \"feedback\"",
                self.visual.scene
            ));
            self.visual.scene = "feedback".into();
        }
        if !matches!(self.visual.gpu.as_str(), "low-power" | "high-performance") {
            warn.push(format!(
                "visual.gpu = {:?} is unknown; using \"low-power\"",
                self.visual.gpu
            ));
            self.visual.gpu = "low-power".into();
        }
        (self, warn)
    }

    pub fn source_kind(&self) -> Option<SourceKind> {
        match self.audio.source.as_str() {
            "pipewire" | "pw" => Some(SourceKind::PipeWire),
            "test" => Some(SourceKind::Test),
            _ => None,
        }
    }

    pub fn analyzer(&self) -> AnalyzerConfig {
        let a = &self.audio;
        AnalyzerConfig {
            bands: a.bands,
            tilt_db_per_octave: a.tilt_db_per_octave,
            range_db: a.range_db,
            band_smoothing: AttackRelease::new(a.attack_ms / 1000.0, a.release_ms / 1000.0),
            gain_decay_db_per_s: a.gain_decay_db_per_s,
            ..AnalyzerConfig::default()
        }
    }

    pub fn shader_dir(&self) -> Option<PathBuf> {
        (!self.dev.shader_dir.is_empty()).then(|| PathBuf::from(&self.dev.shader_dir))
    }
}

/// `$XDG_CONFIG_HOME/miz/config.toml`, falling back to `~/.config/...`.
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("miz").join("config.toml"))
}

/// Reads the config, writing the template first if the file doesn't exist.
pub fn load_or_create(path: &Path) -> anyhow::Result<Config> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::write(path, TEMPLATE).with_context(|| format!("writing {}", path.display()))?;
        eprintln!("miz: wrote default config to {}", path.display());
    }
    load(path)
}

pub fn load(path: &Path) -> anyhow::Result<Config> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let (cfg, warnings) =
        Config::parse(&text).with_context(|| format!("invalid config {}", path.display()))?;
    for w in warnings {
        eprintln!("miz: {}: {w}", path.display());
    }
    Ok(cfg)
}

/// Writes `cfg` to `path`, keeping the file's comments and layout: values
/// are replaced in place in the existing TOML document (the template if the
/// file is missing or unreadable). Written atomically (temp file + rename),
/// so a crash can't leave a half-written config.
pub fn save(path: &Path, cfg: &Config) -> anyhow::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = match existing.parse() {
        Ok(doc) if !existing.trim().is_empty() => doc,
        _ => TEMPLATE.parse().context("parsing the built-in template")?,
    };
    let a = &cfg.audio;
    set(&mut doc, "audio", "source", a.source.as_str().into());
    set(&mut doc, "audio", "bands", (a.bands as i64).into());
    set(
        &mut doc,
        "audio",
        "tilt_db_per_octave",
        float(a.tilt_db_per_octave),
    );
    set(&mut doc, "audio", "range_db", float(a.range_db));
    set(&mut doc, "audio", "attack_ms", float(a.attack_ms));
    set(&mut doc, "audio", "release_ms", float(a.release_ms));
    set(
        &mut doc,
        "audio",
        "gain_decay_db_per_s",
        float(a.gain_decay_db_per_s),
    );
    let v = &cfg.visual;
    set(&mut doc, "visual", "calm", v.calm.into());
    set(&mut doc, "visual", "scene", v.scene.as_str().into());
    set(
        &mut doc,
        "visual",
        "rotate_minutes",
        float(v.rotate_minutes),
    );
    set(&mut doc, "visual", "lock", v.lock.into());
    set(
        &mut doc,
        "visual",
        "crossfade_seconds",
        float(v.crossfade_seconds),
    );
    set(&mut doc, "visual", "render_scale", float(v.render_scale));
    set(&mut doc, "visual", "target_fps", float(v.target_fps));
    set(&mut doc, "visual", "gpu", v.gpu.as_str().into());
    set(&mut doc, "visual", "fullscreen", v.fullscreen.into());
    let s = &cfg.safety;
    set(
        &mut doc,
        "safety",
        "max_brightness_rate",
        float(s.max_brightness_rate),
    );
    set(
        &mut doc,
        "safety",
        "calm_max_brightness_rate",
        float(s.calm_max_brightness_rate),
    );
    set(
        &mut doc,
        "dev",
        "shader_dir",
        cfg.dev.shader_dir.as_str().into(),
    );

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string()).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// f32 -> TOML float without binary noise (0.12f32 would print as
/// 0.11999999731779099).
fn float(v: f32) -> toml_edit::Value {
    let rounded: f64 = format!("{v:.4}").parse().unwrap_or(f64::from(v));
    rounded.into()
}

/// Sets `[table] key = value`, keeping the existing value's decoration
/// (e.g. a trailing `# comment`); creates the table/key if missing.
fn set(doc: &mut toml_edit::DocumentMut, table: &str, key: &str, value: toml_edit::Value) {
    let item = &mut doc[table][key];
    match item.as_value_mut() {
        Some(old) => {
            let decor = old.decor().clone();
            *old = value;
            *old.decor_mut() = decor;
        }
        None => *item = toml_edit::Item::Value(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_matches_defaults() {
        let (cfg, warnings) = Config::parse(TEMPLATE).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn missing_keys_use_defaults_and_unknown_keys_are_errors() {
        let (cfg, _) = Config::parse("[visual]\ncalm = false\n").unwrap();
        assert!(!cfg.visual.calm);
        assert_eq!(cfg.audio, Audio::default());
        assert!(Config::parse("[visual]\ncalmm = false\n").is_err());
    }

    #[test]
    fn save_keeps_comments_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("miz-config-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        // Missing file: saving defaults reproduces the template exactly.
        save(&path, &Config::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);

        let mut cfg = Config::default();
        cfg.visual.calm = false;
        cfg.visual.scene = "nebula".into();
        cfg.safety.calm_max_brightness_rate = 0.07;
        save(&path, &cfg).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(text.contains("# Calm mode: slower, softer, dimmer."));
        assert!(text.contains("calm_max_brightness_rate = 0.07\n"));
        assert_eq!(Config::parse(&text).unwrap().0, cfg);
    }

    #[test]
    fn safety_cannot_be_loosened() {
        let (cfg, warnings) = Config::parse("[safety]\nmax_brightness_rate = 5.0\n").unwrap();
        assert_eq!(cfg.safety.max_brightness_rate, MAX_BRIGHTNESS_RATE_CEILING);
        assert_eq!(warnings.len(), 1);
    }
}
