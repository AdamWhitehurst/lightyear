use std::collections::HashSet;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};

use crate::config::VoxelGenerator;
use crate::meshing::mesh_chunk_greedy;
use crate::types::WorldVoxel;

/// Result of an async chunk generation task.
pub struct ChunkGenResult {
    pub position: IVec3,
    pub mesh: Option<Mesh>,
    pub voxels: Vec<WorldVoxel>,
}

/// Pending async chunk generation tasks for a map entity.
#[derive(Component, Default)]
pub struct PendingChunks {
    pub tasks: Vec<Task<ChunkGenResult>>,
    pub pending_positions: HashSet<IVec3>,
}

/// Spawn an async task that generates voxel data and meshes a chunk.
pub fn spawn_chunk_gen_task(
    pending: &mut PendingChunks,
    position: IVec3,
    generator: &VoxelGenerator,
) {
    let generator = Arc::clone(generator);
    let pool = AsyncComputeTaskPool::get();

    let task = pool.spawn(async move { generate_chunk(position, &generator) });

    pending.tasks.push(task);
    pending.pending_positions.insert(position);
}

fn generate_chunk(position: IVec3, generator: &VoxelGenerator) -> ChunkGenResult {
    let voxels = generator(position);
    let mesh = mesh_chunk_greedy(&voxels);
    ChunkGenResult {
        position,
        mesh,
        voxels,
    }
}
