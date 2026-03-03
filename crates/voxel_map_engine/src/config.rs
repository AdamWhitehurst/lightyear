use bevy::prelude::*;
use std::sync::Arc;

use crate::types::WorldVoxel;

/// Generation function: given chunk position, returns voxel data for the padded 18^3 array.
pub type VoxelGenerator = Arc<dyn Fn(IVec3) -> Vec<WorldVoxel> + Send + Sync>;

/// Configuration for a map instance.
#[derive(Component)]
pub struct VoxelMapConfig {
    pub seed: u64,
    pub spawning_distance: u32,
    pub bounds: Option<IVec3>,
    pub tree_height: u32,
    pub generator: VoxelGenerator,
}

impl VoxelMapConfig {
    pub fn new(
        seed: u64,
        spawning_distance: u32,
        bounds: Option<IVec3>,
        tree_height: u32,
        generator: VoxelGenerator,
    ) -> Self {
        debug_assert!(tree_height > 0, "VoxelMapConfig: tree_height must be > 0");
        debug_assert!(
            spawning_distance > 0,
            "VoxelMapConfig: spawning_distance must be > 0"
        );
        if let Some(b) = bounds {
            debug_assert!(
                b.x > 0 && b.y > 0 && b.z > 0,
                "VoxelMapConfig: bounded maps must have all-positive bounds, got {b}"
            );
        }
        Self {
            seed,
            spawning_distance,
            bounds,
            tree_height,
            generator,
        }
    }
}
