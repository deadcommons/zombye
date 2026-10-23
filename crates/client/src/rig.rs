//! Grey-box characters: boxes attached to a small skeleton, posed by code.
//!
//! Zombies and survivors are both built this way. A rig is a list of bones (parent and rest pivot in
//! model space; the model faces +Z with its feet at y = 0) and a list of boxes, each following one
//! bone. A pose is one local rotation per bone plus an offset for the root. Zombies bake their poses
//! into a vertex animation texture ([`crate::zombie_render`]); survivors are a skinned mesh whose
//! joints are posed every frame ([`crate::survivor`]).

use bevy::prelude::*;

/// (parent, rest pivot in model space). Parents come before their children.
pub type Bone = (Option<usize>, Vec3);

/// A pose: local rotation per bone (about its pivot) and an offset added to the root's pivot.
#[derive(Clone, Debug)]
pub struct Pose<const N: usize> {
    pub rot: [Quat; N],
    pub offset: Vec3,
}

impl<const N: usize> Default for Pose<N> {
    fn default() -> Self {
        Self { rot: [Quat::IDENTITY; N], offset: Vec3::ZERO }
    }
}

impl<const N: usize> Pose<N> {
    /// Blend toward `other` by `t` (0 = self, 1 = other), bone by bone.
    pub fn blend(&self, other: &Self, t: f32) -> Self {
        if t <= 0.0 {
            return self.clone();
        }
        if t >= 1.0 {
            return other.clone();
        }
        let mut out = self.clone();
        for (r, o) in out.rot.iter_mut().zip(other.rot.iter()) {
            *r = r.slerp(*o, t);
        }
        out.offset = self.offset.lerp(other.offset, t);
        out
    }

    /// Blend only the bones in `mask` toward `other` by `t` (the root offset too, if `offset`).
    pub fn blend_bones(&mut self, other: &Self, t: f32, mask: &[usize], offset: bool) {
        if t <= 0.0 {
            return;
        }
        for &b in mask {
            self.rot[b] = self.rot[b].slerp(other.rot[b], t.min(1.0));
        }
        if offset {
            self.offset = self.offset.lerp(other.offset, t.min(1.0));
        }
    }
}

/// Model-space transform of every bone in `pose` (rest pivot included).
pub fn globals<const N: usize>(bones: &[Bone; N], pose: &Pose<N>) -> [Mat4; N] {
    let mut global = [Mat4::IDENTITY; N];
    for (b, (parent, pivot)) in bones.iter().enumerate() {
        let local_pivot = match parent {
            Some(pa) => *pivot - bones[*pa].1,
            None => *pivot + pose.offset,
        };
        let local = Mat4::from_rotation_translation(pose.rot[b], local_pivot);
        global[b] = match parent {
            Some(pa) => global[*pa] * local,
            None => local,
        };
    }
    global
}

/// Skinning matrix per bone: `global_pose * inverse(global_rest)`, mapping rest-pose model space to
/// posed model space.
pub fn skin_matrices<const N: usize>(bones: &[Bone; N], pose: &Pose<N>) -> [Mat4; N] {
    let mut out = globals(bones, pose);
    for (m, (_, pivot)) in out.iter_mut().zip(bones.iter()) {
        *m *= Mat4::from_translation(-*pivot);
    }
    out
}

/// The six faces of an axis-aligned box as (corners counter-clockwise from outside, normal).
pub fn box_faces(min: Vec3, max: Vec3) -> [([Vec3; 4], Vec3); 6] {
    let c = |x: bool, y: bool, z: bool| Vec3::new(if x { max.x } else { min.x }, if y { max.y } else { min.y }, if z { max.z } else { min.z });
    [
        ([c(true, false, false), c(true, true, false), c(true, true, true), c(true, false, true)], Vec3::X),
        ([c(false, false, true), c(false, true, true), c(false, true, false), c(false, false, false)], Vec3::NEG_X),
        ([c(false, true, false), c(false, true, true), c(true, true, true), c(true, true, false)], Vec3::Y),
        ([c(false, false, true), c(false, false, false), c(true, false, false), c(true, false, true)], Vec3::NEG_Y),
        ([c(true, false, true), c(true, true, true), c(false, true, true), c(false, false, true)], Vec3::Z),
        ([c(false, false, false), c(false, true, false), c(true, true, false), c(true, false, false)], Vec3::NEG_Z),
    ]
}

/// Shorthands for authoring poses: rotations about the bone's local axes. Positive X swings a
/// hanging limb backward (and tips the torso forward); positive Z swings it toward +X (the
/// model's left); positive Y turns +Z toward +X.
pub fn rx(a: f32) -> Quat {
    Quat::from_rotation_x(a)
}
pub fn ry(a: f32) -> Quat {
    Quat::from_rotation_y(a)
}
pub fn rz(a: f32) -> Quat {
    Quat::from_rotation_z(a)
}

/// Smooth 0..1 ramp of `x` between `a` and `b`.
pub fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear interpolation helper for angles and distances.
pub fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_pose_skins_to_identity() {
        let bones: [Bone; 2] = [(None, Vec3::new(0.0, 1.0, 0.0)), (Some(0), Vec3::new(0.2, 1.4, 0.0))];
        for m in skin_matrices(&bones, &Pose::default()) {
            assert!(m.abs_diff_eq(Mat4::IDENTITY, 1e-6));
        }
    }

    #[test]
    fn positive_x_swings_a_hanging_limb_backward() {
        let tip = rx(0.5) * Vec3::NEG_Y;
        assert!(tip.z < 0.0);
        let tip = rz(0.5) * Vec3::NEG_Y;
        assert!(tip.x > 0.0);
        let fwd = ry(0.5) * Vec3::Z;
        assert!(fwd.x > 0.0);
    }
}
