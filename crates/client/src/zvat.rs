//! `.zvat` files: a character baked offline for the crowd renderer by `tools/blender/zvat.py` (see
//! its docstring for the layout and `.claude/skills/meshy-assets/SKILL.md` for the pipeline), and the
//! base-colour texture that goes with one. [`crate::zombie_render`] turns them into its VAT.
//!
//! File (little-endian), version 1:
//! `b"ZVAT"`, u32 version, u32 vertices V, u32 indices I, u32 clips C;
//! C × clip: 16-byte name (UTF-8, zero padded), u32 frames, f32 seconds per play, f32 stride (metres
//! per loop at walking speed, 0 = fixed rate), u32 one-shot (0/1);
//! V × 2 f32 UV (glTF convention: v down); I × u32 triangle indices;
//! per clip, per frame, V × 3 f32 positions (game model space: metres, Y up, facing +Z, feet at 0).
//! There are no normals: the crowd shader shades flat from screen-space derivatives.
//!
//! The texture is looked for, in order: `ZOMBYE_CROWD_TEXTURE`, then beside the `.zvat` (same stem)
//! `<stem>.png`, `<stem>.jpg`, `<stem>_base_color.png`, `<stem>_base_color.jpg`, then the base-colour
//! image embedded in `<stem>.glb` (the GLB export `build.py` writes next to the `.zvat`).

use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageSamplerDescriptor, ImageType};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

const MAGIC: &[u8; 4] = b"ZVAT";
pub const VERSION: u32 = 1;
/// Sanity limits, so a broken file is refused rather than eating memory.
const MAX_VERTICES: u32 = 1 << 20;
const MAX_CLIPS: u32 = 64;
const MAX_FRAMES: u32 = 4096;
/// No baked position is further than this from the model's origin (metres).
const MAX_COORD: f32 = 100.0;

#[derive(Clone, Debug, PartialEq)]
pub struct ZvatClip {
    pub name: String,
    pub frames: u32,
    /// Seconds per play (a loop's cycle, or a one-shot's length).
    pub secs: f32,
    /// Metres per loop at walking speed; 0 = played at a fixed rate.
    pub stride: f32,
    pub one_shot: bool,
}

/// A parsed `.zvat`.
#[derive(Clone, Debug, PartialEq)]
pub struct Zvat {
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub clips: Vec<ZvatClip>,
    /// Per clip, per frame, one position per vertex.
    pub positions: Vec<Vec3>,
}

/// Clip names compare without case, spaces, `_` or `-` ("Get_Up" is "getup").
pub fn clip_key(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

impl Zvat {
    pub fn vertices(&self) -> usize {
        self.uvs.len()
    }

    /// The clip called `name` (see [`clip_key`]).
    pub fn clip(&self, name: &str) -> Option<usize> {
        let key = clip_key(name);
        self.clips.iter().position(|c| clip_key(&c.name) == key)
    }

    /// The positions of frame `frame` of clip `clip`.
    pub fn frame(&self, clip: usize, frame: u32) -> &[Vec3] {
        let v = self.vertices();
        let row = self.clips[..clip].iter().map(|c| c.frames as usize).sum::<usize>() + frame as usize;
        &self.positions[row * v..(row + 1) * v]
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != MAGIC {
            return Err("not a .zvat file (no ZVAT magic)".into());
        }
        let version = r.u32()?;
        if version != VERSION {
            return Err(format!("version {version}, this build reads {VERSION}"));
        }
        let (v, i, c) = (r.u32()?, r.u32()?, r.u32()?);
        if v == 0 || v > MAX_VERTICES {
            return Err(format!("{v} vertices (1 to {MAX_VERTICES})"));
        }
        if i == 0 || i % 3 != 0 || i > MAX_VERTICES * 8 {
            return Err(format!("{i} indices (a positive multiple of 3)"));
        }
        if c == 0 || c > MAX_CLIPS {
            return Err(format!("{c} clips (1 to {MAX_CLIPS})"));
        }
        let mut clips = Vec::with_capacity(c as usize);
        for k in 0..c {
            let raw = r.take(16)?;
            let end = raw.iter().position(|&b| b == 0).unwrap_or(16);
            let name = std::str::from_utf8(&raw[..end]).map_err(|_| format!("clip {k}: name isn't UTF-8"))?.to_string();
            let (frames, secs, stride, one_shot) = (r.u32()?, r.f32()?, r.f32()?, r.u32()?);
            if name.is_empty() || clips.iter().any(|o: &ZvatClip| clip_key(&o.name) == clip_key(&name)) {
                return Err(format!("clip {k}: name {name:?} empty or repeated"));
            }
            if frames == 0 || frames > MAX_FRAMES {
                return Err(format!("clip {name}: {frames} frames (1 to {MAX_FRAMES})"));
            }
            if !(secs.is_finite() && secs > 0.0 && secs < 1000.0) {
                return Err(format!("clip {name}: {secs} seconds"));
            }
            if !(stride.is_finite() && (0.0..100.0).contains(&stride)) {
                return Err(format!("clip {name}: stride {stride}"));
            }
            if one_shot > 1 {
                return Err(format!("clip {name}: one-shot flag {one_shot}"));
            }
            clips.push(ZvatClip { name, frames, secs, stride, one_shot: one_shot == 1 });
        }
        let rows: u64 = clips.iter().map(|c| c.frames as u64).sum();
        let expect = r.at as u64 + v as u64 * 8 + i as u64 * 4 + rows * v as u64 * 12;
        if bytes.len() as u64 != expect {
            return Err(format!("{} bytes, the header says {expect}", bytes.len()));
        }
        let mut uvs = Vec::with_capacity(v as usize);
        for _ in 0..v {
            let uv = [r.f32()?, r.f32()?];
            if !uv.iter().all(|x| x.is_finite()) {
                return Err("a UV isn't a number".into());
            }
            uvs.push(uv);
        }
        let mut indices = Vec::with_capacity(i as usize);
        for _ in 0..i {
            let k = r.u32()?;
            if k >= v {
                return Err(format!("index {k} out of range ({v} vertices)"));
            }
            indices.push(k);
        }
        let mut positions = Vec::with_capacity((rows * v as u64) as usize);
        for _ in 0..rows * v as u64 {
            let p = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
            if !(p.is_finite() && p.abs().max_element() < MAX_COORD) {
                return Err(format!("a position out of range: {p}"));
            }
            positions.push(p);
        }
        Ok(Self { uvs, indices, clips, positions })
    }

    /// The file `zvat.py` would write for this (tests build small ones with it).
    #[cfg(test)]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        for n in [VERSION, self.vertices() as u32, self.indices.len() as u32, self.clips.len() as u32] {
            out.extend_from_slice(&n.to_le_bytes());
        }
        for c in &self.clips {
            let mut name = [0u8; 16];
            let n = c.name.len().min(16);
            name[..n].copy_from_slice(&c.name.as_bytes()[..n]);
            out.extend_from_slice(&name);
            out.extend_from_slice(&c.frames.to_le_bytes());
            out.extend_from_slice(&c.secs.to_le_bytes());
            out.extend_from_slice(&c.stride.to_le_bytes());
            out.extend_from_slice(&(c.one_shot as u32).to_le_bytes());
        }
        for uv in &self.uvs {
            uv.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
        }
        self.indices.iter().for_each(|k| out.extend_from_slice(&k.to_le_bytes()));
        self.positions.iter().for_each(|p| p.to_array().iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes())));
        out
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let out = self.bytes.get(self.at..self.at + n).ok_or("file ends early")?;
        self.at += n;
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

// ---------------------------------------------------------------------------------------------
// The base-colour texture.

/// Largest texture side kept: a crowd zombie is at most a couple of hundred pixels tall on screen,
/// so bigger mip levels would never be sampled.
const MAX_TEXTURE: u32 = 1024;

/// Where the texture for the `.zvat` at `zvat` is, and its bytes: a file of its own, or the image
/// embedded in the GLB beside it. `None` if there is none.
pub fn find_texture(zvat: &Path) -> Option<(PathBuf, Vec<u8>, &'static str)> {
    let ext = |p: &Path| match p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("png") => "png",
        _ => "jpg",
    };
    if let Some(p) = std::env::var_os("ZOMBYE_CROWD_TEXTURE").map(PathBuf::from) {
        return std::fs::read(&p).ok().map(|b| {
            let e = ext(&p);
            (p, b, e)
        });
    }
    let dir = zvat.parent().unwrap_or(Path::new("."));
    let stem = zvat.file_stem()?.to_string_lossy().into_owned();
    for name in [format!("{stem}.png"), format!("{stem}.jpg"), format!("{stem}_base_color.png"), format!("{stem}_base_color.jpg")] {
        let p = dir.join(name);
        if let Ok(b) = std::fs::read(&p) {
            let e = ext(&p);
            return Some((p, b, e));
        }
    }
    let glb = dir.join(format!("{stem}.glb"));
    let bytes = std::fs::read(&glb).ok()?;
    let (image, mime) = glb_base_color(&bytes)?;
    Some((glb, image, if mime == "image/png" { "png" } else { "jpg" }))
}

/// The base-colour image of a GLB's first material (else its first image), and its MIME type.
fn glb_base_color(bytes: &[u8]) -> Option<(Vec<u8>, String)> {
    let gltf = gltf::Gltf::from_slice(bytes).ok()?;
    let blob = gltf.blob.as_deref()?;
    let image = gltf
        .materials()
        .find_map(|m| m.pbr_metallic_roughness().base_color_texture())
        .map(|t| t.texture().source())
        .or_else(|| gltf.images().next())?;
    match image.source() {
        gltf::image::Source::View { view, mime_type } => {
            let data = blob.get(view.offset()..view.offset() + view.length())?;
            Some((data.to_vec(), mime_type.to_string()))
        }
        gltf::image::Source::Uri { .. } => None,
    }
}

/// Decodes a PNG or JPEG into an sRGB texture with a full mip chain (averaged in linear light),
/// at most [`MAX_TEXTURE`] on a side, sampled with linear filtering.
pub fn texture_image(bytes: &[u8], ext: &str) -> Result<Image, String> {
    let decoded = Image::from_buffer(bytes, ImageType::Extension(ext), CompressedImageFormats::NONE, true, ImageSampler::Default, RenderAssetUsages::RENDER_WORLD)
        .map_err(|e| e.to_string())?;
    let decoded = decoded.convert(TextureFormat::Rgba8UnormSrgb).ok_or("can't convert the texture to RGBA8")?;
    let (w, h) = (decoded.width(), decoded.height());
    let data = decoded.data.as_deref().ok_or("texture has no data")?;
    Ok(mipmapped(w, h, data))
}

/// An sRGB RGBA8 image of `w × h` pixels with every mip level down to 1 × 1, dropping levels
/// above [`MAX_TEXTURE`].
pub fn mipmapped(w: u32, h: u32, data: &[u8]) -> Image {
    let to_linear: Vec<f32> = (0..256).map(|i| Color::srgb_u8(i as u8, 0, 0).to_linear().red).collect();
    let mut level: Vec<[f32; 4]> =
        data.as_chunks::<4>().0.iter().map(|p| [to_linear[p[0] as usize], to_linear[p[1] as usize], to_linear[p[2] as usize], p[3] as f32 / 255.0]).collect();
    let (mut lw, mut lh) = (w, h);
    let mut levels = Vec::new();
    loop {
        if lw <= MAX_TEXTURE && lh <= MAX_TEXTURE {
            levels.push((lw, lh, level.clone()));
        }
        if lw == 1 && lh == 1 {
            break;
        }
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let mut next = vec![[0.0f32; 4]; (nw * nh) as usize];
        for y in 0..nh {
            for x in 0..nw {
                let mut sum = [0.0f32; 4];
                let mut n = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((x * 2 + dx).min(lw - 1), (y * 2 + dy).min(lh - 1));
                    let p = level[(sy * lw + sx) as usize];
                    sum.iter_mut().zip(p).for_each(|(s, v)| *s += v);
                    n += 1.0;
                }
                next[(y * nw + x) as usize] = sum.map(|s| s / n);
            }
        }
        (lw, lh, level) = (nw, nh, next);
    }
    let (w0, h0) = (levels[0].0, levels[0].1);
    let mut bytes = Vec::new();
    for (_, _, px) in &levels {
        for p in px {
            let c = Color::linear_rgba(p[0], p[1], p[2], p[3]).to_srgba().to_u8_array();
            bytes.extend_from_slice(&c);
        }
    }
    let mut image = Image::new_uninit(Extent3d { width: w0, height: h0, depth_or_array_layers: 1 }, TextureDimension::D2, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
    image.texture_descriptor.mip_level_count = levels.len() as u32;
    image.data = Some(bytes);
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor::linear());
    image
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn tiny() -> Zvat {
        let clips = vec![
            ZvatClip { name: "walk".into(), frames: 3, secs: 1.2, stride: 0.8, one_shot: false },
            ZvatClip { name: "idle".into(), frames: 2, secs: 3.0, stride: 0.0, one_shot: false },
        ];
        let base = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.3, 0.0, 0.0), Vec3::new(0.0, 1.7, 0.1)];
        let positions = (0..5).flat_map(|f| base.map(|p| p + Vec3::Z * f as f32 * 0.01)).collect();
        Zvat { uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]], indices: vec![0, 1, 2], clips, positions }
    }

    #[test]
    fn a_zvat_reads_back_what_was_written() {
        let z = tiny();
        let bytes = z.to_bytes();
        assert_eq!(bytes.len(), 20 + 2 * 32 + 3 * 8 + 3 * 4 + 5 * 3 * 12);
        let back = Zvat::parse(&bytes).unwrap();
        assert_eq!(back, z);
        assert_eq!(back.clip("Idle"), Some(1));
        assert_eq!(back.frame(1, 1)[2], Vec3::new(0.0, 1.7, 0.1 + 0.04));
        assert_eq!(clip_key("Die_Back"), "dieback");
    }

    #[test]
    fn broken_zvats_are_refused() {
        let good = tiny().to_bytes();
        assert!(Zvat::parse(&good[..good.len() - 1]).is_err(), "truncated");
        assert!(Zvat::parse(b"ZVAX").is_err());
        let mut v2 = good.clone();
        v2[4] = 2;
        assert!(Zvat::parse(&v2).unwrap_err().contains("version"));
        let mut z = tiny();
        z.indices[1] = 9;
        assert!(Zvat::parse(&z.to_bytes()).unwrap_err().contains("out of range"));
        let mut z = tiny();
        z.positions[4].y = f32::NAN;
        assert!(Zvat::parse(&z.to_bytes()).is_err());
        let mut z = tiny();
        z.clips[1].name = "WALK".into();
        assert!(Zvat::parse(&z.to_bytes()).unwrap_err().contains("repeated"));
        let mut z = tiny();
        z.clips[0].frames = 0;
        assert!(Zvat::parse(&z.to_bytes()).is_err());
    }

    #[test]
    fn textures_get_a_full_mip_chain() {
        let (w, h) = (8, 4);
        let data: Vec<u8> = (0..w * h).flat_map(|i| if i % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] }).collect();
        let image = mipmapped(w, h, &data);
        assert_eq!(image.texture_descriptor.mip_level_count, 4, "8x4, 4x2, 2x1, 1x1");
        let bytes = image.data.as_ref().unwrap();
        assert_eq!(bytes.len(), 4 * (32 + 8 + 2 + 1));
        // Half white, half black averages to half the light: sRGB ~188, not 128.
        let last = &bytes[bytes.len() - 4..];
        assert!((185..=190).contains(&last[0]), "{last:?}");
    }
}
