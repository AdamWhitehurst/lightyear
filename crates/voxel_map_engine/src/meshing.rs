use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use fast_surface_nets::{SurfaceNetsBuffer, surface_nets};
use ndshape::ConstShape;

use crate::types::{CHUNK_SIZE, PaddedChunkShape};

/// Trait for meshing chunk SDF data into a Bevy Mesh.
pub trait VoxelMesher: Send + Sync {
    fn mesh_chunk(&self, sdf: &[f32]) -> Option<Mesh>;
}

/// Smooth terrain mesher using fast-surface-nets.
pub struct SurfaceNetsMesher;

impl VoxelMesher for SurfaceNetsMesher {
    fn mesh_chunk(&self, sdf: &[f32]) -> Option<Mesh> {
        debug_assert_eq!(sdf.len(), PaddedChunkShape::USIZE);

        let mut buffer = SurfaceNetsBuffer::default();
        surface_nets(sdf, &PaddedChunkShape {}, [0; 3], [17; 3], &mut buffer);

        if buffer.indices.is_empty() {
            return None;
        }

        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, buffer.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, buffer.normals);
        mesh.insert_indices(Indices::U32(buffer.indices));
        Some(mesh)
    }
}

/// Generate SDF for flat terrain at y=0 for the given chunk position.
/// Negative = solid (below surface), positive = air (above surface).
pub fn flat_terrain_sdf(chunk_pos: IVec3) -> Vec<f32> {
    let mut sdf = vec![0.0f32; PaddedChunkShape::USIZE];
    for i in 0..PaddedChunkShape::SIZE {
        let [_x, y, _z] = PaddedChunkShape::delinearize(i);
        let world_y = chunk_pos.y * CHUNK_SIZE as i32 + y as i32 - 1;
        sdf[i as usize] = world_y as f32;
    }
    sdf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_terrain_sdf_produces_mesh_at_surface() {
        let sdf = flat_terrain_sdf(IVec3::new(0, 0, 0));
        let mesh = SurfaceNetsMesher.mesh_chunk(&sdf);
        assert!(
            mesh.is_some(),
            "y=0 chunk should contain a surface crossing"
        );
    }

    #[test]
    fn flat_terrain_sdf_no_mesh_for_underground() {
        let sdf = flat_terrain_sdf(IVec3::new(0, -2, 0));
        let mesh = SurfaceNetsMesher.mesh_chunk(&sdf);
        assert!(
            mesh.is_none(),
            "fully underground chunk should produce no mesh"
        );
    }

    #[test]
    fn flat_terrain_sdf_no_mesh_for_sky() {
        let sdf = flat_terrain_sdf(IVec3::new(0, 2, 0));
        let mesh = SurfaceNetsMesher.mesh_chunk(&sdf);
        assert!(
            mesh.is_none(),
            "fully above-surface chunk should produce no mesh"
        );
    }

    #[test]
    fn mesh_has_valid_attributes() {
        let sdf = flat_terrain_sdf(IVec3::new(0, 0, 0));
        let mesh = SurfaceNetsMesher
            .mesh_chunk(&sdf)
            .expect("should produce mesh");
        assert!(mesh.attribute(Mesh::ATTRIBUTE_POSITION).is_some());
        assert!(mesh.attribute(Mesh::ATTRIBUTE_NORMAL).is_some());
        assert!(mesh.indices().is_some());
    }
}
