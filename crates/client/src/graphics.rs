//! Graphics settings (Linear DCM-59): how dense the instanced vegetation is, each post effect and
//! ground extra as its own switch, the sun's shadow map and multisampling, gathered into presets.
//!
//! Presets: Low, Medium, High (the look the game was made to, the default) and Ultra, plus Auto,
//! which plays the first seconds at High, measures the frame time and settles on the preset the
//! machine holds at a good rate (see `graphics_ui.rs`). F8 opens a panel to pick one in game.
//!
//! Where they come from, later ones winning: the client settings file (`graphics` in
//! `settings.ron`, see `ui_scale.rs`; never read offscreen, whose pictures depend on the flags
//! alone), `--graphics low|medium|high|ultra|auto` (a whole preset), `--vegetation off|low|high|N`
//! (N from 0 to 2) and `--no-post LIST` (`grain`, `tilt`, `vignette`, `bloom`, `ao`, `ground`, or
//! `all`). A game started with no settings file at all starts on Auto.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::Options;
use crate::ui_scale::ClientSettings;

/// A whole set of graphics settings by name. `Custom` is what flags or a hand-edited file made of
/// one; `Auto` measures and then becomes one of the others.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Preset {
    Low,
    Medium,
    #[default]
    High,
    Ultra,
    Auto,
    Custom,
}

impl Preset {
    /// The ones a player picks from, in order.
    pub const CHOICES: [Preset; 5] = [Preset::Low, Preset::Medium, Preset::High, Preset::Ultra, Preset::Auto];

    pub fn name(self) -> &'static str {
        match self {
            Preset::Low => "Low",
            Preset::Medium => "Medium",
            Preset::High => "High",
            Preset::Ultra => "Ultra",
            Preset::Auto => "Auto",
            Preset::Custom => "Custom",
        }
    }

    pub fn parse(text: &str) -> Option<Preset> {
        match text.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Preset::Low),
            "medium" | "med" => Some(Preset::Medium),
            "high" => Some(Preset::High),
            "ultra" => Some(Preset::Ultra),
            "auto" => Some(Preset::Auto),
            _ => None,
        }
    }
}

/// The densest vegetation setting: twice the default.
pub const MAX_VEGETATION: f32 = 2.0;

#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Graphics {
    /// Which preset this is (or `Custom`).
    pub preset: Preset,
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
    /// Shadows of the sun and moon with soft edges that widen away from what casts them.
    pub soft_shadows: bool,
    /// Patchy colour on the ground, worked out per pixel.
    pub ground_detail: bool,
    /// The sun's shadow map, texels a side (one cascade fitted to the view, `daynight.rs`).
    pub shadow_map: u32,
    /// Samples a pixel (1, 2 or 4). Window glass is alpha to coverage, which wants 4.
    pub msaa: u32,
    /// The 3D world's resolution as a share of the window's on a HiDPI screen (0.5..1), scaled up
    /// with the UI kept sharp (`render_scale.rs`; on an ordinary screen it is raised toward 1).
    pub render_scale: f32,
}

impl Default for Graphics {
    fn default() -> Self {
        Self::high()
    }
}

impl Graphics {
    pub fn high() -> Self {
        Self { preset: Preset::High, vegetation: 1.0, grain: true, tilt_shift: true, vignette: true, bloom: true, contact_shadows: true, soft_shadows: true, ground_detail: true, shadow_map: 4096, msaa: 4, render_scale: 0.75 }
    }
    /// For slow machines: a little grass, and nothing that costs a pass or a blend.
    pub fn low() -> Self {
        Self { preset: Preset::Low, vegetation: 0.35, grain: false, tilt_shift: false, vignette: false, bloom: false, contact_shadows: false, soft_shadows: false, ground_detail: false, shadow_map: 2048, msaa: 2, render_scale: 0.6 }
    }
    /// Most of the look: thinner grass, no tilt-shift, half the shadow map.
    pub fn medium() -> Self {
        Self { preset: Preset::Medium, vegetation: 0.6, tilt_shift: false, soft_shadows: false, shadow_map: 2048, ..Self::high() }
    }
    /// More than the game was made to: denser grass and a finer shadow map.
    pub fn ultra() -> Self {
        Self { preset: Preset::Ultra, vegetation: 1.5, shadow_map: 8192, render_scale: 1.0, ..Self::high() }
    }
    /// The settings a preset stands for. Auto starts at High (and is measured, `graphics_ui.rs`);
    /// Custom has no settings of its own, so it gives High.
    pub fn of(preset: Preset) -> Self {
        match preset {
            Preset::Low => Self::low(),
            Preset::Medium => Self::medium(),
            Preset::High | Preset::Custom => Self::high(),
            Preset::Ultra => Self::ultra(),
            Preset::Auto => Self { preset: Preset::Auto, ..Self::high() },
        }
    }
    /// Values a hand-edited file could hold, made sane.
    pub fn clamped(mut self) -> Self {
        self.vegetation = if self.vegetation.is_finite() { self.vegetation.clamp(0.0, MAX_VEGETATION) } else { 1.0 };
        self.render_scale = if self.render_scale.is_finite() { self.render_scale.clamp(0.5, 1.0) } else { 1.0 };
        self.shadow_map = self.shadow_map.clamp(512, 8192).next_power_of_two().min(8192);
        self.msaa = match self.msaa {
            0 | 1 => 1,
            2 | 3 => 2,
            _ => 4,
        };
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
        if let Some(name) = options.graphics.as_deref() {
            match Preset::parse(name) {
                Some(p) => g = Self::of(p),
                None => eprintln!("--graphics {name}: low, medium, high, ultra or auto"),
            }
        } else if !options.offscreen() && !ClientSettings::path(&options.saves).exists() {
            // The very first start: measure and pick.
            g = Self::of(Preset::Auto);
        }
        let before = g.clone();
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
                "soft" | "soft-shadows" => g.soft_shadows = false,
                "all" => g = Self { preset: g.preset, vegetation: g.vegetation, shadow_map: g.shadow_map, msaa: g.msaa, render_scale: g.render_scale, ..Self::low() },
                "" => {}
                other => eprintln!("--no-post {other}: grain, tilt, vignette, bloom, ao, ground, soft or all"),
            }
        }
        if let Some(n) = options.msaa {
            g.msaa = n;
        }
        if let Some(n) = options.shadow_map {
            g.shadow_map = n;
        }
        if let Some(s) = options.render_scale {
            g.render_scale = s;
        }
        g = g.clamped();
        if g != before && g.preset != Preset::Auto {
            g.preset = Preset::Custom;
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
        assert_eq!(Graphics::from_options(&options("--graphics Medium")), Graphics::medium());
        assert_eq!(Graphics::from_options(&options("--graphics ultra")), Graphics::ultra());
        assert_eq!(Graphics::from_options(&options("--graphics auto")).preset, Preset::Auto);
        // A flag over a preset makes it Custom.
        let g = Graphics::from_options(&options("--graphics low --vegetation 1.5"));
        assert_eq!(g, Graphics { preset: Preset::Custom, vegetation: 1.5, ..Graphics::low() });
        let g = Graphics::from_options(&options("--vegetation off --no-post grain,tilt"));
        assert_eq!(g, Graphics { preset: Preset::Custom, vegetation: 0.0, grain: false, tilt_shift: false, ..Graphics::high() });
        let g = Graphics::from_options(&options("--no-post all"));
        assert_eq!(g, Graphics { preset: Preset::Custom, vegetation: 1.0, shadow_map: 4096, msaa: 4, render_scale: 0.75, ..Graphics::low() });
        let g = Graphics::from_options(&options("--msaa 1 --shadow-map 3000 --render-scale 0.2"));
        assert_eq!((g.msaa, g.shadow_map, g.render_scale, g.preset), (1, 4096, 0.5, Preset::Custom));
        // Mistakes leave the setting alone.
        assert_eq!(Graphics::from_options(&options("--vegetation lots --graphics extreme --no-post sparkle")), Graphics::high());
    }

    #[test]
    fn presets_step_down_in_cost() {
        let (l, m, h, u) = (Graphics::low(), Graphics::medium(), Graphics::high(), Graphics::ultra());
        assert!(l.vegetation < m.vegetation && m.vegetation < h.vegetation && h.vegetation < u.vegetation);
        assert!(l.shadow_map <= m.shadow_map && m.shadow_map < h.shadow_map && h.shadow_map < u.shadow_map);
        assert!(l.msaa <= m.msaa && m.msaa <= h.msaa);
        assert!(l.render_scale <= m.render_scale && m.render_scale <= h.render_scale && h.render_scale <= u.render_scale);
        for p in Preset::CHOICES {
            assert_eq!(Preset::parse(p.name()), Some(p));
            assert_eq!(Graphics::of(p).preset, p);
        }
        // High is the look the game was made to.
        assert_eq!(Graphics::default(), Graphics::high());
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
        // A file from before the presets (no preset, shadow map or MSAA): High's.
        let old = ron::from_str::<ClientSettings>("(graphics: (vegetation: 0.5, grain: true, tilt_shift: true, vignette: true, bloom: true, contact_shadows: true, ground_detail: true))").unwrap().graphics;
        assert_eq!(old, Graphics { vegetation: 0.5, ..Graphics::high() });
    }
}
