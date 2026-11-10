//! The HUD's text and backings in one place (#97's readable-text pass), so every panel reads the
//! same at 1280 x 720 and 1920 x 1080: no text smaller than [`SMALL`], body text at [`BODY`],
//! titles at [`TITLE`]; two text colours, [`TEXT`] and the quieter [`DIM`]; and dark backings
//! opaque enough ([`PANEL_ALPHA`], [`HUD_ALPHA`]) that both keep a contrast of at least 7:1 (WCAG
//! AAA) even over the brightest ground: sunlit concrete, snow-white paint (the tests check it).

use bevy::prelude::*;

/// The smallest text on any panel: hints, details, tags (UI pixels at 100% scale).
pub const SMALL: f32 = 13.0;
/// Ordinary lines and rows.
pub const BODY: f32 = 14.0;
/// A panel's title.
pub const TITLE: f32 = 18.0;

pub const TEXT: Color = Color::srgb(0.93, 0.9, 0.82);
/// Quieter text: labels, hints, what's secondary. Still readable on every backing.
pub const DIM: Color = Color::srgb(0.76, 0.74, 0.68);

/// How opaque a panel's backing is (inventory, health, skills, tooltips).
pub const PANEL_ALPHA: f32 = 0.9;
/// How opaque the always-on HUD's backing is (vitals, hotbar, the stats text): a little lighter,
/// to hide less of the world.
pub const HUD_ALPHA: f32 = 0.85;
const BACKING: (f32, f32, f32) = (0.02, 0.02, 0.03);

/// A panel's dark backing.
pub const PANEL_BG: Color = Color::srgba(BACKING.0, BACKING.1, BACKING.2, PANEL_ALPHA);
/// The always-on HUD's backing.
pub const HUD_BG: Color = Color::srgba(BACKING.0, BACKING.1, BACKING.2, HUD_ALPHA);

pub fn font(size: f32) -> TextFont {
    TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative luminance of an sRGB colour (WCAG).
    fn luminance(c: (f32, f32, f32)) -> f32 {
        let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
    }

    fn contrast(a: (f32, f32, f32), b: (f32, f32, f32)) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// The backing at `alpha` over a `world` grey behind it.
    fn over(alpha: f32, world: f32) -> (f32, f32, f32) {
        let mix = |c: f32| c * alpha + world * (1.0 - alpha);
        (mix(BACKING.0), mix(BACKING.1), mix(BACKING.2))
    }

    fn rgb(c: Color) -> (f32, f32, f32) {
        let c = c.to_srgba();
        (c.red, c.green, c.blue)
    }

    #[test]
    fn text_reads_on_every_backing_over_the_brightest_ground() {
        for (name, alpha) in [("panel", PANEL_ALPHA), ("hud", HUD_ALPHA)] {
            // Over pure white (worse than anything the world draws) and over black.
            for world in [1.0, 0.0] {
                for (what, color) in [("text", TEXT), ("dim", DIM)] {
                    let c = contrast(rgb(color), over(alpha, world));
                    assert!(c >= 7.0, "{what} on the {name} backing over {world}: {c:.1}:1");
                }
            }
        }
        // What it was before the pass: dim text on the vitals' 55% backing over bright ground.
        assert!(contrast((0.7, 0.68, 0.62), over(0.55, 1.0)) < 3.0);
    }

    #[test]
    fn no_text_size_is_below_the_smallest() {
        assert!(SMALL >= 13.0 && BODY >= SMALL && TITLE > BODY);
    }
}
