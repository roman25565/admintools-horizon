//! Config files: one TOML file per mod in the server's `config/` folder, written with the
//! defaults (and their comments) the first time, read with every missing key at its default.
//! A bad file never stops the server: the problem is reported and the defaults are used.
//!
//! [`load_toml`] is the shared mechanic any of our mods can use for its own file.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::fs;
use std::path::Path;

/// The default config file, comments included.
pub const DEFAULT_TEXT: &str = include_str!("../../data/admintools.toml");
pub const FILE: &str = "admintools.toml";

/// Reads `dir/file` as `T`, writing `default_text` there first when the file is missing.
/// Returns the value and the problems met (each one line, for the log). A file that does not
/// does not parse gives the values of `default_text`.
pub fn load_toml<T: DeserializeOwned + Default>(dir: &Path, file: &str, default_text: &str) -> (T, Vec<String>) {
    let path = dir.join(file);
    let mut problems = Vec::new();
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            if let Err(e) = fs::create_dir_all(dir).and_then(|()| fs::write(&path, default_text)) {
                problems.push(format!("could not write {}: {e}", path.display()));
            }
            default_text.to_owned()
        }
    };
    match toml::from_str::<T>(&text) {
        Ok(v) => (v, problems),
        Err(e) => {
            problems.push(format!("{}: {e}; using the defaults", path.display()));
            let v = toml::from_str::<T>(default_text).unwrap_or_default();
            (v, problems)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub admin_level: u8,
    pub profiler: ProfilerConfig,
    pub structures: StructuresConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ProfilerConfig {
    pub default_seconds: u32,
    pub max_seconds: u32,
    pub mob_sample_every: u32,
    pub spike_ms: f32,
    pub spike_log_ms: f32,
    pub top: usize,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct StructuresConfig {
    pub max_results: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config { admin_level: 2, profiler: ProfilerConfig::default(), structures: StructuresConfig::default() }
    }
}

impl Default for ProfilerConfig {
    fn default() -> Self {
        ProfilerConfig { default_seconds: 30, max_seconds: 600, mob_sample_every: 4, spike_ms: 60.0, spike_log_ms: 0.0, top: 8 }
    }
}

impl Default for StructuresConfig {
    fn default() -> Self {
        StructuresConfig { max_results: 15 }
    }
}

impl Config {
    /// Values outside their range are brought back in, each with a problem line.
    pub fn checked(mut self, problems: &mut Vec<String>) -> Self {
        if self.admin_level > 4 {
            problems.push(format!("admin_level {} is above 4, using 4", self.admin_level));
            self.admin_level = 4;
        }
        let p = &mut self.profiler;
        if p.max_seconds == 0 || p.max_seconds > 3600 {
            problems.push(format!("profiler.max_seconds {} is outside 1..=3600, using 600", p.max_seconds));
            p.max_seconds = 600;
        }
        if p.default_seconds == 0 || p.default_seconds > p.max_seconds {
            problems.push(format!("profiler.default_seconds {} is outside 1..={}, using 30", p.default_seconds, p.max_seconds));
            p.default_seconds = 30.min(p.max_seconds);
        }
        if p.mob_sample_every == 0 || p.mob_sample_every > 100 {
            problems.push(format!("profiler.mob_sample_every {} is outside 1..=100, using 4", p.mob_sample_every));
            p.mob_sample_every = 4;
        }
        if !(p.spike_ms.is_finite() && p.spike_ms > 0.0) {
            problems.push(format!("profiler.spike_ms {} must be above 0, using 60", p.spike_ms));
            p.spike_ms = 60.0;
        }
        if !(p.spike_log_ms.is_finite() && p.spike_log_ms >= 0.0) {
            problems.push(format!("profiler.spike_log_ms {} must be 0 or above, using 0", p.spike_log_ms));
            p.spike_log_ms = 0.0;
        }
        if p.top == 0 || p.top > 50 {
            p.top = 8;
        }
        if self.structures.max_results == 0 || self.structures.max_results > 100 {
            self.structures.max_results = 15;
        }
        self
    }
}

/// Reads `dir/admintools.toml`.
pub fn load(dir: &Path) -> (Config, Vec<String>) {
    let (c, mut problems) = load_toml::<Config>(dir, FILE, DEFAULT_TEXT);
    let c = c.checked(&mut problems);
    (c, problems)
}
