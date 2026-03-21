use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use bevy::log::info_span;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};

use crate::config::VoxelGenerator;
use crate::meshing::mesh_chunk_greedy;
use crate::types::{ChunkData, FillType, WorldVoxel};

/// Result of an async chunk generation task.
pub struct ChunkGenResult {
    pub position: IVec3,
    pub mesh: Option<Mesh>,
    pub chunk_data: ChunkData,
    /// Whether this chunk was loaded from disk rather than generated.
    pub from_disk: bool,
}

/// Pending async chunk generation tasks for a map entity.
#[derive(Component, Default)]
pub struct PendingChunks {
    pub tasks: Vec<Task<ChunkGenResult>>,
    pub pending_positions: HashSet<IVec3>,
}

/// Spawn an async task that loads a chunk from disk (if available) or generates it.
pub fn spawn_chunk_gen_task(
    pending: &mut PendingChunks,
    position: IVec3,
    generator: &VoxelGenerator,
    save_dir: Option<PathBuf>,
) {
    let generator = Arc::clone(&generator.0);
    let pool = AsyncComputeTaskPool::get();

    let task = pool.spawn(async move {
        if let Some(ref dir) = save_dir {
            match crate::persistence::load_chunk(dir, position) {
                Ok(Some(chunk_data)) => {
                    let mesh = if chunk_data.fill_type == FillType::Empty {
                        None
                    } else {
                        let voxels = {
                            let _span = info_span!("disk_load_expand").entered();
                            chunk_data.voxels.to_voxels()
                        };
                        let _span = info_span!("mesh_chunk").entered();
                        mesh_chunk_greedy(&voxels)
                    };
                    return ChunkGenResult {
                        position,
                        mesh,
                        chunk_data,
                        from_disk: true,
                    };
                }
                Ok(None) => {} // No saved file, generate fresh
                Err(e) => {
                    bevy::log::warn!("Failed to load chunk at {position}: {e}, regenerating");
                }
            }
        }

        generate_chunk(position, &*generator)
    });

    pending.tasks.push(task);
    pending.pending_positions.insert(position);
}

fn generate_chunk(position: IVec3, generator: &dyn Fn(IVec3) -> Vec<WorldVoxel>) -> ChunkGenResult {
    let voxels = {
        let _span = info_span!("terrain_gen").entered();
        generator(position)
    };
    let chunk_data = {
        let _span = info_span!("palettize_chunk").entered();
        ChunkData::from_voxels(&voxels)
    };
    let mesh = if chunk_data.fill_type == FillType::Empty {
        None
    } else {
        let _span = info_span!("mesh_chunk").entered();
        mesh_chunk_greedy(&voxels)
    };
    ChunkGenResult {
        position,
        mesh,
        chunk_data,
        from_disk: false,
    }
}
