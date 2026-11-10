//! The status icons (Linear DCM-57): instead of stat bars, a small column of icons right of the
//! weapon panel, one for each status that has passed a level (hurt, bleeding, pain, sick, queasy,
//! hungry, thirsty, tired, out of breath, loaded down, asleep), coloured and bordered by how bad it
//! is (1 to 4, with as many pips; a good state such as "Well fed" dimmer). An icon pulses when it
//! appears or worsens; hover one for a line on what it means.
//!
//! Which icons there are, their levels, labels and tips are data (`assets/data/status.ron`, read
//! from the `--data` folder: `sim::status`); they are worked out from our private view, our HP and
//! the predicted endurance through the same function the server's MCP tools use
//! (`PrivateView::status_tags`). The pictures are drawn here, in code ([`glyph`]): white shapes on
//! a 64 px image, tinted by severity. Also B to sleep (in `player.rs`).

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use zombye_sim::{StatusData, StatusTag, TagKind};

use crate::player::LocalPlayer;
use crate::server_link::{JoinInfo, ServerLink};

const TEXT: Color = Color::srgb(0.95, 0.93, 0.88);
const TIP_BG: Color = Color::srgba(0.02, 0.02, 0.03, 0.85);
/// An icon's tile and the room between two (UI pixels), and how many stand in a column before the
/// next column starts.
const TILE: f32 = 38.0;
const GAP: f32 = 4.0;
const PER_COLUMN: f32 = 3.0;
/// How long an icon pulses after it appears or worsens.
const PULSE_SECS: f32 = 1.2;
/// The side of a glyph's image, in pixels.
const GLYPH_PX: u32 = 64;

/// The icons' data (`status.ron`) for everything on the client that reads our status.
#[derive(Resource)]
pub struct StatusIcons(pub StatusData);

impl StatusIcons {
    /// Our status icons now (none when dead or before our first private view).
    pub fn tags(&self, link: &ServerLink, join: &JoinInfo, me: &LocalPlayer) -> Vec<StatusTag> {
        match link.own(me.id) {
            Some(v) if !me.dead => {
                let hp_lost = 1.0 - (me.hp / join.combat.player_hp.max(1.0)).clamp(0.0, 1.0);
                v.status_tags(&self.0, &join.needs, hp_lost, me.curr.endurance, me.curr.winded, link.curr.time_scale)
            }
            _ => Vec::new(),
        }
    }
}

#[derive(Component)]
struct Root;
#[derive(Component)]
struct TipText;
/// An icon's tile, its picture and its severity pips (the `n`th of four).
#[derive(Component)]
struct Tile(TagKind);
#[derive(Component)]
struct Glyph(TagKind);
#[derive(Component)]
struct Pip(TagKind, u8);

/// The icons on screen, and when each last appeared or worsened.
#[derive(Resource, Default)]
struct Shown {
    tags: Option<Vec<StatusTag>>,
    pulses: Vec<(TagKind, f32)>,
}

/// The icon the pointer is over.
#[derive(Resource, Default)]
struct Hovered(Option<TagKind>);

pub struct NeedsUiPlugin;

impl Plugin for NeedsUiPlugin {
    fn build(&self, app: &mut App) {
        let data = app.world().get_resource::<crate::Options>().map(|o| zombye_net::content::load_status(&o.data));
        let data = match data {
            Some(Ok(data)) => data,
            Some(Err(e)) => {
                eprintln!("warning: {e}; using the built-in status icons");
                StatusData::builtin().clone()
            }
            None => StatusData::builtin().clone(),
        };
        app.insert_resource(StatusIcons(data)).init_resource::<Shown>().init_resource::<Hovered>().add_systems(Startup, spawn).add_systems(Update, (update, place, pulse, tooltip).chain());
    }
}

fn font(size: f32) -> TextFont {
    TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }
}

/// An icon's colours by kind and severity: the picture and border, and the tile behind it. Good
/// states are dim green, sleep blue, the rest pale yellow, amber, orange and red on a red tile.
pub fn look(kind: TagKind, severity: u8) -> (Color, Color) {
    let tile = Color::srgba(0.03, 0.03, 0.04, 0.72);
    match (kind, severity) {
        (TagKind::Asleep, _) => (Color::srgb(0.5, 0.65, 0.98), Color::srgba(0.04, 0.06, 0.16, 0.8)),
        (_, 0) => (Color::srgba(0.5, 0.68, 0.5, 0.75), Color::srgba(0.03, 0.05, 0.03, 0.5)),
        (_, 1) => (Color::srgb(0.88, 0.83, 0.5), tile),
        (_, 2) => (Color::srgb(0.96, 0.64, 0.2), tile),
        (_, 3) => (Color::srgb(0.97, 0.4, 0.14), Color::srgba(0.14, 0.04, 0.02, 0.8)),
        _ => (Color::srgb(1.0, 0.86, 0.8), Color::srgba(0.62, 0.05, 0.04, 0.92)),
    }
}

/// The border's colour: the picture's, but red at the worst (where the picture goes pale on red).
fn border(kind: TagKind, severity: u8) -> Color {
    if severity >= 4 && kind != TagKind::Asleep { Color::srgb(1.0, 0.25, 0.2) } else { look(kind, severity).0 }
}

/// The kinds that appeared or got worse between two lists of icons: the ones that pulse.
pub fn worsened(before: &[StatusTag], now: &[StatusTag]) -> Vec<TagKind> {
    now.iter().filter(|t| before.iter().find(|b| b.kind == t.kind).is_none_or(|b| t.severity > b.severity)).map(|t| t.kind).collect()
}

/// A pulse `t` (0..1) of the way through: the ring's distance from the tile (UI pixels) and how
/// strong it is (0..1). Two rings, each moving out and fading; nothing once it's over.
pub fn pulse_ring(t: f32) -> (f32, f32) {
    if !(0.0..1.0).contains(&t) {
        return (0.0, 0.0);
    }
    let phase = (t * 2.0).fract();
    (phase * 7.0, (1.0 - phase) * (1.0 - 0.4 * t))
}

// --- The pictures -----------------------------------------------------------------------------

type P = (f32, f32);

fn disc(p: P, c: P, r: f32) -> bool {
    (p.0 - c.0).powi(2) + (p.1 - c.1).powi(2) <= r * r
}

/// Within `w` of the segment from `a` to `b` (a line with round ends).
fn line(p: P, a: P, b: P, w: f32) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy).max(1e-9)).clamp(0.0, 1.0);
    disc(p, (a.0 + dx * t, a.1 + dy * t), w)
}

/// Within `w` of the path through `pts`.
fn path(p: P, pts: &[P], w: f32) -> bool {
    pts.windows(2).any(|s| line(p, s[0], s[1], w))
}

/// Inside the polygon `pts` (even-odd).
fn poly(p: P, pts: &[P]) -> bool {
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn rect(p: P, c: P, half: P) -> bool {
    (p.0 - c.0).abs() <= half.0 && (p.1 - c.1).abs() <= half.1
}

/// A drop, point up, hanging from (0, `top`), `r` round at the bottom.
fn drop(p: P, c: P, r: f32) -> bool {
    disc(p, c, r) || poly(p, &[(c.0, c.1 - 2.6 * r), (c.0 - 0.94 * r, c.1 - 0.34 * r), (c.0 + 0.94 * r, c.1 - 0.34 * r)])
}

/// Is the point `p` (x right, y down, both -1..1) part of `kind`'s picture. Our own shapes: a
/// cross, a drop, a bolt, a thermometer, a swirl, a knife and fork, a half-full glass, a crescent
/// moon, a heartbeat, a weight and a Z.
pub fn glyph(kind: TagKind, p: P) -> bool {
    match kind {
        TagKind::Health => rect(p, (0.0, 0.0), (0.27, 0.8)) || rect(p, (0.0, 0.0), (0.8, 0.27)),
        TagKind::Bleeding => drop(p, (0.0, 0.32), 0.52),
        TagKind::Pain => poly(p, &[(0.2, -0.92), (-0.55, 0.12), (-0.08, 0.12), (-0.28, 0.92), (0.55, -0.2), (0.06, -0.2)]),
        TagKind::Sick => {
            // A thermometer: a tube with its bulb, the tube hollow above the mercury, and marks.
            let tube = line(p, (-0.15, -0.62), (-0.15, 0.3), 0.24) || disc(p, (-0.15, 0.48), 0.42);
            let hollow = line(p, (-0.15, -0.62), (-0.15, -0.22), 0.11);
            let marks = [-0.6, -0.3, 0.0].iter().any(|&y| line(p, (0.3, y), (0.62, y), 0.06));
            (tube && !hollow) || marks
        }
        TagKind::Queasy => {
            // A swirl: two and a quarter turns of a spiral.
            let pts: Vec<P> = (0..=54).map(|i| i as f32 / 54.0).map(|t| ((t * 14.2).cos() * (0.08 + 0.74 * t), (t * 14.2).sin() * (0.08 + 0.74 * t))).collect();
            path(p, &pts, 0.085)
        }
        TagKind::Hunger => {
            // A fork (three tines on a handle) and a knife (a blade with a curved back).
            let fork = line(p, (-0.42, -0.25), (-0.42, 0.82), 0.09)
                || line(p, (-0.68, -0.28), (-0.16, -0.28), 0.08)
                || [-0.68, -0.42, -0.16].iter().any(|&x| line(p, (x, -0.82), (x, -0.28), 0.06));
            let blade = p.0 >= 0.3 && p.1 <= 0.1 && ((p.0 - 0.3) / 0.36).powi(2) + ((p.1 - 0.1) / 0.95).powi(2) <= 1.0;
            fork || blade || line(p, (0.39, 0.1), (0.39, 0.82), 0.09)
        }
        TagKind::Thirst => {
            // A glass, half full.
            let glass = poly(p, &[(-0.6, -0.8), (0.6, -0.8), (0.42, 0.82), (-0.42, 0.82)]);
            let air = poly(p, &[(-0.43, -0.9), (0.43, -0.9), (0.36, -0.05), (-0.36, -0.05)]);
            glass && !air
        }
        TagKind::Tiredness => disc(p, (-0.08, 0.0), 0.82) && !disc(p, (0.3, -0.2), 0.7),
        TagKind::Endurance => path(p, &[(-0.8, 0.1), (-0.48, 0.1), (-0.26, -0.62), (0.04, 0.72), (0.28, -0.25), (0.42, 0.1), (0.8, 0.1)], 0.09),
        TagKind::Load => {
            // A weight with a ring to lift it by.
            let ring = disc(p, (0.0, -0.5), 0.36) && !disc(p, (0.0, -0.5), 0.19);
            ring || poly(p, &[(-0.5, -0.28), (0.5, -0.28), (0.8, 0.82), (-0.8, 0.82)])
        }
        TagKind::Asleep => path(p, &[(-0.5, -0.62), (0.5, -0.62), (-0.5, 0.62), (0.5, 0.62)], 0.12),
    }
}

/// How much of pixel (`x`, `y`) of a `size` px image `kind`'s picture covers (0..1; 4 x 4 samples).
pub fn coverage(kind: TagKind, x: u32, y: u32, size: u32) -> f32 {
    let mut hits = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            let u = (x as f32 + (sx as f32 + 0.5) / 4.0) / size as f32 * 2.0 - 1.0;
            let v = (y as f32 + (sy as f32 + 0.5) / 4.0) / size as f32 * 2.0 - 1.0;
            hits += glyph(kind, (u, v)) as u32;
        }
    }
    hits as f32 / 16.0
}

/// `kind`'s picture as a white image with the shape in its alpha, to be tinted.
fn glyph_image(kind: TagKind) -> Image {
    let mut data = Vec::with_capacity((GLYPH_PX * GLYPH_PX * 4) as usize);
    for y in 0..GLYPH_PX {
        for x in 0..GLYPH_PX {
            data.extend_from_slice(&[255, 255, 255, (coverage(kind, x, y, GLYPH_PX) * 255.0).round() as u8]);
        }
    }
    Image::new(Extent3d { width: GLYPH_PX, height: GLYPH_PX, depth_or_array_layers: 1 }, TextureDimension::D2, data, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD)
}

// --- On screen --------------------------------------------------------------------------------

/// Every icon the data lists, hidden until its status applies: they stay the same entities, so a
/// pulse and the pointer's hover carry on while the levels change.
fn spawn(mut commands: Commands, icons: Res<StatusIcons>, mut images: ResMut<Assets<Image>>) {
    commands
        .spawn((
            Root,
            Node { position_type: PositionType::Absolute, bottom: px(16), left: px(300), flex_direction: FlexDirection::Column, align_items: AlignItems::FlexStart, ..default() },
            Pickable::IGNORE,
        ))
        .with_children(|c| {
            c.spawn((
                Node { flex_direction: FlexDirection::Column, flex_wrap: FlexWrap::Wrap, row_gap: px(GAP), column_gap: px(GAP), max_height: px(PER_COLUMN * TILE + (PER_COLUMN - 1.0) * GAP), ..default() },
                Pickable::IGNORE,
            ))
            .with_children(|col| {
                for icon in &icons.0.icons {
                    let kind = icon.kind;
                    col.spawn((
                        Tile(kind),
                        Node {
                            display: Display::None,
                            width: px(TILE),
                            height: px(TILE),
                            border: UiRect::all(px(2)),
                            border_radius: BorderRadius::all(px(6)),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(Color::NONE),
                        BorderColor::all(Color::NONE),
                        Outline { width: px(2), offset: px(0), color: Color::NONE },
                    ))
                    .with_children(|tile| {
                        tile.spawn((Glyph(kind), ImageNode { image: images.add(glyph_image(kind)), ..default() }, Node { width: px(24), height: px(24), margin: UiRect::bottom(px(4)), ..default() }, Pickable::IGNORE));
                        tile.spawn((Node { position_type: PositionType::Absolute, bottom: px(2), left: px(0), right: px(0), justify_content: JustifyContent::Center, column_gap: px(2), ..default() }, Pickable::IGNORE)).with_children(|pips| {
                            for n in 1..=zombye_sim::status::MAX_SEVERITY {
                                pips.spawn((Pip(kind, n), Node { width: px(5), height: px(3), ..default() }, BackgroundColor(Color::NONE), Pickable::IGNORE));
                            }
                        });
                    })
                    .observe(move |_: On<PointerOver>, mut h: ResMut<Hovered>| h.0 = Some(kind))
                    .observe(move |_: On<PointerOut>, mut h: ResMut<Hovered>| {
                        if h.0 == Some(kind) {
                            h.0 = None;
                        }
                    });
                }
            });
        });
    // The hovered icon's line, beside the icons along the bottom: under the current action's bar
    // and the hint line, which stand higher.
    commands.spawn((
        TipText,
        Text::new(""),
        font(13.0),
        TextColor(TEXT),
        Node { position_type: PositionType::Absolute, bottom: px(16), left: px(350), max_width: px(520), padding: UiRect::all(px(6)), ..default() },
        BackgroundColor(TIP_BG),
        Visibility::Hidden,
        Pickable::IGNORE,
    ));
}

/// How wide `icons` icons stand (UI pixels): columns of [`PER_COLUMN`].
pub fn columns_width(icons: usize) -> f32 {
    let columns = (icons as f32 / PER_COLUMN).ceil();
    columns * TILE + (columns - 1.0).max(0.0) * GAP
}

/// The icons stand just right of the weapon panel, however wide its text makes it, and the
/// hovered one's line right of them.
#[allow(clippy::type_complexity)]
fn place(shown: Res<Shown>, vitals: Query<&ComputedNode, With<crate::hud::VitalsPanel>>, mut root: Query<&mut Node, (With<Root>, Without<TipText>)>, mut tip: Query<&mut Node, With<TipText>>) {
    let (Ok(panel), Ok(mut node), Ok(mut tip)) = (vitals.single(), root.single_mut(), tip.single_mut()) else { return };
    let left = (16.0 + panel.size().x * panel.inverse_scale_factor() + 10.0).round();
    if node.left != px(left) {
        node.left = px(left);
    }
    let beside = px(left + columns_width(shown.tags.as_ref().map_or(0, Vec::len)) + 8.0);
    if tip.left != beside {
        tip.left = beside;
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update(
    time: Res<Time>,
    icons: Res<StatusIcons>,
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    me: Res<LocalPlayer>,
    options: Res<crate::Options>,
    mut shown: ResMut<Shown>,
    mut hovered: ResMut<Hovered>,
    mut tiles: Query<(&Tile, &mut Node, &mut BackgroundColor, &mut BorderColor), (Without<Glyph>, Without<Pip>)>,
    mut glyphs: Query<(&Glyph, &mut ImageNode)>,
    mut pips: Query<(&Pip, &mut BackgroundColor), Without<Tile>>,
) {
    let tags = icons.tags(&link, &join, &me);
    if shown.tags.as_ref() == Some(&tags) {
        return;
    }
    // Icons that appear or worsen pulse; not the ones a world starts with.
    if let Some(before) = &shown.tags {
        let now = time.elapsed_secs();
        for kind in worsened(before, &tags) {
            shown.pulses.retain(|(k, _)| *k != kind);
            shown.pulses.push((kind, now));
        }
    }
    // `--hover-tag N`: as if the pointer were over the Nth icon shown.
    if let Some(n) = options.hover_tag {
        hovered.0 = tags.get(n).map(|t| t.kind);
    }
    let severity = |kind: TagKind| tags.iter().find(|t| t.kind == kind).map(|t| t.severity);
    for (tile, mut node, mut bg, mut edge) in &mut tiles {
        let s = severity(tile.0);
        let display = if s.is_some() { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
        if let Some(s) = s {
            // The worst level has a thicker border as well as its colours.
            let width = UiRect::all(px(if s >= 4 { 3 } else { 2 }));
            if node.border != width {
                node.border = width;
            }
            bg.0 = look(tile.0, s).1;
            *edge = BorderColor::all(border(tile.0, s));
        }
    }
    for (glyph, mut image) in &mut glyphs {
        if let Some(s) = severity(glyph.0) {
            image.color = look(glyph.0, s).0;
        }
    }
    for (pip, mut bg) in &mut pips {
        if let Some(s) = severity(pip.0) {
            let on = look(pip.0, s).0;
            bg.0 = if pip.1 <= s { on } else if s > 0 { on.with_alpha(0.18) } else { Color::NONE };
        }
    }
    shown.tags = Some(tags);
}

/// The ring round an icon that has just appeared or worsened.
fn pulse(time: Res<Time>, mut shown: ResMut<Shown>, mut tiles: Query<(&Tile, &mut Outline)>) {
    let now = time.elapsed_secs();
    let severity = |kind: TagKind| shown.tags.as_ref().and_then(|tags| tags.iter().find(|t| t.kind == kind)).map_or(0, |t| t.severity);
    for (tile, mut outline) in &mut tiles {
        let t = shown.pulses.iter().find(|(k, _)| *k == tile.0).map(|(_, at)| (now - at) / PULSE_SECS);
        let (offset, strength) = t.map_or((0.0, 0.0), pulse_ring);
        let color = if strength > 0.0 { border(tile.0, severity(tile.0)).with_alpha(strength) } else { Color::NONE };
        if outline.color != color {
            outline.color = color;
            outline.offset = px(offset);
        }
    }
    if shown.pulses.iter().any(|(_, at)| now - at >= PULSE_SECS) {
        shown.pulses.retain(|(_, at)| now - at < PULSE_SECS);
    }
}

fn tooltip(hovered: Res<Hovered>, shown: Res<Shown>, mut tip: Query<(&mut Text, &mut Visibility), With<TipText>>) {
    let Ok((mut text, mut vis)) = tip.single_mut() else { return };
    let line = hovered.0.and_then(|kind| shown.tags.as_ref()?.iter().find(|t| t.kind == kind)).map(|t| t.tip.clone());
    let v = if line.is_some() { Visibility::Inherited } else { Visibility::Hidden };
    let want = line.unwrap_or_default();
    if text.0 != want {
        text.0 = want;
    }
    if *vis != v {
        *vis = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(kind: TagKind, severity: u8) -> StatusTag {
        StatusTag { kind, label: String::new(), severity, tip: String::new() }
    }

    #[test]
    fn every_kind_has_its_own_picture() {
        let size = 32;
        let picture = |kind| (0..size * size).map(|i| coverage(kind, i % size, i / size, size) >= 0.5).collect::<Vec<bool>>();
        let pictures: Vec<_> = TagKind::ALL.iter().map(|&k| (k, picture(k))).collect();
        for (kind, p) in &pictures {
            // Something to see, with room round it: between 8% and 60% of the tile, nothing on its edge.
            let filled = p.iter().filter(|&&on| on).count() as f32 / p.len() as f32;
            assert!((0.08..0.6).contains(&filled), "{kind:?} fills {filled}");
            let edge = (0..size).any(|i| p[i as usize] || p[(size * (size - 1) + i) as usize] || p[(i * size) as usize] || p[(i * size + size - 1) as usize]);
            assert!(!edge, "{kind:?} touches the edge of its tile");
        }
        // No two alike: each differs from every other in at least 12% of its pixels.
        for (i, (a, pa)) in pictures.iter().enumerate() {
            for (b, pb) in &pictures[i + 1..] {
                let differ = pa.iter().zip(pb).filter(|(x, y)| x != y).count() as f32 / pa.len() as f32;
                assert!(differ > 0.12, "{a:?} and {b:?} differ in only {differ}");
            }
        }
    }

    #[test]
    fn icons_stand_three_to_a_column() {
        assert_eq!(columns_width(0), 0.0);
        assert_eq!((columns_width(1), columns_width(3)), (TILE, TILE));
        assert_eq!(columns_width(4), 2.0 * TILE + GAP);
        assert_eq!(columns_width(10), 4.0 * TILE + 3.0 * GAP);
    }

    #[test]
    fn severities_look_different() {
        for kind in [TagKind::Thirst, TagKind::Health] {
            let looks: Vec<_> = (0..=4).map(|s| look(kind, s)).collect();
            for (i, a) in looks.iter().enumerate() {
                assert!(looks[i + 1..].iter().all(|b| a.0 != b.0), "{kind:?} severity {i}");
            }
        }
        // The worst stands out by its tile too, and sleep is its own colour whatever the level.
        assert_ne!(look(TagKind::Thirst, 4).1, look(TagKind::Thirst, 3).1);
        assert_eq!(look(TagKind::Asleep, 1), look(TagKind::Asleep, 3));
    }

    #[test]
    fn an_icon_pulses_when_it_appears_or_worsens_only() {
        let before = [tag(TagKind::Thirst, 1), tag(TagKind::Hunger, 2), tag(TagKind::Pain, 3)];
        // Thirst worsens, hunger eases, pain goes, bleeding starts.
        let now = [tag(TagKind::Thirst, 2), tag(TagKind::Hunger, 1), tag(TagKind::Bleeding, 1)];
        assert_eq!(worsened(&before, &now), [TagKind::Thirst, TagKind::Bleeding]);
        assert_eq!(worsened(&now, &now), []);
        // A good state turning bad is worse; turning good again isn't.
        assert_eq!(worsened(&[tag(TagKind::Hunger, 0)], &[tag(TagKind::Hunger, 1)]), [TagKind::Hunger]);
        assert_eq!(worsened(&[tag(TagKind::Hunger, 1)], &[tag(TagKind::Hunger, 0)]), []);
    }

    #[test]
    fn a_pulse_is_two_rings_and_then_nothing() {
        assert_eq!(pulse_ring(-0.1), (0.0, 0.0));
        assert_eq!(pulse_ring(1.0), (0.0, 0.0));
        let (start, mid, second) = (pulse_ring(0.0), pulse_ring(0.25), pulse_ring(0.5));
        assert!(start.0 == 0.0 && start.1 == 1.0, "{start:?}");
        assert!(mid.0 > start.0 && mid.1 < start.1, "moves out and fades: {mid:?}");
        assert!(second.0 == 0.0 && second.1 < start.1 && second.1 > mid.1, "the second ring is fainter than the first: {second:?}");
    }
}
