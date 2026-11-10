//! Graphics settings (Linear DCM-59): how dense the instanced vegetation is, and each post effect
//! and ground extra as its own switch, so a slow machine can turn them off one by one or all at
//! once with the low preset.
//!
//! Where they come from, later ones winning: the client settings file (`graphics` in
//! `settings.ron`, see `ui_scale.rs`; never read offscreen, whose pictures depend on the flags
//! alone), `--graphics low|high` (a whole preset), `--vegetation off|low|high|N` (N from 0 to 2)
//! and `--no-post LIST` (`grain`, `tilt`, `vignette`, `bloom`, `ao`, `ground`, or `all`).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::Options;
use crate::ui_scale::ClientSettings;

/// The densest vegetation setting: twice the default.
pub const MAX_VEGETATION: f32 = 2.0;

#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Graphics {
    /// Instanced grass, flowers and litter: 0 none, 1 the default, up to [`MAX_VEGETATION`].
    pub vegetation: f32,
    /// Film grain.
    pub grain: bool,
    /// The soft focus toward the top and bottom of the screen.
    pub tilt_shift: bool,
    /// The darkened corners.
    pub vignette: bool,
    /// The glow round light sources.
    pub bloom: bool,
    /// The soft dark line where walls meet the ground and floors.
    pub contact_shadows: bool,
    /// Patchy colour on the ground, worked out per pixel.
    pub ground_detail: bool,
}

impl Default for Graphics {
    fn default() -> Self {
        Self::high()
    }
}

impl Graphics {
    pub fn high() -> Self {
        Self { vegetation: 1.0, grain: true, tilt_shift: true, vignette: true, bloom: true, contact_shadows: true, ground_detail: true }
    }

    /// For slow machines: a little grass, and nothing that costs a pass or a blend.
    pub fn low() -> Self {
        Self { vegetation: 0.35, grain: false, tilt_shift: false, vignette: false, bloom: false, contact_shadows: false, ground_detail: false }
    }

    /// Values a hand-edited file could hold, made sane.
    pub fn clamped(mut self) -> Self {
        self.vegetation = if self.vegetation.is_finite() { self.vegetation.clamp(0.0, MAX_VEGETATION) } else { 1.0 };
        self
    }

    /// `--vegetation`'s value: `off`, `low`, `high`, or a number from 0 to [`MAX_VEGETATION`].
    pub fn parse_vegetation(text: &str) -> Option<f32> {
        match text.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Some(0.0),
            "low" => Some(Self::low().vegetation),
            "on" | "high" => Some(1.0),
            n => n.parse::<f32>().ok().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v.min(MAX_VEGETATION)),
        }
    }

    /// The settings for this session (see the module docs for the order).
    pub fn from_options(options: &Options) -> Self {
        let mut g = if options.offscreen() { Self::default() } else { ClientSettings::load(&ClientSettings::path(&options.saves)).0.graphics.clamped() };
        match options.graphics.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("low") => g = Self::low(),
            Some("high") => g = Self::high(),
            Some(other) => eprintln!("--graphics {other}: low or high"),
            None => {}
        }
        if let Some(v) = options.vegetation.as_deref() {
            match Self::parse_vegetation(v) {
                Some(v) => g.vegetation = v,
                None => eprintln!("--vegetation {v}: off, low, high or a number from 0 to {MAX_VEGETATION}"),
            }
        }
        for name in options.no_post.iter().flat_map(|l| l.split(',')).map(|n| n.trim().to_ascii_lowercase()) {
            match name.as_str() {
                "grain" => g.grain = false,
                "tilt" | "tilt-shift" | "tiltshift" => g.tilt_shift = false,
                "vignette" => g.vignette = false,
                "bloom" => g.bloom = false,
                "ao" | "contact" => g.contact_shadows = false,
                "ground" => g.ground_detail = false,
                "all" => g = Self { vegetation: g.vegetation, ..Self::low() },
                "" => {}
                other => eprintln!("--no-post {other}: grain, tilt, vignette, bloom, ao, ground or all"),
            }
        }
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(line: &str) -> Options {
        // Offscreen, so no settings file is read.
        Options::parse(&format!("--render-to x.png {line}").split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn flags_pick_presets_and_switch_effects_off() {
        assert_eq!(Graphics::from_options(&options("")), Graphics::high());
        assert_eq!(Graphics::from_options(&options("--graphics low")), Graphics::low());
        let g = Graphics::from_options(&options("--graphics low --vegetation 1.5"));
        assert_eq!(g, Graphics { vegetation: 1.5, ..Graphics::low() });
        let g = Graphics::from_options(&options("--vegetation off --no-post grain,tilt"));
        assert_eq!(g, Graphics { vegetation: 0.0, grain: false, tilt_shift: false, ..Graphics::high() });
        let g = Graphics::from_options(&options("--no-post all"));
        assert_eq!(g, Graphics { vegetation: 1.0, ..Graphics::low() });
        // Mistakes leave the setting alone.
        assert_eq!(Graphics::from_options(&options("--vegetation lots --graphics ultra --no-post sparkle")), Graphics::high());
    }

    #[test]
    fn vegetation_values_are_checked() {
        assert_eq!(Graphics::parse_vegetation("off"), Some(0.0));
        assert_eq!(Graphics::parse_vegetation("HIGH"), Some(1.0));
        assert_eq!(Graphics::parse_vegetation("0.5"), Some(0.5));
        assert_eq!(Graphics::parse_vegetation("9"), Some(MAX_VEGETATION));
        assert_eq!(Graphics::parse_vegetation("-1"), None);
        assert_eq!(Graphics::parse_vegetation("nan"), None);
        assert_eq!(Graphics { vegetation: f32::NAN, ..Graphics::high() }.clamped().vegetation, 1.0);
    }

    #[test]
    fn the_settings_file_keeps_them() {
        let text = ron::to_string(&ClientSettings { graphics: Graphics::low(), ..Default::default() }).unwrap();
        assert_eq!(ron::from_str::<ClientSettings>(&text).unwrap().graphics, Graphics::low());
        // An older file has none: the defaults.
        assert_eq!(ron::from_str::<ClientSettings>("(ui_scale: 110)").unwrap().graphics, Graphics::high());
        assert_eq!(ron::from_str::<ClientSettings>("(graphics: (grain: false))").unwrap().graphics, Graphics { grain: false, ..Graphics::high() });
    }
}
