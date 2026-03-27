use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

/// Material extension that performs GPU-side cylindrical Y-axis billboarding.
///
/// Inserts a Y-rotation into the model matrix in the vertex shader so quads
/// always face the camera in the XZ plane. Preserves existing bone rotations
/// (Z-rotations appear as screen-plane tilt). Handles both skinned and
/// non-skinned meshes.
pub type BillboardMaterial = ExtendedMaterial<StandardMaterial, BillboardExt>;

/// Marker extension for GPU billboard vertex shader. Contains no
/// additional uniforms -- camera data comes from Bevy's view uniform.
#[derive(AsBindGroup, Asset, TypePath, Clone, Default)]
pub struct BillboardExt {}

impl MaterialExtension for BillboardExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/billboard.wgsl".into()
    }

    /// Disabled: the prepass uses StandardMaterial's vertex shader which doesn't
    /// billboard, producing depth values that conflict with the main pass billboard
    /// positions.
    fn enable_prepass() -> bool {
        false
    }

    /// Disabled: shadow pass has the same vertex shader mismatch as prepass.
    fn enable_shadows() -> bool {
        false
    }
}
