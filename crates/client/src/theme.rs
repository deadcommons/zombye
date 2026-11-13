//! The UI's skin, in one place (Linear DCM-61, on #97's readable-text pass): every panel takes its
//! text sizes and colours, its backings, buttons, slots, bars and rules from here, so the HUD,
//! the inventory, its right-click menu and Make list, the health and skills panels, the hotbar,
//! the current action, and the death and morning screens are one rough painted skin.
//!
//! **Text:** no text smaller than [`SMALL`], body text at [`BODY`], titles at [`TITLE`] (centred,
//! in capitals: [`title`]); two text colours, [`TEXT`] and the quieter [`DIM`], dark [`INK`] on the
//! light buttons, [`GOLD`] for what matters now. **Typefaces:** Barlow for text and the HUD
//! ([`font`]; [`strong`], its Medium, for what should stand out), Barlow Condensed SemiBold for
//! titles, tabs and headers ([`head`]), and Bevy's built-in monospace only for the developer's
//! read-outs ([`mono`]). Every text on a panel takes its `TextFont` from one of those four;
//! figures are tabular. Nothing is as wide as it is long any more: a layout that must know a
//! text's width asks [`width`].
//!
//! **Paint:** the textures are made here in code when the client starts ([`ThemePlugin`]; nothing
//! is loaded from a file, nothing is drawn after anyone else's art): dark charcoal sheets with
//! brushed, torn edges, slightly see-through ([`sheet`] for panels, [`strip`] for the small pieces
//! of the HUD, tooltips and menus); plain light buttons with a worn edge ([`button`], [`keycap`]);
//! hatched slots with a worn rim ([`slot`]); a dry-brushed bar ([`bar_track`], [`bar_fill`],
//! [`rule`]); and a dark dab behind a status glyph ([`dab`]). Each is a small image cut in nine
//! (`TextureSlicer`), its sides and middle repeated rather than stretched, so the brush keeps its
//! scale on a panel of any size and at any UI scale; they are built to repeat without a seam.
//! Each is one quad a node, drawn by Bevy's UI pass with everything else.
//!
//! The sheets are opaque enough that [`TEXT`] and [`DIM`] keep a contrast of at least 7:1 (WCAG
//! AAA) on them over the brightest ground the game draws, sunlit concrete, and [`TEXT`] over
//! pure white too ([`DIM`] 4.5:1 there); the tests work it out from the textures' own pixels,
//! blended in linear light as the UI is drawn.

use bevy::asset::{RenderAssetUsages, uuid_handle};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::VisualBox;
use bevy::text::{FontFeatureTag, FontSize, FontSource, FontWeight};
use bevy::ui::widget::NodeImageMode;
use std::sync::LazyLock;

// --- Text -------------------------------------------------------------------------------------

/// The smallest text on any panel: hints, details, tags (UI pixels at 100% scale).
/// (A pixel up on what the monospace had, each of them: Barlow's letters are narrower and a
/// little lower, so it takes a size more to read as well, and still takes less room across.)
pub const SMALL: f32 = 14.0;
/// Ordinary lines and rows.
pub const BODY: f32 = 15.0;
/// A panel's title.
pub const TITLE: f32 = 22.0;

pub const TEXT: Color = Color::srgb(0.95, 0.94, 0.9);
/// Quieter text: labels, hints, what's secondary. Still readable on every backing.
pub const DIM: Color = Color::srgb(0.8, 0.79, 0.74);
/// Text on a light button.
pub const INK: Color = Color::srgb(0.07, 0.07, 0.08);
/// What matters now: the key to press, the thing picked, a level gained.
pub const GOLD: Color = Color::srgb(0.97, 0.84, 0.36);
/// Something wrong: a refusal, blood.
pub const BAD: Color = Color::srgb(0.95, 0.4, 0.3);

// The typefaces (SIL OFL 1.1, assets/fonts/barlow/OFL.txt): Barlow for text, Barlow Condensed
// for titles and headers. Compiled in, like the textures: no file to find at run time, so an
// offscreen render, a test and a game started from another folder all have them.
const BARLOW: Handle<Font> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c71");
const BARLOW_MEDIUM: Handle<Font> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c72");
const CONDENSED: Handle<Font> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c73");
const BARLOW_TTF: &[u8] = include_bytes!("../../../assets/fonts/barlow/Barlow-Regular.ttf");
const BARLOW_MEDIUM_TTF: &[u8] = include_bytes!("../../../assets/fonts/barlow/Barlow-Medium.ttf");
const CONDENSED_TTF: &[u8] = include_bytes!("../../../assets/fonts/barlow/BarlowCondensed-SemiBold.ttf");
// The brand's two faces, for the screens before the game (docs/ui-design.md, The screens before
// the game): Bebas Neue (SIL OFL 1.1) for titles and buttons, Special Elite (Apache 2.0) for the
// loading lines.
const BEBAS: Handle<Font> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c74");
const ELITE: Handle<Font> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c75");
const BEBAS_TTF: &[u8] = include_bytes!("../../../assets/fonts/bebas-neue/BebasNeue-Regular.ttf");
const ELITE_TTF: &[u8] = include_bytes!("../../../assets/fonts/special-elite/SpecialElite-Regular.ttf");

/// The faces the UI is set in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    /// Barlow Regular: body text and the HUD ([`font`]).
    Text,
    /// Barlow Medium: text that should stand out ([`strong`]).
    Strong,
    /// Barlow Condensed SemiBold: titles, tabs, headers ([`head`]).
    Head,
    /// Bebas Neue: the brand's headline face, on the screens before the game ([`brand`]).
    Brand,
    /// Special Elite: the survivor's voice, the loading lines ([`note`]).
    Note,
}

impl Face {
    fn ttf(self) -> &'static [u8] {
        match self {
            Face::Text => BARLOW_TTF,
            Face::Strong => BARLOW_MEDIUM_TTF,
            Face::Head => CONDENSED_TTF,
            Face::Brand => BEBAS_TTF,
            Face::Note => ELITE_TTF,
        }
    }

    pub fn font(self, size: f32) -> TextFont {
        let (handle, weight) = match self {
            Face::Text => (BARLOW, FontWeight::NORMAL),
            Face::Strong => (BARLOW_MEDIUM, FontWeight::MEDIUM),
            Face::Head => (CONDENSED, FontWeight::SEMIBOLD),
            Face::Brand => (BEBAS, FontWeight::NORMAL),
            Face::Note => (ELITE, FontWeight::NORMAL),
        };
        // Tabular figures: every digit as wide as the next, so counts, weights and clocks line up
        // down a column and don't shuffle sideways as they change.
        TextFont { font: FontSource::Handle(handle), font_size: FontSize::Px(size), weight, font_features: [FontFeatureTag::TABULAR_FIGURES].into(), ..default() }
    }

    /// The advance of each printable ASCII character (space to `~`), in ems.
    fn advances(self) -> &'static [f32; 95] {
        static TABLES: LazyLock<[[f32; 95]; 5]> = LazyLock::new(|| [Face::Text, Face::Strong, Face::Head, Face::Brand, Face::Note].map(|f| advances(f.ttf()).unwrap_or([0.6; 95])));
        &TABLES[self as usize]
    }
}

/// Body text and the HUD: Barlow.
pub fn font(size: f32) -> TextFont {
    Face::Text.font(size)
}

/// Text that should stand out from the lines round it: Barlow Medium.
pub fn strong(size: f32) -> TextFont {
    Face::Strong.font(size)
}

/// Titles, tabs and section headers: Barlow Condensed SemiBold. Made for capitals.
pub fn head(size: f32) -> TextFont {
    Face::Head.font(size)
}

/// Titles and buttons on the screens before the game (the menus, settings, new game): Bebas
/// Neue, the brand's headline face. Capitals only.
pub fn brand(size: f32) -> TextFont {
    Face::Brand.font(size)
}

/// The survivor's voice, a line or two at most (the loading lines): Special Elite.
pub fn note(size: f32) -> TextFont {
    Face::Note.font(size)
}

/// Bevy's built-in monospace, for the developer's read-outs (the stats line, the debug stats
/// window, the light preview): columns of numbers that are lined up with spaces.
pub fn mono(size: f32) -> TextFont {
    TextFont { font_size: FontSize::Px(size), ..default() }
}

/// How wide `text` is at `size` in `face`, in UI pixels, from the face's own advance widths: for
/// the few places that must know a width before the layout does (a tab turned on its side, a
/// slot that must hold a word). Digits count at the widest digit (they are set tabular); kerning
/// isn't counted, and a character outside ASCII counts as a wide letter.
pub fn width(face: Face, text: &str, size: f32) -> f32 {
    let table = face.advances();
    let digit = table[(b'0' - 32) as usize..=(b'9' - 32) as usize].iter().cloned().fold(0.0, f32::max);
    let em: f32 = text.chars().map(|c| if c.is_ascii_digit() { digit } else if (' '..='~').contains(&c) { table[c as usize - 32] } else { 0.7 }).sum();
    em * size
}

/// The advances of printable ASCII in a TrueType file, in ems: its `cmap` (the format 4 table
/// every font has for the basic plane) to find each glyph, `hmtx` for its advance, `head` for
/// the units to an em.
fn advances(ttf: &[u8]) -> Option<[f32; 95]> {
    let u16_at = |o: usize| ttf.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize);
    let u32_at = |o: usize| ttf.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let table = |tag: &[u8; 4]| (0..u16_at(4)?).map(|i| 12 + 16 * i).find(|&r| ttf.get(r..r + 4) == Some(&tag[..])).and_then(|r| u32_at(r + 8));
    let em = u16_at(table(b"head")? + 18)? as f32;
    let metrics = u16_at(table(b"hhea")? + 34)?;
    let (hmtx, cmap) = (table(b"hmtx")?, table(b"cmap")?);
    let sub = (0..u16_at(cmap + 2)?).filter_map(|i| u32_at(cmap + 8 + 8 * i)).map(|o| cmap + o).find(|&s| u16_at(s) == Some(4))?;
    let segs = u16_at(sub + 6)? / 2;
    let (ends, starts, deltas, offsets) = (sub + 14, sub + 16 + 2 * segs, sub + 16 + 4 * segs, sub + 16 + 6 * segs);
    let glyph = |c: usize| -> Option<usize> {
        let s = (0..segs).find(|&s| u16_at(ends + 2 * s).is_some_and(|e| e >= c))?;
        let start = u16_at(starts + 2 * s)?;
        if start > c {
            return Some(0);
        }
        let (delta, off) = (u16_at(deltas + 2 * s)?, u16_at(offsets + 2 * s)?);
        if off == 0 {
            return Some((c + delta) & 0xFFFF);
        }
        let g = u16_at(offsets + 2 * s + off + 2 * (c - start))?;
        Some(if g == 0 { 0 } else { (g + delta) & 0xFFFF })
    };
    let mut out = [0.0; 95];
    for (k, w) in out.iter_mut().enumerate() {
        // (Glyphs past the last full metric share its advance.)
        let g = glyph(32 + k)?.min(metrics.checked_sub(1)?);
        *w = u16_at(hmtx + 4 * g)? as f32 / em;
    }
    (em > 0.0).then_some(out)
}

// --- Flat tints (rows, hairlines: things there are hundreds of stay plain quads) ---------------

/// The charcoal of the sheets, as a flat colour: for a backing on a line of text itself, which
/// can't carry a texture.
const CHARCOAL: (f32, f32, f32) = (0.085, 0.085, 0.095);
/// How opaque a panel's sheet is: slightly see-through. (The UI is blended in linear light, where
/// a few per cent of sunlit ground behind a dark sheet is a lot: at 90% a panel over a pale wall
/// is a mid grey. The tests work the contrast out the way it is drawn.)
pub const PANEL_ALPHA: f32 = 0.965;
/// How opaque the small pieces of the HUD are: a little lighter, to hide less of the world.
pub const HUD_ALPHA: f32 = 0.96;
const HUD_CHARCOAL: (f32, f32, f32) = (0.07, 0.07, 0.078);
/// A flat panel backing (the debug windows, a line of text's own backing).
pub const PANEL_BG: Color = Color::srgba(CHARCOAL.0, CHARCOAL.1, CHARCOAL.2, PANEL_ALPHA);
/// A flat HUD backing.
pub const HUD_BG: Color = Color::srgba(HUD_CHARCOAL.0, HUD_CHARCOAL.1, HUD_CHARCOAL.2, HUD_ALPHA);

/// A row at rest, under the pointer, and picked.
pub const ROW: Color = Color::srgba(1.0, 1.0, 1.0, 0.03);
pub const ROW_HOVER: Color = Color::srgba(0.95, 0.93, 0.85, 0.2);
pub const ROW_PICKED: Color = Color::srgba(0.97, 0.84, 0.36, 0.26);
/// A pane set into a sheet: a shade darker.
#[allow(dead_code)]
pub const INSET: Color = Color::srgba(0.0, 0.0, 0.0, 0.38);
/// A hairline.
#[allow(dead_code)]
pub const LINE: Color = Color::srgba(0.95, 0.94, 0.9, 0.16);

// --- The textures -----------------------------------------------------------------------------

const SHEET: Handle<Image> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c61");
const STRIP: Handle<Image> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c62");
const BUTTON: Handle<Image> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c63");
const SLOT: Handle<Image> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c64");
const BAR: Handle<Image> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c65");
const DAB: Handle<Image> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c66");
const GRAIN: Handle<Image> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c67");
/// The grain's tile, texels a side.
const GRAIN_SIZE: u32 = 256;

/// A sheet of paint cut in nine: `border` texels of ragged edge each side, and a middle of
/// `period` texels that repeats.
#[derive(Clone, Copy, Debug)]
struct Paint {
    border: u32,
    period: u32,
    /// How far in the paint starts along the left and right edges (texels: least, most), where
    /// the strokes end, and along the top and bottom, where they run.
    side: (f32, f32),
    top: (f32, f32),
    /// How far in from where it starts the paint is still thin (the bristles miss in streaks).
    dry: f32,
    color: (f32, f32, f32),
    alpha: f32,
    /// How much the strokes differ in lightness.
    grain: f32,
    seed: u32,
}

const SHEET_PAINT: Paint = Paint { border: 32, period: 64, side: (2.0, 12.0), top: (1.5, 6.0), dry: 9.0, color: CHARCOAL, alpha: PANEL_ALPHA, grain: 0.03, seed: 11 };
const STRIP_PAINT: Paint = Paint { border: 12, period: 40, side: (1.0, 6.0), top: (0.5, 2.5), dry: 5.0, color: HUD_CHARCOAL, alpha: HUD_ALPHA, grain: 0.025, seed: 23 };
/// (White, to be tinted: a button's colour is its `ImageNode`'s.)
const BUTTON_PAINT: Paint = Paint { border: 8, period: 32, side: (0.4, 3.0), top: (0.3, 1.8), dry: 4.0, color: (0.93, 0.93, 0.93), alpha: 1.0, grain: 0.12, seed: 37 };

const SLOT_BORDER: u32 = 10;
const SLOT_PERIOD: u32 = 44;
const BAR_SIZE: (u32, u32) = (64, 16);
const BAR_BORDER: (f32, f32) = (8.0, 3.0);
const DAB_SIZE: u32 = 64;

/// A button's colour at rest and when it is the one that's on.
pub const BUTTON_REST: Color = Color::srgb(0.8, 0.79, 0.75);
pub const BUTTON_ON: Color = GOLD;
/// A slot's tint at rest, empty, and under the pointer (the texture is light, to be tinted down).
pub const SLOT_REST: Color = Color::srgb(0.62, 0.62, 0.64);
pub const SLOT_EMPTY: Color = Color::srgba(0.5, 0.5, 0.52, 0.6);
pub const SLOT_HOVER: Color = Color::srgb(1.0, 0.96, 0.84);

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / 16_777_216.0
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smooth noise, 0..1, in cells of about `cell` texels, the same again every `period` texels both
/// ways (the cells are fitted to it), so a texture made of it repeats without a seam.
fn noise(x: f32, y: f32, cell: (f32, f32), period: f32, seed: u32) -> f32 {
    let (nx, ny) = ((period / cell.0).round().max(1.0), (period / cell.1).round().max(1.0));
    let (fx, fy) = (x * nx / period, y * ny / period);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let at = |i: i32, j: i32| hash((x0 as i32 + i).rem_euclid(nx as i32), (y0 as i32 + j).rem_euclid(ny as i32), seed);
    let (a, b) = (at(0, 0) + (at(1, 0) - at(0, 0)) * sx, at(0, 1) + (at(1, 1) - at(0, 1)) * sx);
    a + (b - a) * sy
}

/// Pixels as sRGB bytes, straight alpha.
struct Canvas {
    w: u32,
    h: u32,
    data: Vec<u8>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Canvas {
        Canvas { w, h, data: Vec::with_capacity((w * h * 4) as usize) }
    }

    /// The next pixel, row by row.
    fn push(&mut self, c: (f32, f32, f32), a: f32) {
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        self.data.extend_from_slice(&[b(c.0), b(c.1), b(c.2), b(a)]);
    }

    /// Red, green, blue and alpha at (`x`, `y`), 0..1 (tests).
    #[cfg(test)]
    fn at(&self, x: u32, y: u32) -> [f32; 4] {
        let i = ((y * self.w + x) * 4) as usize;
        [0, 1, 2, 3].map(|k| self.data[i + k] as f32 / 255.0)
    }

    fn image(self) -> Image {
        Image::new(Extent3d { width: self.w, height: self.h, depth_or_array_layers: 1 }, TextureDimension::D2, self.data, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD)
    }
}

/// A sheet of paint laid on with a wide dry brush, left to right: the strokes end raggedly at the
/// sides, each few rows somewhere else, and thin out there; the top and bottom are a rough line.
/// The colour is the same everywhere (only the alpha makes the shape), so filtering leaves no halo.
fn paint(p: &Paint) -> Canvas {
    let size = 2 * p.border + p.period;
    let (b, period) = (p.border as f32, p.period as f32);
    let mut c = Canvas::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Where we are in the repeating middle (the borders carry on from it).
            let (u, v) = ((px - b).rem_euclid(period), (py - b).rem_euclid(period));
            let (right, bottom) = (px > size as f32 / 2.0, py > size as f32 / 2.0);
            let dx = if right { size as f32 - px } else { px };
            let dy = if bottom { size as f32 - py } else { py };
            // The side this stroke ends at: bristle by bristle, and torn in longer waves.
            let s = p.seed + 20 * right as u32;
            let ragged = 0.4 * noise(0.0, v, (period, 2.0), period, s).powf(1.6) + 0.3 * noise(0.0, v, (period, 5.0), period, s + 1) + 0.3 * noise(0.0, v, (period, 16.0), period, s + 2);
            let ex = p.side.0 + (p.side.1 - p.side.0) * ragged;
            let s = p.seed + 4 + 20 * bottom as u32;
            let ey = p.top.0 + (p.top.1 - p.top.0) * (0.5 * noise(u, 0.0, (3.0, period), period, s) + 0.5 * noise(u, 0.0, (18.0, period), period, s + 2));
            let mut a = (dx - ex).clamp(0.0, 1.0).min((dy - ey).clamp(0.0, 1.0));
            // Dry brush: toward the ends of the strokes the bristles miss in long streaks.
            let streak = noise(u, v, (22.0, 1.6), period, p.seed + 8);
            let thin = 1.0 - ((dx - ex) / p.dry).clamp(0.0, 1.0);
            a *= 1.0 - 0.7 * thin * (1.0 - smooth(0.35, 0.55, streak));
            let thin = 1.0 - ((dy - ey) / (p.dry * 0.4)).clamp(0.0, 1.0);
            a *= 1.0 - 0.45 * thin * (1.0 - smooth(0.3, 0.5, noise(u, v, (9.0, 1.6), period, p.seed + 9)));
            // The strokes differ a little in how much paint they carried.
            let k = p.grain * ((noise(u, v, (32.0, 2.0), period, p.seed + 10) - 0.5) + 0.5 * (noise(u, v, (12.0, 12.0), period, p.seed + 11) - 0.5));
            c.push((p.color.0 + k, p.color.1 + k, p.color.2 + k), a * p.alpha);
        }
    }
    c
}

/// A slot: a square of lighter charcoal with a fine cross-hatch, as if scored, and a pale rim
/// worn through in places. Light, to be tinted down (and up again under the pointer).
fn slot_pixels() -> Canvas {
    let size = 2 * SLOT_BORDER + SLOT_PERIOD;
    let (b, period) = (SLOT_BORDER as f32, SLOT_PERIOD as f32);
    let mut c = Canvas::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let (u, v) = ((px - b).rem_euclid(period), (py - b).rem_euclid(period));
            let d = px.min(py).min(size as f32 - px).min(size as f32 - py);
            // Along the edge: how far round we are, for the rim's wear.
            let along = if (px.min(size as f32 - px)) < (py.min(size as f32 - py)) { v } else { u + 17.0 };
            let wear = noise(along, 0.0, (5.0, period), period, 51);
            let (ui, vi) = (u as i32, v as i32);
            let hatch = if (ui + vi).rem_euclid(4) == 0 || (ui - vi).rem_euclid(4) == 0 { 0.11 } else { 0.0 };
            let mut k = 0.3 + hatch + 0.06 * (noise(u, v, (11.0, 11.0), period, 52) - 0.5) + 0.03 * (hash(ui, vi, 53) - 0.5);
            if d < 2.4 && d > 0.6 && wear > 0.3 {
                k = 0.62 + 0.2 * (wear - 0.3);
            }
            let a = (d - 1.4 * noise(along, 0.0, (2.0, period), period, 54)).clamp(0.0, 1.0) * 0.94;
            c.push((k, k, k * 1.03), a);
        }
    }
    c
}

/// One stroke of a dry brush, white, to be tinted: ragged at both ends, rough along its top and
/// bottom, thinner where the bristles carried less.
fn bar_pixels() -> Canvas {
    let (w, h) = BAR_SIZE;
    let mut c = Canvas::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let dx = px.min(w as f32 - px);
            let dy = py.min(h as f32 - py);
            let end = 0.5 + 5.5 * hash(y as i32, (px > w as f32 / 2.0) as i32, 61);
            let edge = 0.3 + 2.2 * noise(px, 0.0, (4.0, 64.0), 64.0, 62 + (py > h as f32 / 2.0) as u32);
            let body = 0.72 + 0.28 * noise(px, py, (16.0, 1.5), 64.0, 64);
            c.push((1.0, 1.0, 1.0), (dx - end).clamp(0.0, 1.0).min((dy - edge).clamp(0.0, 1.0)) * body);
        }
    }
    c
}

/// A dab of paint, white, to be tinted: a rough round blot, for behind a status glyph.
fn dab_pixels() -> Canvas {
    let size = DAB_SIZE;
    let mut c = Canvas::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = ((x as f32 + 0.5) / size as f32 * 2.0 - 1.0, (y as f32 + 0.5) / size as f32 * 2.0 - 1.0);
            let r = (px * px + py * py).sqrt();
            let turn = (py.atan2(px) / std::f32::consts::TAU + 0.5) * 96.0;
            let reach = 0.74 + 0.2 * noise(turn, 0.0, (8.0, 96.0), 96.0, 71) + 0.05 * noise(turn, 0.0, (2.0, 96.0), 96.0, 72);
            let streak = noise((px + 1.0) * 32.0, (py + 1.0) * 32.0, (20.0, 1.8), 64.0, 73);
            let thin = smooth(reach - 0.4, reach, r);
            let a = (1.0 - smooth(reach - 0.07, reach, r)) * (1.0 - 0.6 * thin * (1.0 - smooth(0.3, 0.5, streak)));
            c.push((1.0, 1.0, 1.0), a);
        }
    }
    c
}

/// The grain of an old print, to lay over a screen's ground: fine pale specks and dark ones.
/// Repeats without a seam (and has nothing big enough to show that it repeats).
fn grain_pixels() -> Canvas {
    let n = GRAIN_SIZE;
    let mut c = Canvas::new(n, n);
    for y in 0..n {
        for x in 0..n {
            let speck = hash(x as i32, y as i32, 82);
            let pale = smooth(0.95, 1.0, speck) * 0.07;
            let dark = smooth(0.06, 0.0, speck) * 0.12;
            if pale >= dark {
                c.push((0.914, 0.847, 0.733), pale);
            } else {
                c.push((0.0, 0.0, 0.0), dark);
            }
        }
    }
    c
}

/// Makes the skin's textures. Add it before the UI's plugins (it needs Bevy's image assets).
pub struct ThemePlugin;

impl Plugin for ThemePlugin {
    fn build(&self, app: &mut App) {
        if let Some(mut fonts) = app.world_mut().get_resource_mut::<Assets<Font>>() {
            for (handle, face) in [(BARLOW, Face::Text), (BARLOW_MEDIUM, Face::Strong), (CONDENSED, Face::Head), (BEBAS, Face::Brand), (ELITE, Face::Note)] {
                let _ = fonts.insert(handle.id(), Font::from_bytes(face.ttf().to_vec()));
            }
        }
        let Some(mut images) = app.world_mut().get_resource_mut::<Assets<Image>>() else { return };
        for (handle, canvas) in [(SHEET, paint(&SHEET_PAINT)), (STRIP, paint(&STRIP_PAINT)), (BUTTON, paint(&BUTTON_PAINT)), (SLOT, slot_pixels()), (BAR, bar_pixels()), (DAB, dab_pixels()), (GRAIN, grain_pixels())] {
            let _ = images.insert(handle.id(), canvas.image());
        }
        flat::load(&mut images);
    }
}

// --- What panels put on their nodes --------------------------------------------------------------

/// `image` cut in nine, `border` texels each side, drawn over the whole of its node (padding
/// included), sides and middle repeated at their own size.
fn sliced(image: Handle<Image>, border: BorderRect, color: Color, tile: bool) -> ImageNode {
    let mode = if tile { SliceScaleMode::Tile { stretch_value: 1.0 } } else { SliceScaleMode::Stretch };
    ImageNode {
        image,
        color,
        image_mode: NodeImageMode::Sliced(TextureSlicer { border, center_scale_mode: mode, sides_scale_mode: mode, max_corner_scale: 1.0 }),
        visual_box: VisualBox::BorderBox,
        ..default()
    }
}

/// The grain of an old print, repeated over its whole node: lay it over the ground of a screen
/// before the game (the brand's side), under everything else on it.
pub fn grain() -> ImageNode {
    ImageNode { image: GRAIN, image_mode: NodeImageMode::Tiled { tile_x: true, tile_y: true, stretch_value: 1.0 }, ..default() }
}

/// A panel's backing: a charcoal sheet with brushed, torn edges. Give the node [`SHEET_PAD`] of
/// padding, so nothing stands on the ragged part.
pub fn sheet() -> ImageNode {
    sheet_tinted(Color::WHITE)
}

/// [`sheet`] with a tint (the death screen's is a little red).
pub fn sheet_tinted(color: Color) -> ImageNode {
    sliced(SHEET, BorderRect::all(SHEET_PAINT.border as f32), color, true)
}

/// The backing of a small piece of the HUD, a tooltip or a menu: the same paint, finer.
pub fn strip() -> ImageNode {
    sliced(STRIP, BorderRect::all(STRIP_PAINT.border as f32), Color::WHITE, true)
}

/// The padding a [`sheet`] needs (UI pixels: sides, then top and bottom) to keep what's on it
/// off its ragged edge, and a [`strip`]'s.
pub const SHEET_PAD: (f32, f32) = (22.0, 16.0);
pub const STRIP_PAD: (f32, f32) = (12.0, 8.0);

/// A plain light button with a worn edge; `on`: the one that's chosen. Its text is [`INK`].
pub fn button(on: bool) -> ImageNode {
    sliced(BUTTON, BorderRect::all(BUTTON_PAINT.border as f32), if on { BUTTON_ON } else { BUTTON_REST }, true)
}

/// A hotbar or item slot, tinted `color` ([`SLOT_REST`], [`SLOT_EMPTY`], [`SLOT_HOVER`]).
pub fn slot(color: Color) -> ImageNode {
    // (Its middle stretched, not repeated: a slot is about the texture's own size, and a repeat
    // squeezed to fit would show where the hatch starts again.)
    sliced(SLOT, BorderRect::all(SLOT_BORDER as f32), color, false)
}

fn brush(color: Color) -> ImageNode {
    sliced(BAR, BorderRect::axes(BAR_BORDER.0, BAR_BORDER.1), color, false)
}

/// The empty length of a bar (at least 8 UI pixels high): one pale dry stroke.
pub fn bar_track() -> ImageNode {
    brush(Color::srgba(0.95, 0.94, 0.9, 0.2))
}

/// The filled length of a bar, in `color`: a child of the [`bar_track`] node, as wide as its share.
pub fn bar_fill(color: Color) -> ImageNode {
    brush(color)
}

/// A thin brushed line across a panel (on a node 2 or 3 UI pixels high).
pub fn rule() -> ImageNode {
    ImageNode { image: BAR, color: Color::srgba(0.95, 0.94, 0.9, 0.3), image_mode: NodeImageMode::Stretch, visual_box: VisualBox::BorderBox, ..default() }
}

/// A dab of paint in `color` behind a status glyph.
pub fn dab(color: Color) -> ImageNode {
    ImageNode { image: DAB, color, image_mode: NodeImageMode::Stretch, visual_box: VisualBox::BorderBox, ..default() }
}

/// A brushed line across the panel, with room above and below.
pub fn spawn_rule(c: &mut ChildSpawnerCommands) {
    c.spawn((Node { width: Val::Percent(100.0), height: px(3), flex_shrink: 0.0, margin: UiRect::vertical(px(8)), ..default() }, rule(), Pickable::IGNORE));
}

/// A panel's title: centred, in capitals, over a short brushed stroke.
pub fn title(c: &mut ChildSpawnerCommands, text: &str) {
    c.spawn((Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Center, align_self: AlignSelf::Stretch, row_gap: px(5), margin: UiRect::bottom(px(6)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|t| {
        t.spawn((Text::new(text.to_uppercase()), head(TITLE), TextColor(TEXT), TextLayout::no_wrap(), Pickable::IGNORE));
        t.spawn((Node { width: px(96), height: px(3), ..default() }, rule(), Pickable::IGNORE));
    });
}

/// A key to press, as a small light button, then what it does: "[Enter] carry on".
pub fn keycap(c: &mut ChildSpawnerCommands, key: &str, does: &str) {
    c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), ..default() }, Pickable::IGNORE)).with_children(|row| {
        row.spawn((Node { padding: UiRect::axes(px(10), px(4)), justify_content: JustifyContent::Center, min_width: px(34), ..default() }, button(false), Pickable::IGNORE)).with_children(|k| {
            k.spawn((Text::new(key.to_string()), strong(BODY), TextColor(INK), TextLayout::no_wrap(), Pickable::IGNORE));
        });
        row.spawn((Text::new(does.to_string()), font(BODY), TextColor(TEXT), Pickable::IGNORE));
    });
}

// --- The flat skin (docs/ui-design.md) ------------------------------------------------------------

/// The new UI's flat chrome (docs/ui-design.md): tokens and the few pieces every screen of the
/// frame is built from. Screens move to it one at a time; the painted pieces above stay for the
/// ones that haven't yet.
pub mod flat {
    use super::*;

    /// The frame's backing over the world. The guide's "ink at 86%" is how it looks; the UI is
    /// blended in linear light, where that takes this much (as the painted sheets found, #97).
    pub const VEIL: Color = Color::srgba(0.078, 0.078, 0.078, 0.975);
    /// A floating piece (a menu, a hover card): solid ink.
    pub const SOLID: Color = Color::srgb(0.086, 0.086, 0.086);
    pub const BONE: Color = Color::srgb(0.949, 0.902, 0.788);
    pub const DIM: Color = Color::srgb(0.776, 0.741, 0.651);
    /// Only for what needn't be read to play: the label of an empty slot.
    pub const FAINT: Color = Color::srgb(0.49, 0.467, 0.416);
    pub const LINE: Color = Color::srgba(0.949, 0.902, 0.788, 0.11);
    pub const CELL: Color = Color::srgba(0.949, 0.902, 0.788, 0.045);
    pub const CELL_HOVER: Color = Color::srgba(0.949, 0.902, 0.788, 0.12);
    pub const GREEN: Color = Color::srgb(0.553, 0.69, 0.42);
    pub const GREEN_SOFT: Color = Color::srgba(0.553, 0.69, 0.42, 0.16);
    pub const AMBER: Color = Color::srgb(0.878, 0.69, 0.29);
    pub const RED: Color = Color::srgb(0.843, 0.149, 0.169);
    /// Red as text on ink (the accent itself is too dark to read there).
    pub const RED_TEXT: Color = Color::srgb(0.95, 0.42, 0.38);
    pub const INK: Color = Color::srgb(0.078, 0.078, 0.078);

    /// Text sizes: a screen's name, a column's title, a pane's name, body, small.
    pub const DISPLAY: f32 = 30.0;
    pub const TITLE: f32 = 22.0;
    pub const HEAD: f32 = 17.0;
    pub const BODY: f32 = 15.0;
    pub const SMALL: f32 = 14.0;

    /// The frame: top bar, rail and footer; a tile and a compact row.
    pub const TOP_BAR: f32 = 76.0;
    pub const RAIL: f32 = 92.0;
    pub const FOOTER: f32 = 46.0;
    pub const TILE: f32 = 104.0;
    pub const RADIUS: f32 = 6.0;

    const ICONS: Handle<Image> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0c81");
    const ICON_PX: f32 = 96.0;
    const ICON_COLUMNS: usize = 10;
    const ICON_IDS: &str = include_str!("../../../assets/ui/item_icons.txt");

    /// The item icons (`assets/ui/item_icons.png`), compiled in like the typefaces.
    pub(super) fn load(images: &mut Assets<Image>) {
        use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
        let bytes = include_bytes!("../../../assets/ui/item_icons.png");
        match Image::from_buffer(bytes, ImageType::Extension("png"), CompressedImageFormats::NONE, true, ImageSampler::linear(), RenderAssetUsages::default()) {
            Ok(image) => {
                let _ = images.insert(ICONS.id(), image);
            }
            Err(e) => eprintln!("item icons: {e}"),
        }
    }

    /// Where the icon of the item `id` is in the sheet, if it has one.
    pub fn icon_cell(id: &str) -> Option<Rect> {
        let k = ICON_IDS.lines().position(|l| l.trim() == id)?;
        let at = Vec2::new((k % ICON_COLUMNS) as f32, (k / ICON_COLUMNS) as f32) * ICON_PX;
        Some(Rect::from_corners(at, at + Vec2::splat(ICON_PX)))
    }

    /// The icon of the item `id`, tinted `color` (white: as drawn). `None`: it has none (a mod's
    /// item), and the caller draws the item's colour instead.
    pub fn icon(id: &str, color: Color) -> Option<ImageNode> {
        icon_cell(id).map(|rect| ImageNode { image: ICONS, rect: Some(rect), color, ..default() })
    }

    /// An item's picture, `size` square: its icon, else a block of its own colour.
    pub fn item_icon(c: &mut ChildSpawnerCommands, def: Option<&zombye_sim::ItemDef>, size: f32, alpha: f32) {
        let node = Node { width: px(size), height: px(size), flex_shrink: 0.0, ..default() };
        match def.and_then(|d| icon(&d.id, Color::srgba(1.0, 1.0, 1.0, alpha))) {
            Some(image) => {
                c.spawn((node, image, Pickable::IGNORE));
            }
            None => {
                let colour = def.map_or(FAINT, |d| Color::srgba(d.color.0, d.color.1, d.color.2, alpha));
                c.spawn((node, Pickable::IGNORE)).with_children(|b| {
                    b.spawn((Node { position_type: PositionType::Absolute, left: percent(22), right: percent(22), top: percent(22), bottom: percent(22), border_radius: BorderRadius::all(px(4)), ..default() }, BackgroundColor(colour), Pickable::IGNORE));
                });
            }
        }
    }

    /// A key, as a small outlined cap: "[Tab]".
    pub fn key(c: &mut ChildSpawnerCommands, key: &str) {
        c.spawn((Node { padding: UiRect::axes(px(6), px(1)), min_width: px(22), justify_content: JustifyContent::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(4)), flex_shrink: 0.0, ..default() }, BorderColor::all(Color::srgba(0.949, 0.902, 0.788, 0.3)), BackgroundColor(CELL), Pickable::IGNORE)).with_children(|k| {
            k.spawn((Text::new(key.to_string()), strong(SMALL), TextColor(BONE), TextLayout::no_wrap(), Pickable::IGNORE));
        });
    }

    /// A key and what it does, for the footer: "[Click] move".
    pub fn hint(c: &mut ChildSpawnerCommands, keys: &[&str], does: &str) {
        // (Not with the help text off: `help_text.rs`.)
        if !crate::help_text::keys() {
            return;
        }
        c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(5), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|r| {
            for k in keys {
                key(r, k);
            }
            r.spawn((Text::new(does.to_string()), font(SMALL), TextColor(DIM), TextLayout::no_wrap(), Node { margin: UiRect::left(px(3)), ..default() }, Pickable::IGNORE));
        });
    }

    /// A chip or small button: a caps label in a rounded cell; `on`: the accent. The caller
    /// observes its presses.
    pub fn chip<'a>(parent: &'a mut ChildSpawnerCommands, text: &str, on: bool) -> EntityCommands<'a> {
        let rest = if on { GREEN } else { CELL };
        let mut e = parent.spawn((
            Node { padding: UiRect::axes(px(11), px(5)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(RADIUS)), flex_shrink: 0.0, ..default() },
            BorderColor::all(if on { GREEN } else { LINE }),
            BackgroundColor(rest),
        ));
        e.with_children(|b| {
            b.spawn((Text::new(text.to_uppercase()), head(BODY), TextColor(if on { INK } else { BONE }), TextLayout::no_wrap(), Pickable::IGNORE));
        });
        if !on {
            hover(&mut e, rest);
        }
        e
    }

    /// A wide button (the main action of a column is `main`: filled with the accent).
    pub fn button<'a>(parent: &'a mut ChildSpawnerCommands, text: &str, main: bool) -> EntityCommands<'a> {
        let rest = if main { GREEN } else { CELL };
        let mut e = parent.spawn((
            Node { flex_grow: 1.0, flex_basis: px(0), height: px(38), justify_content: JustifyContent::Center, align_items: AlignItems::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(RADIUS)), ..default() },
            BorderColor::all(if main { GREEN } else { LINE }),
            BackgroundColor(rest),
        ));
        e.with_children(|b| {
            b.spawn((Text::new(text.to_uppercase()), head(HEAD), TextColor(if main { INK } else { BONE }), TextLayout::no_wrap(), Pickable::IGNORE));
        });
        if !main {
            hover(&mut e, rest);
        }
        e
    }

    /// Lit while the pointer is on it, back to `rest` after.
    pub fn hover(e: &mut EntityCommands, rest: Color) {
        e.observe(|ev: On<PointerOver>, mut q: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = q.get_mut(ev.entity) {
                bg.0 = CELL_HOVER;
            }
        })
        .observe(move |ev: On<PointerOut>, mut q: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = q.get_mut(ev.entity) {
                bg.0 = rest;
            }
        });
    }

    /// A thin bar, `share` of it filled in `color`.
    pub fn bar(c: &mut ChildSpawnerCommands, share: f32, color: Color, node: Node) {
        c.spawn((Node { height: px(3), border_radius: BorderRadius::all(px(2)), flex_shrink: 0.0, ..node }, BackgroundColor(LINE), Pickable::IGNORE)).with_children(|b| {
            b.spawn((Node { width: Val::Percent(share.clamp(0.0, 1.0) * 100.0), height: Val::Percent(100.0), border_radius: BorderRadius::all(px(2)), ..default() }, BackgroundColor(color), Pickable::IGNORE));
        });
    }

    /// A hairline across a column.
    pub fn line(c: &mut ChildSpawnerCommands) {
        c.spawn((Node { height: px(1), flex_shrink: 0.0, ..default() }, BackgroundColor(LINE), Pickable::IGNORE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lin(v: f32) -> f32 {
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    }

    /// Relative luminance of an sRGB colour (WCAG).
    fn luminance(c: (f32, f32, f32)) -> f32 {
        0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
    }

    /// The contrast of two luminances (WCAG).
    fn contrast(la: f32, lb: f32) -> f32 {
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// The luminance of a backing of sRGB colour `c` at `alpha` over a `world` grey (sRGB) behind
    /// it, blended as the UI is drawn: in linear light.
    fn over(c: (f32, f32, f32), alpha: f32, world: f32) -> f32 {
        luminance(c) * alpha + lin(world) * (1.0 - alpha)
    }

    fn rgb(c: Color) -> (f32, f32, f32) {
        let c = c.to_srgba();
        (c.red, c.green, c.blue)
    }

    /// The brightest ground the game draws behind a panel: sunlit concrete after the tone map.
    const SUNLIT: f32 = 0.85;

    /// Text on a backing: 7:1 (AAA) over the brightest ground, and over pure white (worse than
    /// anything the world draws) still 7:1 for the main text and 4.5:1 (AA) for the quiet one.
    fn reads(name: &str, c: (f32, f32, f32), alpha: f32) {
        for (what, color, on_white) in [("text", TEXT, 7.0), ("dim", DIM, 4.5)] {
            let l = luminance(rgb(color));
            for (world, least) in [(0.0, 7.0), (SUNLIT, 7.0), (1.0, on_white)] {
                let got = contrast(l, over(c, alpha, world));
                assert!(got >= least, "{what} on the {name} over {world}: {got:.1}:1");
            }
        }
    }

    /// The texels of a painted sheet where text may stand: inside its padding.
    fn inside(p: &Paint, pad: (f32, f32)) -> Vec<[f32; 4]> {
        let c = paint(p);
        let (px, py) = (pad.0 as u32, pad.1 as u32);
        (py..c.h - py).flat_map(|y| (px..c.w - px).map(move |x| (x, y))).map(|(x, y)| c.at(x, y)).collect()
    }

    #[test]
    fn text_reads_on_every_backing_over_the_brightest_ground() {
        // Every texel text can stand on.
        for (name, p, pad) in [("sheet", SHEET_PAINT, SHEET_PAD), ("strip", STRIP_PAINT, STRIP_PAD)] {
            for t in inside(&p, pad) {
                reads(name, (t[0], t[1], t[2]), t[3]);
            }
        }
        for (name, bg) in [("flat panel", PANEL_BG), ("flat hud", HUD_BG)] {
            let b = bg.to_srgba();
            reads(name, (b.red, b.green, b.blue), b.alpha);
        }
        // Ink on a button, at rest and on, whatever the grain (the tint multiplies in linear light).
        let b = paint(&BUTTON_PAINT);
        let darkest = (8..b.h - 8).flat_map(|y| (8..b.w - 8).map(move |x| (x, y))).map(|(x, y)| b.at(x, y)[0]).fold(1.0, f32::min);
        for tint in [BUTTON_REST, BUTTON_ON] {
            let c = contrast(luminance(rgb(INK)), luminance(rgb(tint)) * lin(darkest));
            assert!(c >= 7.0, "ink on a button: {c:.1}:1");
        }
        // What it was before #97's pass: dim text on the vitals' 55% backing over bright ground.
        assert!(contrast(luminance((0.7, 0.68, 0.62)), over((0.02, 0.02, 0.03), 0.55, 1.0)) < 3.0);
    }

    /// The flat frame's text reads over the brightest ground too, and every shipped item has
    /// its icon in the sheet.
    #[test]
    fn the_flat_frame_reads_and_every_item_has_an_icon() {
        let veil = flat::VEIL.to_srgba();
        for (what, color, least) in [("bone", flat::BONE, 7.0), ("dim", flat::DIM, 7.0), ("green", flat::GREEN, 4.5), ("amber", flat::AMBER, 4.5), ("red text", flat::RED_TEXT, 4.5)] {
            for world in [0.0, SUNLIT] {
                let got = contrast(luminance(rgb(color)), over((veil.red, veil.green, veil.blue), veil.alpha, world));
                assert!(got >= least, "{what} on the veil over {world}: {got:.1}:1");
            }
        }
        assert!(flat::SMALL >= SMALL);
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let items = zombye_net::content::load_config(&data, 1).unwrap().items;
        let missing: Vec<&str> = items.iter().filter(|d| flat::icon_cell(&d.id).is_none()).map(|d| d.id.as_str()).collect();
        assert!(missing.is_empty(), "items with no icon: {missing:?}");
    }

    #[test]
    fn no_text_size_is_below_the_smallest() {
        assert!(SMALL >= 14.0 && BODY >= SMALL && TITLE > BODY);
    }

    /// The faces' own tables are read: letters differ in width (nothing is monospace any more),
    /// the condensed face is the narrowest and all of them narrower than the built-in monospace's
    /// 0.62 em a letter, which the old layouts were sized by.
    #[test]
    fn widths_come_from_the_faces() {
        for face in [Face::Text, Face::Strong, Face::Head] {
            let table = advances(face.ttf()).unwrap_or_else(|| panic!("{face:?}: no advances"));
            assert!(table.iter().all(|w| (0.1..1.2).contains(w)), "{face:?}: {table:?}");
            assert!(width(face, "i", 10.0) < width(face, "m", 10.0) * 0.5, "{face:?}");
            assert_eq!(width(face, "", 14.0), 0.0);
            assert_eq!(width(face, "ab", 20.0), 2.0 * width(face, "ab", 10.0));
            // Figures are set tabular: every digit counts the same.
            assert_eq!(width(face, "111", 14.0), width(face, "808", 14.0));
        }
        let sample = "Baseball bat, 2 x tin of beans";
        let mono = sample.chars().count() as f32 * 14.0 * 0.62;
        let (text, strong, head) = (width(Face::Text, sample, 14.0), width(Face::Strong, sample, 14.0), width(Face::Head, sample, 14.0));
        assert!(head < text && text <= strong && strong < mono * 0.9, "{head} {text} {strong} {mono}");
    }

    /// Each face is what its file name says, and every font file has its licence beside it and a
    /// row in ATTRIBUTION.md.
    #[test]
    fn the_fonts_are_attributed() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let attribution = std::fs::read_to_string(repo.join("ATTRIBUTION.md")).unwrap();
        let dir = repo.join("assets/fonts/barlow");
        assert!(std::fs::read_to_string(dir.join("OFL.txt")).unwrap().contains("SIL OPEN FONT LICENSE Version 1.1"));
        let mut fonts = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            if name.ends_with(".ttf") {
                fonts += 1;
                assert!(attribution.contains(&format!("`assets/fonts/barlow/{name}`")), "{name} has no ATTRIBUTION.md row");
            }
        }
        // Only the weights the UI uses are in the repo.
        assert_eq!(fonts, 3);
    }

    #[test]
    fn sheets_are_solid_inside_and_ragged_at_the_edge() {
        for (name, p, pad) in [("sheet", SHEET_PAINT, SHEET_PAD), ("strip", STRIP_PAINT, STRIP_PAD), ("button", BUTTON_PAINT, (5.0, 4.0))] {
            // The thin, dry part of the paint ends inside the border, so the repeated middle is whole.
            assert!(p.side.1 + p.dry <= p.border as f32 && p.top.1 + p.dry * 0.4 <= p.border as f32, "{name}");
            let c = paint(&p);
            assert_eq!(c.data.len(), (c.w * c.h * 4) as usize);
            // Solid where things stand on it: all the paint there is.
            let least = inside(&p, pad).iter().map(|t| t[3]).fold(1.0, f32::min);
            assert!(least >= p.alpha * 0.6, "{name}: {least}");
            let middle = c.at(c.w / 2, c.h / 2)[3];
            assert!((middle - p.alpha).abs() < 0.01, "{name}: {middle}");
            // Ragged: down the left edge the paint starts at several different places, and the
            // outermost texels are bare.
            let starts: Vec<u32> = (p.border..p.border + p.period).map(|y| (0..c.w).find(|&x| c.at(x, y)[3] > p.alpha * 0.5).unwrap()).collect();
            let (min, max) = (*starts.iter().min().unwrap(), *starts.iter().max().unwrap());
            assert!(max > min, "{name}: a straight edge at {min}");
            assert!((0..c.h).all(|y| c.at(0, y)[3] < 0.5) || p.side.0 < 0.5, "{name}: paint to the very edge");
        }
    }

    /// Cut in nine with the sides and middle repeated, a sheet shows no seam: the middle's last
    /// column is followed by its first, which must look like the column that really follows it.
    #[test]
    fn the_repeated_middle_has_no_seam() {
        for p in [SHEET_PAINT, STRIP_PAINT, BUTTON_PAINT] {
            let c = paint(&p);
            let (a, b) = (p.border, p.border + p.period);
            for k in 0..c.w {
                for ch in 0..4 {
                    // Across: column b (the right border's first) is column a again, down every row
                    // of the middle and of the top and bottom edges alike; and the same down.
                    if (a..b).contains(&k) || k < p.border / 4 {
                        assert!((c.at(a, k)[ch] - c.at(b, k)[ch]).abs() < 0.02, "across at row {k}");
                        assert!((c.at(k, a)[ch] - c.at(k, b)[ch]).abs() < 0.02, "down at column {k}");
                    }
                }
            }
        }
        let s = slot_pixels();
        let (a, b) = (SLOT_BORDER, SLOT_BORDER + SLOT_PERIOD);
        for k in a..b {
            assert!((s.at(a, k)[0] - s.at(b, k)[0]).abs() < 0.02 && (s.at(k, a)[0] - s.at(k, b)[0]).abs() < 0.02, "slot at {k}");
        }
    }

    #[test]
    fn the_small_textures_have_their_shapes() {
        // A slot: hatched (more than one lightness inside), a pale rim, see-through at the corner.
        let s = slot_pixels();
        let mid: Vec<f32> = (20..40).map(|x| s.at(x, 30)[0]).collect();
        assert!(mid.iter().cloned().fold(0.0, f32::max) - mid.iter().cloned().fold(1.0, f32::min) > 0.04);
        assert!((0..s.w).map(|x| s.at(x, 1)[0]).fold(0.0, f32::max) > 0.55, "a rim");
        // A bar: paint in the middle, bare at both ends and along the top.
        let b = bar_pixels();
        assert!(b.at(32, 8)[3] > 0.7 && b.at(0, 8)[3] < 0.2 && b.at(63, 8)[3] < 0.2 && b.at(32, 0)[3] < 0.6);
        // A dab: solid in the middle, nothing in the corners.
        let d = dab_pixels();
        assert!(d.at(32, 32)[3] > 0.95 && d.at(0, 0)[3] == 0.0 && d.at(63, 63)[3] == 0.0);
    }

    #[test]
    fn the_same_textures_every_time() {
        assert_eq!(paint(&SHEET_PAINT).data, paint(&SHEET_PAINT).data);
        assert_eq!(slot_pixels().data, slot_pixels().data);
    }
}
