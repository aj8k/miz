//! Shader library: WGSL sources, either built into the binary or loaded
//! from a directory (dev mode, hot reload).
//!
//! WGSL has no `#include`, so `common.wgsl` is prepended to every shader.
//! Every composed source is validated with naga (the same front end wgpu
//! uses) before it reaches the GPU, so a typo produces a readable
//! `file:line: message` instead of a broken pipeline.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shader {
    BassPulse,
    Present,
    Hud,
    Luma,
    Crossfade,
    FeedbackWarp,
    FeedbackWave,
    FeedbackDisplay,
    LissajousDecay,
    LissajousLine,
    LissajousDisplay,
    Radial,
    Nebula,
}

impl Shader {
    pub const ALL: [Shader; 13] = [
        Shader::BassPulse,
        Shader::Present,
        Shader::Hud,
        Shader::Luma,
        Shader::Crossfade,
        Shader::FeedbackWarp,
        Shader::FeedbackWave,
        Shader::FeedbackDisplay,
        Shader::LissajousDecay,
        Shader::LissajousLine,
        Shader::LissajousDisplay,
        Shader::Radial,
        Shader::Nebula,
    ];

    pub fn file(self) -> &'static str {
        match self {
            Shader::BassPulse => "bass_pulse.wgsl",
            Shader::Present => "present.wgsl",
            Shader::Hud => "hud.wgsl",
            Shader::Luma => "luma.wgsl",
            Shader::Crossfade => "crossfade.wgsl",
            Shader::FeedbackWarp => "feedback_warp.wgsl",
            Shader::FeedbackWave => "feedback_wave.wgsl",
            Shader::FeedbackDisplay => "feedback_display.wgsl",
            Shader::LissajousDecay => "lissajous_decay.wgsl",
            Shader::LissajousLine => "lissajous_line.wgsl",
            Shader::LissajousDisplay => "lissajous_display.wgsl",
            Shader::Radial => "radial.wgsl",
            Shader::Nebula => "nebula.wgsl",
        }
    }

    fn embedded(self) -> &'static str {
        match self {
            Shader::BassPulse => include_str!("../../shaders/bass_pulse.wgsl"),
            Shader::Present => include_str!("../../shaders/present.wgsl"),
            Shader::Hud => include_str!("../../shaders/hud.wgsl"),
            Shader::Luma => include_str!("../../shaders/luma.wgsl"),
            Shader::Crossfade => include_str!("../../shaders/crossfade.wgsl"),
            Shader::FeedbackWarp => include_str!("../../shaders/feedback_warp.wgsl"),
            Shader::FeedbackWave => include_str!("../../shaders/feedback_wave.wgsl"),
            Shader::FeedbackDisplay => include_str!("../../shaders/feedback_display.wgsl"),
            Shader::LissajousDecay => include_str!("../../shaders/lissajous_decay.wgsl"),
            Shader::LissajousLine => include_str!("../../shaders/lissajous_line.wgsl"),
            Shader::LissajousDisplay => include_str!("../../shaders/lissajous_display.wgsl"),
            Shader::Radial => include_str!("../../shaders/radial.wgsl"),
            Shader::Nebula => include_str!("../../shaders/nebula.wgsl"),
        }
    }
}

const COMMON_FILE: &str = "common.wgsl";
const COMMON: &str = include_str!("../../shaders/common.wgsl");

/// A complete, validated set of shader sources.
pub struct ShaderLib {
    composed: HashMap<Shader, String>,
}

impl ShaderLib {
    /// Loads every shader (from `dir` if given, else the ones built into the
    /// binary) and validates it. On error
    /// returns a report of *all* failing files, so you can fix them in one go.
    pub fn load(dir: Option<&Path>) -> Result<Self, String> {
        let read = |file: &str, builtin: &'static str| -> Result<String, String> {
            match dir {
                Some(d) => std::fs::read_to_string(d.join(file))
                    .map_err(|e| format!("{}: {e}", d.join(file).display())),
                None => Ok(builtin.to_string()),
            }
        };
        let common = read(COMMON_FILE, COMMON)?;
        // +1: the newline we insert between common and the shader.
        let common_lines = common.lines().count() as u32 + 1;
        let place = dir.map(|d| format!("{}/", d.display())).unwrap_or_default();
        let mut composed = HashMap::new();
        let mut errors = Vec::new();
        for shader in Shader::ALL {
            let src = match read(shader.file(), shader.embedded()) {
                Ok(s) => s,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            let full = format!("{common}\n{src}");
            match validate(&full) {
                Ok(()) => {
                    composed.insert(shader, full);
                }
                Err((line, msg)) => {
                    let file = shader.file();
                    errors.push(match line {
                        Some(l) if l > common_lines => {
                            format!("{place}{file}:{}: {msg}", l - common_lines)
                        }
                        Some(l) => format!("{place}{COMMON_FILE}:{l}: {msg} (via {file})"),
                        None => format!("{place}{file}: {msg}"),
                    });
                }
            }
        }
        if errors.is_empty() {
            Ok(Self { composed })
        } else {
            Err(errors.join("\n"))
        }
    }

    pub fn module(&self, device: &wgpu::Device, shader: Shader) -> wgpu::ShaderModule {
        // Every entry is present after a successful `load`; the embedded
        // fallback keeps this infallible anyway.
        let fallback;
        let source = match self.composed.get(&shader) {
            Some(s) => s.as_str(),
            None => {
                fallback = format!("{COMMON}\n{}", shader.embedded());
                &fallback
            }
        };
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(shader.file()),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        })
    }
}

/// Parses and validates WGSL; on error returns (line in `source`, message).
fn validate(source: &str) -> Result<(), (Option<u32>, String)> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| {
        (
            e.location(source).map(|l| l.line_number),
            e.message().to_string(),
        )
    })?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| {
        // Append the chain of causes: the innermost is the most specific.
        let mut msg = e.as_inner().to_string();
        let mut cause = std::error::Error::source(e.as_inner());
        while let Some(c) = cause {
            msg = format!("{msg}: {c}");
            cause = c.source();
        }
        (e.location(source).map(|l| l.line_number), msg)
    })?;
    Ok(())
}

/// Shader directory for `--dev`: the source tree's `shaders/` (baked in at
/// compile time, so `cargo run -- --dev` works from anywhere).
pub fn source_tree_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_shaders_are_valid() {
        if let Err(e) = ShaderLib::load(None) {
            panic!("{e}");
        }
    }

    #[test]
    fn errors_point_at_the_right_file_and_line() {
        let dir = std::env::temp_dir().join(format!("miz-shader-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(COMMON_FILE), COMMON).unwrap();
        for s in Shader::ALL {
            std::fs::write(dir.join(s.file()), s.embedded()).unwrap();
        }
        std::fs::write(dir.join("radial.wgsl"), "\n\nfn broken( {\n").unwrap();
        let err = ShaderLib::load(Some(&dir)).err().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(err.contains("radial.wgsl:3:"), "{err}");
        assert_eq!(err.lines().count(), 1, "{err}");
    }
}
