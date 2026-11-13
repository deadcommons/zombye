//! Biomes: what grows on the wild ground, and how, from data (`assets/data/biomes.ron`).
//!
//! Outside town (and on the corners of town the blocks leave wild) every tile belongs to one
//! biome. A biome says how its trees stand (how many, in what clumps, how far apart, which kinds
//! and how tall), its bushes, the bare patches, and how the client draws its ground: colours,
//! grass, flowers, ferns, rocks, stumps, fallen branches and pebbles. [`AreaDef`] says where a
//! biome may be: how far from town, how high, how steep, how big its patches are and how much of
//! the ground it gets. A new biome is a new entry in the file; no code.
//!
//! The simulation reads only where trees, bushes and bare patches go (they are tiles of the
//! map); the rest is the client's. Which biome a tile is in ([`BiomeMap`]) is worked out with the
//! map from the seed, the town's bounds and the terrain, with nothing but `+ - * /`, so the
//! server and every client get the same: derived, never saved or sent.

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::map::Feature;
use crate::rng::hash01;
use crate::terrain::Terrain;

/// The shipped file: what a world without a data folder of its own grows.
const SHIPPED: &str = include_str!("../../../assets/data/biomes.ron");

/// No biome: in town, where lots, streets and yards are laid out by the town generator.
const NONE: u8 = u8::MAX;

/// `biomes.ron`: the biomes, and how they are laid over the wild ground.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BiomeData {
    /// The id of one biome to grow over all the wild ground instead of laying them out by their
    /// areas: for trying a biome's settings (the client's and server's `--biome ID` sets it).
    #[serde(default)]
    pub everywhere: Option<String>,
    /// The side, in metres, of the squares a biome is chosen for. Smaller follows the areas more
    /// closely and costs a little more when a world is made.
    #[serde(default = "default_cell_m")]
    pub cell_m: i32,
    /// How ragged the line between two biomes is, in metres: tiles this near a border fall to
    /// either side, so the trees and ground of two biomes mix there. 0 is a straight step.
    #[serde(default = "default_ragged_m")]
    pub ragged_m: f32,
    /// Over how many metres a biome fades in at the ends of its `town_distance`.
    #[serde(default = "default_blend_distance_m")]
    pub blend_distance_m: f32,
    /// Over how many metres of height a biome fades in at the ends of its `height`.
    #[serde(default = "default_blend_height_m")]
    pub blend_height_m: f32,
    /// Over how much grade a biome fades in at the ends of its `grade`.
    #[serde(default = "default_blend_grade")]
    pub blend_grade: f32,
    /// The biomes. Where none fits a place, the first one grows there.
    pub biomes: Vec<BiomeDef>,
}

fn default_cell_m() -> i32 {
    4
}
fn default_ragged_m() -> f32 {
    3.0
}
fn default_blend_distance_m() -> f32 {
    18.0
}
fn default_blend_height_m() -> f32 {
    3.0
}
fn default_blend_grade() -> f32 {
    0.04
}

impl Default for BiomeData {
    /// The shipped biomes.
    fn default() -> Self {
        shipped().clone()
    }
}

/// The biomes this build ships (`assets/data/biomes.ron`, compiled in).
pub fn shipped() -> &'static BiomeData {
    static DATA: std::sync::OnceLock<BiomeData> = std::sync::OnceLock::new();
    DATA.get_or_init(|| parse_biomes(SHIPPED).expect("the shipped biomes.ron"))
}

/// One biome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BiomeDef {
    /// Its name in the file, in `everywhere` and `--biome`: lower case, digits and `_`.
    pub id: String,
    /// Its name for people (tools and docs). Left out: the id.
    #[serde(default)]
    pub name: String,
    /// Where it may be.
    #[serde(default)]
    pub area: AreaDef,
    /// Its trees.
    #[serde(default)]
    pub trees: TreesDef,
    /// Its bushes.
    #[serde(default)]
    pub bushes: BushesDef,
    /// Its ground: colours and bare patches.
    #[serde(default)]
    pub ground: GroundDef,
    /// What lies on its ground: ferns, rocks, stumps, branches, pebbles.
    #[serde(default)]
    pub litter: LitterDef,
    /// Its grass and wildflowers.
    #[serde(default)]
    pub grass: GrassDef,
}

/// Where a biome may be. At each place every biome that fits gets a score, its `weight` times a
/// noise of its own that rises and falls every `patch_m` metres, and the highest takes the place:
/// so a heavier biome gets more ground and a bigger `patch_m` gets it in bigger pieces.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AreaDef {
    /// How much of the ground it takes where it fits, against the others' weights. 0: never laid
    /// out (only with `everywhere`).
    pub weight: f32,
    /// About how big its patches are, in metres.
    pub patch_m: f32,
    /// How far from the edge of town it may be, in metres (from, to). The wild corners inside the
    /// town's bounds count as 0.
    pub town_distance: (f32, f32),
    /// How high the ground it may be on is, in metres above the town (from, to).
    pub height: (f32, f32),
    /// How steep the hillside it may be on is, as rise over run (from, to): 0.1 is 10 m up over
    /// 100 m. The hills' steepest faces are about 0.3.
    pub grade: (f32, f32),
    /// Whether it may fill the wild corners inside the town's bounds.
    pub in_town: bool,
}

impl Default for AreaDef {
    fn default() -> Self {
        Self { weight: 1.0, patch_m: 90.0, town_distance: (0.0, 100_000.0), height: (-1000.0, 1000.0), grade: (0.0, 10.0), in_town: true }
    }
}

/// A kind of tree. Broadleaves and birches are the map's `Tree`, pines and spruces its
/// `PineTree` (what they take to fell and the logs they give, `crate::trees`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TreeKind {
    /// A round-crowned broadleaf.
    Broadleaf,
    /// A slim birch.
    Birch,
    /// A pine.
    Pine,
    /// A spruce: narrower and taller than a pine.
    Spruce,
}

impl TreeKind {
    /// The map feature a tree of this kind is.
    pub fn feature(self) -> Feature {
        match self {
            TreeKind::Broadleaf | TreeKind::Birch => Feature::Tree,
            TreeKind::Pine | TreeKind::Spruce => Feature::PineTree,
        }
    }
}

/// One kind of tree in a biome.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeKindDef {
    /// Which tree.
    pub kind: TreeKind,
    /// How many of the biome's trees are this kind, against the other kinds' weights.
    #[serde(default = "one")]
    pub weight: f32,
    /// How tall they grow, in metres (from, to): each tree somewhere between.
    pub height: (f32, f32),
}

fn one() -> f32 {
    1.0
}

/// How a biome's trees stand. Trees come in clumps with thinner ground between: a noise that
/// rises and falls every `clump_m` metres is a clump where it is high.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TreesDef {
    /// Trees per 100 m² inside a clump (before `spacing` turns some away). 25 is the most a
    /// spacing of 1 has room for.
    pub thick: f32,
    /// Trees per 100 m² between the clumps.
    pub thin: f32,
    /// About how big clumps and the gaps between them are, in metres.
    pub clump_m: f32,
    /// About how much of the ground is clump, 0 (none) to 1 (all).
    pub clumped: f32,
    /// How softly a clump thins out at its edge: 0 a hard edge, 1 a long fade.
    pub soft: f32,
    /// The tiles (metres) kept free of other trunks round each tree, 1 to 6. 1: trees may stand
    /// two tiles apart, the closest the map lets them.
    pub spacing: i32,
    /// The kinds of tree and how tall each grows. None: no trees.
    pub kinds: Vec<TreeKindDef>,
    /// Hillsides steeper than this (rise over run) begin to lose their trees...
    pub slope_from: f32,
    /// ...and from this steep on they have lost `slope_most` of them.
    pub slope_to: f32,
    /// The share of trees the steepest hillsides lose, 0 to 1.
    pub slope_most: f32,
}

impl Default for TreesDef {
    fn default() -> Self {
        Self { thick: 0.0, thin: 0.0, clump_m: 24.0, clumped: 0.4, soft: 0.3, spacing: 1, kinds: Vec::new(), slope_from: 0.11, slope_to: 0.19, slope_most: 0.85 }
    }
}

/// A biome's bushes (drawn only: bodies walk through them).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BushesDef {
    /// Bushes per 100 m² of open ground.
    pub per_100m2: f32,
    /// How many times as many grow inside a clump of trees as between clumps (1: the same
    /// everywhere; 0: none in the clumps).
    pub in_clumps: f32,
    /// How big they are (from, to); 1 is about a metre across.
    pub size: (f32, f32),
}

impl Default for BushesDef {
    fn default() -> Self {
        Self { per_100m2: 0.0, in_clumps: 1.0, size: (0.85, 1.25) }
    }
}

/// A colour as `"#rrggbb"` (sRGB), as in `palette.ron`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Colour(pub [u8; 3]);

impl TryFrom<String> for Colour {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        let bad = || format!("{s:?} isn't a colour like \"#5d7a3a\"");
        let h = s.strip_prefix('#').filter(|h| h.len() == 6 && h.is_ascii()).ok_or_else(bad)?;
        let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| bad());
        Ok(Colour([byte(0)?, byte(2)?, byte(4)?]))
    }
}

impl From<Colour> for String {
    fn from(c: Colour) -> String {
        format!("#{:02x}{:02x}{:02x}", c.0[0], c.0[1], c.0[2])
    }
}

/// A biome's ground. Colours are the ground's own, under the grass that grows on it; a colour
/// left out is the palette's (`palette.ron`), as the wild ground was drawn before biomes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GroundDef {
    /// The open ground, in patches of each colour (1 to 4 colours; the first covers most).
    /// None: the palette's long grass.
    pub colours: Vec<Colour>,
    /// About how big the patches of colour are, in metres.
    pub patch_m: f32,
    /// The ground where trees stand close (three or more within two tiles), in patches like
    /// `colours`. None: the palette's forest floor.
    pub under_trees: Vec<Colour>,
    /// Fallen needles, in drifts on the ground under pines and spruces. None: the palette's.
    pub needles: Option<Colour>,
    /// How much of the ground under conifers the needles cover, 0 to 1.
    pub needle_cover: f32,
    /// Bare earth patches (a tile of dirt each) per 100 m² of open ground.
    pub bare_per_100m2: f32,
    /// The earth that shows through on a slope (`terrain`: the hillsides' colour drifts to it
    /// from a grade of about 0.06). None: the palette's `dirt`.
    pub slope: Option<Colour>,
    /// The stone of the steepest slopes (from a grade of about 0.11). None: the palette's `cliff`.
    pub rock: Option<Colour>,
}

impl Default for GroundDef {
    fn default() -> Self {
        Self { colours: Vec::new(), patch_m: 5.0, under_trees: Vec::new(), needles: None, needle_cover: 0.25, bare_per_100m2: 1.5, slope: None, rock: None }
    }
}

/// What lies on a biome's ground, each per 100 m² of free ground (drawn only: nothing here
/// blocks walking). "Under trees" is ground with three or more trees within two tiles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LitterDef {
    /// Ferns under trees.
    pub ferns: f32,
    /// Ferns in the open.
    pub ferns_open: f32,
    /// Rocks, anywhere.
    pub rocks: f32,
    /// How big rocks are, in metres across (from, to), at most 1.2.
    pub rock_size: (f32, f32),
    /// How many rocks lie in a group of two to four instead of alone, 0 to 1.
    pub rock_groups: f32,
    /// Old stumps under trees.
    pub stumps: f32,
    /// Fallen branches under trees and within a few metres of one.
    pub branches: f32,
    /// How long branches are, in metres (from, to), at most 2.5.
    pub branch_size: (f32, f32),
    /// Scatterings of pebbles, anywhere.
    pub pebbles: f32,
}

impl Default for LitterDef {
    fn default() -> Self {
        Self { ferns: 0.0, ferns_open: 0.0, rocks: 0.0, rock_size: (0.35, 0.75), rock_groups: 0.3, stumps: 0.0, branches: 0.0, branch_size: (0.7, 1.6), pebbles: 0.0 }
    }
}

/// How long a biome's grass is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GrassKind {
    /// No grass: bare ground.
    None,
    /// Thin and short, the ground showing through.
    #[default]
    Thin,
    /// Short, as a meadow grazed down.
    Short,
    /// Long.
    Long,
}

/// A biome's grass and wildflowers (the client's instanced ground cover, `vegetation.ron` for
/// what all of it shares).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GrassDef {
    /// The grass in the open.
    pub open: GrassKind,
    /// The grass where trees stand close.
    pub under_trees: GrassKind,
    /// How much of the open ground has grass, 0 to 1.
    pub cover: f32,
    /// How much of the ground under trees has grass, 0 to 1.
    pub cover_under_trees: f32,
    /// How dry it is, 0 fresh green to 1 straw (from, to): it varies between the two in drifts.
    pub dry: (f32, f32),
    /// Wildflowers: 0 none, 1 as many as a town's rough ground, more for a flower meadow.
    pub flowers: f32,
}

impl Default for GrassDef {
    fn default() -> Self {
        Self { open: GrassKind::Long, under_trees: GrassKind::Thin, cover: 1.0, cover_under_trees: 0.6, dry: (0.15, 0.6), flowers: 0.3 }
    }
}

fn finite(name: &str, v: &[f32]) -> Result<(), String> {
    if v.iter().all(|x| x.is_finite()) { Ok(()) } else { Err(format!("{name}: every number must be finite")) }
}

fn range(name: &str, (a, b): (f32, f32), lo: f32, hi: f32) -> Result<(), String> {
    if a.is_finite() && b.is_finite() && a <= b && a >= lo && b <= hi { Ok(()) } else { Err(format!("{name} ({a}, {b}) must go from low to high within {lo} to {hi}")) }
}

fn within(name: &str, v: f32, lo: f32, hi: f32) -> Result<(), String> {
    if v.is_finite() && v >= lo && v <= hi { Ok(()) } else { Err(format!("{name} {v} is not {lo} to {hi}")) }
}

impl BiomeDef {
    /// Its name for people.
    pub fn label(&self) -> &str {
        if self.name.is_empty() { &self.id } else { &self.name }
    }

    fn check(&self) -> Result<(), String> {
        let a = &self.area;
        within("area.weight", a.weight, 0.0, 1000.0)?;
        within("area.patch_m", a.patch_m, 8.0, 4000.0)?;
        range("area.town_distance", a.town_distance, 0.0, 1.0e6)?;
        range("area.height", a.height, -1.0e4, 1.0e4)?;
        range("area.grade", a.grade, 0.0, 10.0)?;
        let t = &self.trees;
        within("trees.thick", t.thick, 0.0, 25.0)?;
        within("trees.thin", t.thin, 0.0, 25.0)?;
        within("trees.clump_m", t.clump_m, 4.0, 1000.0)?;
        within("trees.clumped", t.clumped, 0.0, 1.0)?;
        within("trees.soft", t.soft, 0.0, 1.0)?;
        if !(1..=6).contains(&t.spacing) {
            return Err(format!("trees.spacing {} is not 1 to 6", t.spacing));
        }
        if (t.thick > 0.0 || t.thin > 0.0) && t.kinds.is_empty() {
            return Err("trees: thick or thin is over 0 but kinds lists no tree".into());
        }
        for (i, k) in t.kinds.iter().enumerate() {
            within(&format!("trees.kinds[{i}].weight"), k.weight, 0.0, 1000.0)?;
            range(&format!("trees.kinds[{i}].height"), k.height, 1.5, 20.0)?;
            if t.kinds[..i].iter().any(|o| o.kind == k.kind) {
                return Err(format!("trees.kinds lists {:?} twice", k.kind));
            }
        }
        if !t.kinds.is_empty() && t.kinds.iter().all(|k| k.weight <= 0.0) {
            return Err("trees.kinds: every weight is 0".into());
        }
        within("trees.slope_from", t.slope_from, 0.0, 10.0)?;
        within("trees.slope_to", t.slope_to, t.slope_from, 10.0)?;
        within("trees.slope_most", t.slope_most, 0.0, 1.0)?;
        within("bushes.per_100m2", self.bushes.per_100m2, 0.0, 25.0)?;
        within("bushes.in_clumps", self.bushes.in_clumps, 0.0, 20.0)?;
        range("bushes.size", self.bushes.size, 0.3, 2.0)?;
        let g = &self.ground;
        if g.colours.len() > 4 || g.under_trees.len() > 4 {
            return Err("ground: at most 4 colours in a list".into());
        }
        within("ground.patch_m", g.patch_m, 0.5, 200.0)?;
        within("ground.needle_cover", g.needle_cover, 0.0, 1.0)?;
        within("ground.bare_per_100m2", g.bare_per_100m2, 0.0, 50.0)?;
        let l = &self.litter;
        for (name, v) in [("ferns", l.ferns), ("ferns_open", l.ferns_open), ("rocks", l.rocks), ("stumps", l.stumps), ("branches", l.branches), ("pebbles", l.pebbles)] {
            within(&format!("litter.{name}"), v, 0.0, 60.0)?;
        }
        range("litter.rock_size", l.rock_size, 0.1, 1.2)?;
        within("litter.rock_groups", l.rock_groups, 0.0, 1.0)?;
        range("litter.branch_size", l.branch_size, 0.3, 2.5)?;
        let s = &self.grass;
        within("grass.cover", s.cover, 0.0, 1.0)?;
        within("grass.cover_under_trees", s.cover_under_trees, 0.0, 1.0)?;
        range("grass.dry", s.dry, 0.0, 1.0)?;
        within("grass.flowers", s.flowers, 0.0, 10.0)?;
        Ok(())
    }

    /// The kind of tree tile `t` grows here: one of this biome's kinds of the map's feature
    /// (`conifer`: a `PineTree`), by their weights and a hash of the tile. `None` when the biome
    /// has no such kind (a tree the town planted, or one from another file's map).
    pub fn tree_kind(&self, t: IVec2, conifer: bool) -> Option<&TreeKindDef> {
        let fits = |k: &&TreeKindDef| (k.kind.feature() == Feature::PineTree) == conifer && k.weight > 0.0;
        let total: f32 = self.trees.kinds.iter().filter(fits).map(|k| k.weight).sum();
        if total <= 0.0 {
            return None;
        }
        let mut u = hash01(0x62_696f, t.x as u64, t.y as u64, 3) * total;
        let mut last = None;
        for k in self.trees.kinds.iter().filter(fits) {
            last = Some(k);
            if u < k.weight {
                break;
            }
            u -= k.weight;
        }
        last
    }

    /// Whether the tree a tile of this biome grows is a conifer, by the kinds' weights.
    fn grows_conifer(&self, seed: u64, t: IVec2) -> bool {
        let total: f32 = self.trees.kinds.iter().map(|k| k.weight.max(0.0)).sum();
        let conifers: f32 = self.trees.kinds.iter().filter(|k| k.kind.feature() == Feature::PineTree).map(|k| k.weight.max(0.0)).sum();
        hash01(seed, t.x as u64, t.y as u64, 0xb103) * total < conifers
    }
}

impl BiomeData {
    /// What's wrong with these biomes, if anything; errors name the biome.
    pub fn check(&self) -> Result<(), String> {
        if self.biomes.is_empty() {
            return Err("no biomes".into());
        }
        if self.biomes.len() >= NONE as usize {
            return Err(format!("{} biomes: at most {}", self.biomes.len(), NONE - 1));
        }
        if !(1..=32).contains(&self.cell_m) {
            return Err(format!("cell_m {} is not 1 to 32", self.cell_m));
        }
        within("ragged_m", self.ragged_m, 0.0, 30.0)?;
        within("blend_distance_m", self.blend_distance_m, 0.0, 1000.0)?;
        within("blend_height_m", self.blend_height_m, 0.0, 100.0)?;
        within("blend_grade", self.blend_grade, 0.0, 1.0)?;
        finite("biomes", &[self.ragged_m])?;
        for (i, b) in self.biomes.iter().enumerate() {
            if b.id.is_empty() || !b.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                return Err(format!("biome {:?}: an id is lower-case letters, digits and _", b.id));
            }
            if self.biomes[..i].iter().any(|o| o.id == b.id) {
                return Err(format!("biome {:?} is there twice", b.id));
            }
            b.check().map_err(|e| format!("biome {:?}: {e}", b.id))?;
        }
        if let Some(id) = &self.everywhere
            && !self.biomes.iter().any(|b| &b.id == id)
        {
            return Err(format!("everywhere: no biome {id:?} (there are: {})", self.ids().join(", ")));
        }
        Ok(())
    }

    /// Every biome's id, in the file's order.
    pub fn ids(&self) -> Vec<&str> {
        self.biomes.iter().map(|b| b.id.as_str()).collect()
    }

    /// These biomes with `id` grown everywhere (`--biome ID`).
    pub fn with_everywhere(mut self, id: &str) -> Result<Self, String> {
        self.everywhere = Some(id.to_string());
        self.check()?;
        Ok(self)
    }
}

/// Parse and check `biomes.ron`.
pub fn parse_biomes(text: &str) -> Result<BiomeData, String> {
    let data: BiomeData = ron::from_str(text).map_err(|e| e.to_string())?;
    data.check()?;
    Ok(data)
}

/// `x` within `from..=to`, fading in over `blend` outside either end: 1 inside, 0 beyond the fade.
fn fit(x: f32, (from, to): (f32, f32), blend: f32) -> f32 {
    if x >= from && x <= to {
        return 1.0;
    }
    let out = if x < from { from - x } else { x - to };
    if blend <= 0.0 { 0.0 } else { (1.0 - out / blend).max(0.0) }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    if b <= a {
        return if x >= b { 1.0 } else { 0.0 };
    }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn hash2(seed: u64, x: i32, y: i32) -> f32 {
    let mut h = seed ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Smooth value noise in [0, 1].
fn value_noise(seed: u64, p: Vec2) -> f32 {
    let i = p.floor();
    let f = p - i;
    let (x, y) = (i.x as i32, i.y as i32);
    let u = f * f * (Vec2::splat(3.0) - 2.0 * f);
    let a = hash2(seed, x, y);
    let b = hash2(seed, x + 1, y);
    let c = hash2(seed, x, y + 1);
    let d = hash2(seed, x + 1, y + 1);
    (a + (b - a) * u.x) + ((c + (d - c) * u.x) - (a + (b - a) * u.x)) * u.y
}

/// A number of a biome's own from its id (FNV-1a), so its patches stay where they are when
/// biomes are added, taken out or put in another order.
fn salt(id: &str) -> u64 {
    id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// What to plant on a tile (see [`BiomeMap::plant`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plant {
    /// A tree: the map's `Tree` or `PineTree`.
    Tree(Feature),
    /// A bush.
    Bush,
    /// A patch of bare earth.
    Bare,
}

/// Which biome every tile of a map is in, with the biomes themselves. Derived with the map (see
/// the module docs); a map made without one (a test's, an empty one) has none anywhere.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeMap {
    defs: Vec<BiomeDef>,
    size: IVec2,
    seed: u64,
    /// The biome of each tile (an index into `defs`), `NONE` in town.
    tiles: Vec<u8>,
}

impl BiomeMap {
    /// Lay `data`'s biomes over a map of `tiles`: none inside the town (`town_min..town_max`)
    /// but on its `wild` rectangles (`min..max` each), and elsewhere the best-scoring biome of
    /// each `cell_m` square, with ragged borders.
    pub fn generate(data: &BiomeData, seed: u64, tiles: IVec2, town_min: IVec2, town_max: IVec2, wild: &[(IVec2, IVec2)], terrain: &Terrain) -> Self {
        let defs = data.biomes.clone();
        if defs.is_empty() || tiles.min_element() <= 0 {
            return Self::default();
        }
        let everywhere = data.everywhere.as_ref().and_then(|id| defs.iter().position(|b| &b.id == id));
        let cell = data.cell_m.max(1);
        let cells = (tiles + IVec2::splat(cell - 1)) / cell + IVec2::ONE;
        let salts: Vec<u64> = defs.iter().map(|b| seed ^ salt(&b.id)).collect();
        let in_town = |p: IVec2| p.cmpge(town_min).all() && p.cmplt(town_max).all();
        // The best biome of each square, by its middle.
        let mut grid = vec![0u8; (cells.x * cells.y) as usize];
        if everywhere.is_none() {
            for cy in 0..cells.y {
                for cx in 0..cells.x {
                    let c = (IVec2::new(cx, cy).as_vec2() + Vec2::splat(0.5)) * cell as f32;
                    let ci = c.as_ivec2();
                    let outside = IVec2::new((town_min.x - ci.x).max(ci.x - town_max.x).max(0), (town_min.y - ci.y).max(ci.y - town_max.y).max(0));
                    let dist = outside.max_element() as f32;
                    let (height, grade) = if terrain.is_flat() { (0.0, 0.0) } else { (terrain.height(c), terrain.broad_grade(c)) };
                    let town = in_town(ci);
                    let (mut best, mut best_score) = (0usize, 0.0f32);
                    for (i, b) in defs.iter().enumerate() {
                        let a = &b.area;
                        if a.weight <= 0.0 || (town && !a.in_town) {
                            continue;
                        }
                        let fits = fit(dist, a.town_distance, data.blend_distance_m) * fit(height, a.height, data.blend_height_m) * fit(grade, a.grade, data.blend_grade);
                        if fits <= 0.0 {
                            continue;
                        }
                        let n = value_noise(salts[i], c / a.patch_m) * 0.7 + value_noise(salts[i] ^ 0x5bd1, c / (a.patch_m * 0.37)) * 0.3;
                        let score = a.weight * fits * (0.2 + n);
                        if score > best_score {
                            (best, best_score) = (i, score);
                        }
                    }
                    grid[(cy * cells.x + cx) as usize] = best as u8;
                }
            }
        }
        let wild_at = |p: IVec2| wild.iter().any(|(lo, hi)| p.cmpge(*lo).all() && p.cmplt(*hi).all());
        let mut out = vec![NONE; (tiles.x * tiles.y) as usize];
        let ragged = data.ragged_m.max(0.0);
        for y in 0..tiles.y {
            for x in 0..tiles.x {
                let p = IVec2::new(x, y);
                if in_town(p) && !wild_at(p) {
                    continue;
                }
                out[(y * tiles.x + x) as usize] = match everywhere {
                    Some(i) => i as u8,
                    None => {
                        // Pushed aside by up to `ragged_m`, in drifts and tile by tile, so borders wander.
                        let v = p.as_vec2() + Vec2::splat(0.5);
                        let push = Vec2::new(
                            value_noise(seed ^ 0x7a31, v / 7.0) * 0.7 + hash01(seed, x as u64, y as u64, 0xb101) * 0.3 - 0.5,
                            value_noise(seed ^ 0x19c5, v / 7.0) * 0.7 + hash01(seed, x as u64, y as u64, 0xb102) * 0.3 - 0.5,
                        ) * 2.0 * ragged;
                        let c = ((v + push) / cell as f32).as_ivec2().clamp(IVec2::ZERO, cells - IVec2::ONE);
                        grid[(c.y * cells.x + c.x) as usize]
                    }
                };
            }
        }
        Self { defs, size: tiles, seed, tiles: out }
    }

    /// No biome anywhere (a map made without the generator).
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// The biomes, as [`index_at`](Self::index_at) numbers them.
    pub fn defs(&self) -> &[BiomeDef] {
        &self.defs
    }

    /// The number of the biome tile `t` is in: `None` in town and off the map.
    pub fn index_at(&self, t: IVec2) -> Option<usize> {
        if t.min_element() < 0 || t.x >= self.size.x || t.y >= self.size.y {
            return None;
        }
        match self.tiles.get((t.y * self.size.x + t.x) as usize) {
            Some(&i) if i != NONE => Some(i as usize),
            _ => None,
        }
    }

    /// The biome tile `t` is in: `None` in town and off the map.
    pub fn at(&self, t: IVec2) -> Option<&BiomeDef> {
        self.index_at(t).map(|i| &self.defs[i])
    }

    /// How many tiles each biome has, in `defs` order.
    pub fn counts(&self) -> Vec<usize> {
        let mut n = vec![0; self.defs.len()];
        for &i in &self.tiles {
            if i != NONE {
                n[i as usize] += 1;
            }
        }
        n
    }

    /// How much of a clump of trees tile `t` is in, 0 (between clumps) to 1 (inside one), by
    /// its biome's `clump_m`, `clumped` and `soft`.
    pub fn clump(&self, t: IVec2) -> f32 {
        let Some(b) = self.at(t) else { return 0.0 };
        let tr = &b.trees;
        let p = t.as_vec2() + Vec2::splat(0.5);
        let s = self.seed ^ salt(&b.id);
        let n = value_noise(s ^ 0xc1, p / tr.clump_m) * 0.65 + value_noise(s ^ 0xc2, p / (tr.clump_m * 0.31)) * 0.35;
        // The noise sits about a half, most of it between 0.2 and 0.8: the line that leaves
        // `clumped` of the ground above it.
        let line = 0.5 + (0.5 - tr.clumped) * 0.75;
        let soft = 0.015 + 0.2 * tr.soft;
        if tr.clumped >= 1.0 {
            1.0
        } else if tr.clumped <= 0.0 {
            0.0
        } else {
            smoothstep(line - soft, line + soft, n)
        }
    }

    /// What the biome of open tile `t` plants on it, if anything: by hashes of the seed and the
    /// tile alone, so changing one biome's numbers moves nothing in another, or in town. `grade`
    /// is the hillside's there (`Terrain::broad_grade`). The caller keeps trees `spacing` apart.
    pub fn plant(&self, t: IVec2, grade: f32) -> Option<Plant> {
        let b = self.at(t)?;
        let h = |k: u64| hash01(self.seed, t.x as u64, t.y as u64, k);
        let clump = self.clump(t);
        let tr = &b.trees;
        let steep = 1.0 - tr.slope_most * smoothstep(tr.slope_from, tr.slope_to, grade);
        let trees = (tr.thin + (tr.thick - tr.thin) * clump) * steep / 100.0;
        let roll = h(0xb110);
        if !tr.kinds.is_empty() && roll < trees {
            return Some(Plant::Tree(if b.grows_conifer(self.seed, t) { Feature::PineTree } else { Feature::Tree }));
        }
        let bushes = b.bushes.per_100m2 * (1.0 + (b.bushes.in_clumps - 1.0) * clump) / 100.0;
        if h(0xb111) < bushes {
            return Some(Plant::Bush);
        }
        (h(0xb112) < b.ground.bare_per_100m2 / 100.0).then_some(Plant::Bare)
    }
}

#[cfg(test)]
mod tests;
