use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

/// Material extension for GPU-billboarded sprite rig meshes (skinned).
///
/// Uses a dedicated vertex shader that correctly decomposes bone Z-rotation
/// from joint hierarchies containing arbitrary Y-rotations and facing mirrors.
pub type SpriteRigMaterial = ExtendedMaterial<StandardMaterial, SpriteRigBillboardExt>;

/// Marker extension for sprite rig billboard vertex shader.
#[derive(AsBindGroup, Asset, TypePath, Clone, Default)]
pub struct SpriteRigBillboardExt {}

impl MaterialExtension for SpriteRigBillboardExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/sprite_rig_billboard.wgsl".into()
    }

    /// Disabled: prepass uses StandardMaterial's vertex shader which doesn't
    /// billboard, producing conflicting depth values.
    fn enable_prepass() -> bool {
        false
    }

    /// Disabled: shadow pass has the same vertex shader mismatch as prepass.
    fn enable_shadows() -> bool {
        false
    }
}
