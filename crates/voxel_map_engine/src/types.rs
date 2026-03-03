use bevy::prelude::*;
use ndshape::ConstShape;
use serde::{Deserialize, Serialize};

/// 16^3 voxel chunks with 1-voxel padding on each side -> 18^3 padded array
pub type PaddedChunkShape = ndshape::ConstShape3u32<18, 18, 18>;

pub const CHUNK_SIZE: u32 = 16;
pub const PADDED_CHUNK_SIZE: u32 = 18;

/// Voxel data stored per position
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum WorldVoxel {
    Air,
    Unset,
    Solid(u8),
}

impl Default for WorldVoxel {
    fn default() -> Self {
        Self::Unset
    }
}

/// How a chunk is filled (optimization for uniform chunks)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillType {
    Empty,
    Mixed,
    Uniform(WorldVoxel),
}

/// Voxel data for one chunk (16^3 with 1-voxel padding = 18^3)
#[derive(Clone)]
pub struct ChunkData {
    pub voxels: Vec<WorldVoxel>,
    pub fill_type: FillType,
    pub hash: u64,
}

impl ChunkData {
    pub fn new_empty() -> Self {
        Self {
            voxels: vec![WorldVoxel::Air; PaddedChunkShape::SIZE as usize],
            fill_type: FillType::Empty,
            hash: 0,
        }
    }
}

/// Network-serializable voxel type (mirrors WorldVoxel without Unset)
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub enum VoxelType {
    Air,
    Solid(u8),
}

impl From<VoxelType> for WorldVoxel {
    fn from(v: VoxelType) -> Self {
        match v {
            VoxelType::Air => WorldVoxel::Air,
            VoxelType::Solid(m) => WorldVoxel::Solid(m),
        }
    }
}

impl From<WorldVoxel> for VoxelType {
    fn from(v: WorldVoxel) -> Self {
        match v {
            WorldVoxel::Air | WorldVoxel::Unset => VoxelType::Air,
            WorldVoxel::Solid(m) => VoxelType::Solid(m),
        }
    }
}

impl block_mesh::Voxel for WorldVoxel {
    fn get_visibility(&self) -> block_mesh::VoxelVisibility {
        match self {
            WorldVoxel::Air | WorldVoxel::Unset => block_mesh::VoxelVisibility::Empty,
            WorldVoxel::Solid(_) => block_mesh::VoxelVisibility::Opaque,
        }
    }
}

impl block_mesh::MergeVoxel for WorldVoxel {
    type MergeValue = u8;
    type MergeValueFacingNeighbour = u8;

    fn merge_value(&self) -> u8 {
        match self {
            WorldVoxel::Solid(m) => *m,
            _ => 0,
        }
    }

    fn merge_value_facing_neighbour(&self) -> u8 {
        self.merge_value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_chunk_shape_size() {
        assert_eq!(PaddedChunkShape::SIZE, 18 * 18 * 18);
    }

    #[test]
    fn padded_chunk_shape_linearize_roundtrip() {
        let point = [5u32, 10, 3];
        let index = PaddedChunkShape::linearize(point);
        let back = PaddedChunkShape::delinearize(index);
        assert_eq!(point, back);
    }

    #[test]
    fn chunk_data_new_empty() {
        let chunk = ChunkData::new_empty();
        assert_eq!(chunk.voxels.len(), PaddedChunkShape::USIZE);
        assert_eq!(chunk.fill_type, FillType::Empty);
        assert!(chunk.voxels.iter().all(|v| *v == WorldVoxel::Air));
    }

    #[test]
    fn world_voxel_to_voxel_type_roundtrip() {
        let air: VoxelType = WorldVoxel::Air.into();
        assert_eq!(air, VoxelType::Air);
        let back: WorldVoxel = air.into();
        assert_eq!(back, WorldVoxel::Air);

        let solid: VoxelType = WorldVoxel::Solid(42).into();
        assert_eq!(solid, VoxelType::Solid(42));
        let back: WorldVoxel = solid.into();
        assert_eq!(back, WorldVoxel::Solid(42));
    }

    #[test]
    fn world_voxel_unset_maps_to_air() {
        let vt: VoxelType = WorldVoxel::Unset.into();
        assert_eq!(vt, VoxelType::Air);
    }
}
