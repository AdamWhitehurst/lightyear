/// A voxel within a `.vox` model, either empty or filled with a palette index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VoxModelVoxel {
    #[default]
    Empty,
    /// Filled with a palette index (0..=254).
    Filled(u8),
}

impl block_mesh::Voxel for VoxModelVoxel {
    fn get_visibility(&self) -> block_mesh::VoxelVisibility {
        match self {
            Self::Empty => block_mesh::VoxelVisibility::Empty,
            Self::Filled(_) => block_mesh::VoxelVisibility::Opaque,
        }
    }
}

impl block_mesh::MergeVoxel for VoxModelVoxel {
    type MergeValue = u8;
    type MergeValueFacingNeighbour = u8;

    fn merge_value(&self) -> u8 {
        match self {
            Self::Filled(idx) => *idx,
            Self::Empty => 0,
        }
    }

    fn merge_value_facing_neighbour(&self) -> u8 {
        self.merge_value()
    }
}
