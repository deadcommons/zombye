//! Every colour of the generated outdoors (ground, road paint, kerbs, lawns, forest floor, trees,
//! hedges), in one table, as sRGB. Muted and a little dusty, per docs/art-direction.md; #60 moves
//! this to a data file shared with `tools/blender/palette.py`.

use bevy::prelude::*;

/// An sRGB colour from the table.
#[derive(Clone, Copy, Debug)]
pub struct Swatch(pub f32, pub f32, pub f32);

impl Swatch {
    pub fn lin(self) -> LinearRgba {
        Color::srgb(self.0, self.1, self.2).to_linear()
    }
}

// Roads.
pub const ASPHALT: Swatch = Swatch(0.26, 0.26, 0.27);
/// Repair patches: newer, darker tar.
pub const ASPHALT_PATCH: Swatch = Swatch(0.20, 0.20, 0.21);
/// The two worn bands in each lane where the wheels run.
pub const WHEEL_TRACK: Swatch = Swatch(0.24, 0.24, 0.25);
/// Dust and grit along the kerb.
pub const GUTTER: Swatch = Swatch(0.31, 0.30, 0.28);
/// Faded centre-line yellow and off-white lane, bay and crossing paint.
pub const PAINT_YELLOW: Swatch = Swatch(0.74, 0.62, 0.30);
pub const PAINT_WHITE: Swatch = Swatch(0.80, 0.79, 0.74);
pub const OIL_STAIN: Swatch = Swatch(0.17, 0.17, 0.17);

// Concrete.
pub const SIDEWALK: Swatch = Swatch(0.60, 0.59, 0.55);
pub const SIDEWALK_JOINT: Swatch = Swatch(0.45, 0.44, 0.41);
pub const KERB_TOP: Swatch = Swatch(0.68, 0.67, 0.63);
pub const KERB_FACE: Swatch = Swatch(0.50, 0.49, 0.46);
pub const DRIVEWAY: Swatch = Swatch(0.51, 0.50, 0.46);
pub const PATH: Swatch = Swatch(0.55, 0.51, 0.45);

// Lawns and yards: a few close olive greens in broad patches.
pub const LAWN: [Swatch; 3] = [Swatch(0.37, 0.43, 0.25), Swatch(0.34, 0.41, 0.23), Swatch(0.40, 0.45, 0.27)];
/// Long, unkempt grass: round fences and walls, vacant lots, overgrown patches.
pub const LONG_GRASS: Swatch = Swatch(0.43, 0.45, 0.27);
/// Rough grass along roads with no sidewalk.
pub const VERGE: Swatch = Swatch(0.45, 0.44, 0.29);
pub const TUFT: Swatch = Swatch(0.51, 0.53, 0.32);
pub const TUFT_DRY: Swatch = Swatch(0.60, 0.56, 0.37);
pub const DIRT: Swatch = Swatch(0.41, 0.34, 0.25);
/// Turned soil of a flower bed.
pub const SOIL: Swatch = Swatch(0.29, 0.23, 0.18);
/// Muted flowers: brick red, cream, mustard, mauve.
pub const FLOWERS: [Swatch; 4] = [Swatch(0.62, 0.27, 0.25), Swatch(0.86, 0.83, 0.71), Swatch(0.78, 0.66, 0.32), Swatch(0.54, 0.41, 0.58)];
pub const SHRUB: [Swatch; 2] = [Swatch(0.24, 0.34, 0.18), Swatch(0.29, 0.37, 0.19)];
pub const HEDGE: Swatch = Swatch(0.22, 0.32, 0.17);
pub const HEDGE_TOP: Swatch = Swatch(0.27, 0.38, 0.20);
pub const FENCE_WOOD: Swatch = Swatch(0.55, 0.48, 0.38);

// Forest floor.
pub const FOREST_FLOOR: [Swatch; 2] = [Swatch(0.28, 0.30, 0.19), Swatch(0.25, 0.27, 0.17)];
/// Brown needle litter under pines.
pub const NEEDLES: Swatch = Swatch(0.34, 0.28, 0.19);
pub const FERN: [Swatch; 2] = [Swatch(0.25, 0.36, 0.18), Swatch(0.31, 0.38, 0.19)];
pub const ROCK: Swatch = Swatch(0.50, 0.50, 0.47);
pub const DEADWOOD: Swatch = Swatch(0.36, 0.28, 0.20);
/// The cut face of a stump or log end.
pub const CUT_WOOD: Swatch = Swatch(0.62, 0.50, 0.34);

// Trees.
pub const BARK: Swatch = Swatch(0.40, 0.31, 0.23);
pub const BARK_DARK: Swatch = Swatch(0.33, 0.25, 0.19);
pub const BIRCH_BARK: Swatch = Swatch(0.80, 0.78, 0.72);
pub const BIRCH_MARK: Swatch = Swatch(0.23, 0.22, 0.21);
/// Broadleaf crowns: a deep green, a mid green, an olive and a dusty one; two or three per tree.
pub const LEAF: [Swatch; 4] = [Swatch(0.24, 0.36, 0.18), Swatch(0.30, 0.41, 0.20), Swatch(0.37, 0.41, 0.19), Swatch(0.29, 0.34, 0.18)];
pub const BIRCH_LEAF: [Swatch; 2] = [Swatch(0.44, 0.50, 0.25), Swatch(0.39, 0.47, 0.23)];
pub const PINE: [Swatch; 2] = [Swatch(0.16, 0.28, 0.18), Swatch(0.19, 0.31, 0.19)];
pub const SPRUCE: [Swatch; 2] = [Swatch(0.13, 0.23, 0.19), Swatch(0.16, 0.27, 0.22)];
