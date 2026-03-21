use bevy::log::info_span;
use bevy::prelude::*;
use bevy::tasks::futures::check_ready;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use std::collections::{HashMap, HashSet};

use crate::chunk::{ChunkTarget, VoxelChunk};
use crate::config::{VoxelGenerator, VoxelMapConfig};
use crate::generation::{PendingChunks, spawn_chunk_gen_task};
use crate::instance::VoxelMapInstance;
use crate::meshing::mesh_chunk_greedy;
use crate::types::CHUNK_SIZE;

const MAX_TASKS_PER_FRAME: usize = 32;

/// Default PBR material applied to voxel chunk meshes.
#[derive(Resource)]
pub struct DefaultVoxelMaterial(pub Handle<StandardMaterial>);

/// Startup system that creates the default voxel material.
pub fn init_default_material(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let handle = materials.add(StandardMaterial {
        base_color: Color::srgb(0.5, 0.7, 0.3),
        perceptual_roughness: 0.9,
        ..default()
    });
    commands.insert_resource(DefaultVoxelMaterial(handle));
}

/// A pending async remesh task for a chunk mutated in-place.
struct RemeshTask {
    chunk_pos: IVec3,
    task: Task<Option<Mesh>>,
}

/// Component tracking pending remesh tasks for a map instance.
#[derive(Component, Default)]
pub struct PendingRemeshes {
    tasks: Vec<RemeshTask>,
}

/// Auto-insert `PendingChunks` and `PendingRemeshes` on map entities that lack them.
///
/// Gated on `With<VoxelGenerator>` — maps without a generator don't start loading chunks.
pub fn ensure_pending_chunks(
    mut commands: Commands,
    chunks_query: Query<
        Entity,
        (
            With<VoxelMapInstance>,
            With<VoxelGenerator>,
            Without<PendingChunks>,
        ),
    >,
    remesh_query: Query<
        Entity,
        (
            With<VoxelMapInstance>,
            With<VoxelGenerator>,
            Without<PendingRemeshes>,
        ),
    >,
) {
    for entity in &chunks_query {
        info!("ensure_pending_chunks: adding PendingChunks to {entity:?}");
        commands.entity(entity).insert(PendingChunks::default());
    }
    for entity in &remesh_query {
        info!("ensure_pending_chunks: adding PendingRemeshes to {entity:?}");
        commands.entity(entity).insert(PendingRemeshes::default());
    }
}

/// Per-target cached state for desired chunk positions.
///
/// Opaque to external consumers; only exposed as `pub` because Bevy's system
/// parameter inference requires the type to be visible at the function's visibility.
pub struct TargetCache {
    chunk_pos: IVec3,
    map_entity: Entity,
    distance: u32,
    desired: HashSet<IVec3>,
}

/// Determine which chunk positions should be loaded based on all targets for a map.
/// Spawn async generation tasks for missing chunks and mark out-of-range chunks for removal.
///
/// Caches each target's desired chunk set and only recomputes when the target
/// crosses a chunk boundary, changes map, or changes distance.
pub fn update_chunks(
    mut map_query: Query<(
        Entity,
        &mut VoxelMapInstance,
        &VoxelMapConfig,
        &VoxelGenerator,
        &mut PendingChunks,
        &GlobalTransform,
    )>,
    target_query: Query<(Entity, &ChunkTarget, &GlobalTransform)>,
    mut tick: Local<u32>,
    mut target_cache: Local<HashMap<Entity, TargetCache>>,
    mut desired_cache: Local<HashMap<Entity, HashSet<IVec3>>>,
) {
    let map_count = map_query.iter().count();
    *tick += 1;
    if map_count > 0 && *tick % 300 == 0 {
        trace!("update_chunks: iterating {map_count} map(s)");
    }

    let mut maps_needing_update = purge_stale_targets(&target_query, &mut target_cache);

    for (map_entity, mut instance, config, generator, mut pending, map_transform) in &mut map_query
    {
        let map_inv = map_transform.affine().inverse();
        let targets_changed = update_target_caches_for_map(
            map_entity,
            &map_inv,
            config.bounds,
            &target_query,
            &mut target_cache,
        );
        if targets_changed {
            maps_needing_update.insert(map_entity);
        }

        if maps_needing_update.contains(&map_entity) {
            let desired = {
                let _span = info_span!("collect_desired_positions").entered();
                union_desired_from_cache(&target_cache, map_entity)
            };

            if desired.is_empty() && !instance.loaded_chunks.is_empty() {
                info!(
                    "update_chunks: map {map_entity:?} has {} loaded chunks but 0 desired — will clean up",
                    instance.loaded_chunks.len()
                );
            }

            {
                let _span = info_span!("remove_out_of_range_chunks").entered();
                remove_out_of_range_chunks(&mut instance, &desired, config.save_dir.as_deref());
            }

            desired_cache.insert(map_entity, desired);
        }

        if config.generates_chunks {
            if let Some(desired) = desired_cache.get(&map_entity) {
                spawn_missing_chunks(&mut instance, &mut pending, config, generator, desired);
            }
        }
    }
}

/// Remove cache entries for despawned targets. Returns the set of map entities
/// that had targets removed (and thus need their desired sets rebuilt).
fn purge_stale_targets(
    target_query: &Query<(Entity, &ChunkTarget, &GlobalTransform)>,
    cache: &mut HashMap<Entity, TargetCache>,
) -> HashSet<Entity> {
    let active: HashSet<Entity> = target_query.iter().map(|(e, _, _)| e).collect();
    let stale: Vec<Entity> = cache
        .keys()
        .filter(|e| !active.contains(e))
        .copied()
        .collect();
    let mut affected_maps = HashSet::new();
    for entity in stale {
        if let Some(cached) = cache.remove(&entity) {
            affected_maps.insert(cached.map_entity);
        }
    }
    affected_maps
}

/// Check each target on this map and update its cache if its chunk position changed.
/// Returns true if any target was added or changed.
fn update_target_caches_for_map(
    map_entity: Entity,
    map_inv: &bevy::math::Affine3A,
    bounds: Option<IVec3>,
    target_query: &Query<(Entity, &ChunkTarget, &GlobalTransform)>,
    cache: &mut HashMap<Entity, TargetCache>,
) -> bool {
    let mut changed = false;

    for (target_entity, target, transform) in target_query.iter() {
        if target.map_entity != map_entity {
            continue;
        }

        let local_pos = map_inv.transform_point3(transform.translation());
        let chunk_pos = world_to_chunk_pos(local_pos);

        let needs_update = match cache.get(&target_entity) {
            Some(cached) => {
                cached.chunk_pos != chunk_pos
                    || cached.map_entity != map_entity
                    || cached.distance != target.distance
            }
            None => true,
        };

        if needs_update {
            let desired = compute_target_desired(chunk_pos, target.distance, bounds);
            cache.insert(
                target_entity,
                TargetCache {
                    chunk_pos,
                    map_entity,
                    distance: target.distance,
                    desired,
                },
            );
            changed = true;
        }
    }

    changed
}

/// Compute the set of chunk positions desired by a single target.
fn compute_target_desired(center: IVec3, distance: u32, bounds: Option<IVec3>) -> HashSet<IVec3> {
    let dist = distance as i32;
    let mut desired = HashSet::new();
    for x in -dist..=dist {
        for y in -dist..=dist {
            for z in -dist..=dist {
                let pos = center + IVec3::new(x, y, z);
                if is_within_bounds(pos, bounds) {
                    desired.insert(pos);
                }
            }
        }
    }
    desired
}

/// Union all cached desired sets for a given map.
fn union_desired_from_cache(
    cache: &HashMap<Entity, TargetCache>,
    map_entity: Entity,
) -> HashSet<IVec3> {
    cache
        .values()
        .filter(|c| c.map_entity == map_entity)
        .flat_map(|c| c.desired.iter().copied())
        .collect()
}

fn world_to_chunk_pos(translation: Vec3) -> IVec3 {
    (translation / CHUNK_SIZE as f32).floor().as_ivec3()
}

fn is_within_bounds(pos: IVec3, bounds: Option<IVec3>) -> bool {
    match bounds {
        Some(b) => pos.x.abs() < b.x && pos.y.abs() < b.y && pos.z.abs() < b.z,
        None => true,
    }
}

fn remove_out_of_range_chunks(
    instance: &mut VoxelMapInstance,
    desired: &HashSet<IVec3>,
    save_dir: Option<&std::path::Path>,
) {
    let removed: Vec<IVec3> = instance
        .loaded_chunks
        .iter()
        .filter(|pos| !desired.contains(pos))
        .copied()
        .collect();
    for pos in removed {
        if instance.dirty_chunks.remove(&pos) {
            if let Some(dir) = save_dir {
                if let Some(chunk_data) = instance.get_chunk_data(pos) {
                    if let Err(e) = crate::persistence::save_chunk(dir, pos, chunk_data) {
                        error!("Failed to save evicted dirty chunk at {pos}: {e}");
                    }
                }
            }
        }
        instance.loaded_chunks.remove(&pos);
        instance.remove_chunk_data(pos);
    }
}

fn spawn_missing_chunks(
    instance: &mut VoxelMapInstance,
    pending: &mut PendingChunks,
    config: &VoxelMapConfig,
    generator: &VoxelGenerator,
    desired: &HashSet<IVec3>,
) {
    let mut spawned = 0;

    for &pos in desired {
        if spawned >= MAX_TASKS_PER_FRAME {
            break;
        }
        if instance.loaded_chunks.contains(&pos) {
            continue;
        }
        if is_already_pending(pending, pos) {
            continue;
        }

        spawn_chunk_gen_task(pending, pos, generator, config.save_dir.clone());
        spawned += 1;
    }
}

fn is_already_pending(pending: &PendingChunks, pos: IVec3) -> bool {
    pending.pending_positions.contains(&pos)
}

/// Poll pending chunk generation tasks and spawn mesh entities for completed ones.
pub fn poll_chunk_tasks(
    mut commands: Commands,
    mut map_query: Query<(Entity, &mut VoxelMapInstance, &mut PendingChunks)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    default_material: Option<Res<DefaultVoxelMaterial>>,
) {
    let Some(default_material) = default_material else {
        warn!("DefaultVoxelMaterial resource not found; chunk meshes will not spawn");
        return;
    };

    for (map_entity, mut instance, mut pending) in &mut map_query {
        let mut i = 0;
        while i < pending.tasks.len() {
            if let Some(result) = check_ready(&mut pending.tasks[i]) {
                let _ = pending.tasks.swap_remove(i);
                debug_assert!(
                    pending.pending_positions.contains(&result.position),
                    "poll_chunk_tasks: completed chunk at {:?} was not in pending_positions",
                    result.position
                );
                pending.pending_positions.remove(&result.position);
                handle_completed_chunk(
                    &mut commands,
                    &mut instance,
                    &mut *meshes,
                    &mut *materials,
                    &*default_material,
                    map_entity,
                    result,
                );
            } else {
                i += 1;
            }
        }
    }
}

fn color_from_chunk_pos(pos: IVec3) -> Color {
    let hash = (pos.x.wrapping_mul(73856093))
        ^ (pos.y.wrapping_mul(19349663))
        ^ (pos.z.wrapping_mul(83492791));
    let hue = ((hash as u32) % 360) as f32;
    Color::hsl(hue, 0.5, 0.5)
}

fn handle_completed_chunk(
    commands: &mut Commands,
    instance: &mut VoxelMapInstance,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    default_material: &DefaultVoxelMaterial,
    map_entity: Entity,
    result: crate::generation::ChunkGenResult,
) {
    instance.loaded_chunks.insert(result.position);
    instance.insert_chunk_data(result.position, result.chunk_data);

    let Some(mesh) = result.mesh else {
        return;
    };

    let mesh_handle = meshes.add(mesh);
    let offset = chunk_world_offset(result.position);

    let material = if instance.debug_colors {
        materials.add(StandardMaterial {
            base_color: color_from_chunk_pos(result.position),
            perceptual_roughness: 0.9,
            ..default()
        })
    } else {
        default_material.0.clone()
    };

    let chunk_entity = commands
        .spawn((
            VoxelChunk {
                position: result.position,
                lod_level: 0,
            },
            Mesh3d(mesh_handle),
            MeshMaterial3d(material),
            Transform::from_translation(offset),
        ))
        .id();

    commands.entity(map_entity).add_child(chunk_entity);
}

fn chunk_world_offset(chunk_pos: IVec3) -> Vec3 {
    chunk_pos.as_vec3() * CHUNK_SIZE as f32 - Vec3::ONE
}

/// Despawn chunk entities whose position is no longer in the parent map's loaded_chunks.
pub fn despawn_out_of_range_chunks(
    mut commands: Commands,
    chunk_query: Query<(Entity, &VoxelChunk, &ChildOf)>,
    map_query: Query<&VoxelMapInstance>,
) {
    for (entity, chunk, child_of) in &chunk_query {
        debug_assert!(
            map_query.get(child_of.0).is_ok(),
            "VoxelChunk {:?} at {:?} is child of {:?} which has no VoxelMapInstance",
            entity,
            chunk.position,
            child_of.0
        );
        let Ok(instance) = map_query.get(child_of.0) else {
            warn!(
                "VoxelChunk entity {:?} has ChildOf pointing to non-map entity {:?}",
                entity, child_of.0
            );
            continue;
        };

        if !instance.loaded_chunks.contains(&chunk.position) {
            info!(
                "despawn_out_of_range_chunks: despawning chunk {:?} at {:?} (parent map {:?})",
                entity, chunk.position, child_of.0
            );
            commands.entity(entity).despawn();
        }
    }
}

/// Drains `chunks_needing_remesh` and spawns async mesh tasks from existing octree data.
pub fn spawn_remesh_tasks(mut map_query: Query<(&mut VoxelMapInstance, &mut PendingRemeshes)>) {
    let pool = AsyncComputeTaskPool::get();
    for (mut instance, mut pending) in &mut map_query {
        let positions: Vec<IVec3> = instance.chunks_needing_remesh.drain().collect();

        for chunk_pos in positions {
            let Some(chunk_data) = instance.get_chunk_data(chunk_pos) else {
                trace!("spawn_remesh_tasks: chunk {chunk_pos} no longer in octree, skipping");
                continue;
            };
            if chunk_data.fill_type == crate::types::FillType::Empty {
                trace!("spawn_remesh_tasks: chunk {chunk_pos} is empty, skipping remesh");
                continue;
            }
            let voxels = {
                let _span = info_span!("expand_palette").entered();
                chunk_data.voxels.to_voxels()
            };
            let task = pool.spawn(async move { mesh_chunk_greedy(&voxels) });
            pending.tasks.push(RemeshTask { chunk_pos, task });
        }
    }
}

/// Polls completed remesh tasks and swaps meshes on existing chunk entities.
pub fn poll_remesh_tasks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    default_material: Res<DefaultVoxelMaterial>,
    mut map_query: Query<(Entity, &VoxelMapInstance, &mut PendingRemeshes)>,
    chunk_query: Query<(Entity, &VoxelChunk, &ChildOf)>,
) {
    for (map_entity, instance, mut pending) in &mut map_query {
        let mut i = 0;
        while i < pending.tasks.len() {
            let Some(mesh_opt) = check_ready(&mut pending.tasks[i].task) else {
                i += 1;
                continue;
            };
            let remesh = pending.tasks.swap_remove(i);

            if !instance.loaded_chunks.contains(&remesh.chunk_pos) {
                continue;
            }

            let existing = chunk_query
                .iter()
                .find(|(_, vc, parent)| vc.position == remesh.chunk_pos && parent.0 == map_entity);

            match (mesh_opt, existing) {
                (Some(mesh), Some((entity, _, _))) => {
                    let handle = meshes.add(mesh);
                    commands.entity(entity).insert(Mesh3d(handle));
                }
                (Some(mesh), None) => {
                    let handle = meshes.add(mesh);
                    let offset = chunk_world_offset(remesh.chunk_pos);
                    let material = if instance.debug_colors {
                        materials.add(StandardMaterial {
                            base_color: color_from_chunk_pos(remesh.chunk_pos),
                            perceptual_roughness: 0.9,
                            ..default()
                        })
                    } else {
                        default_material.0.clone()
                    };
                    let chunk_entity = commands
                        .spawn((
                            VoxelChunk {
                                position: remesh.chunk_pos,
                                lod_level: 0,
                            },
                            Mesh3d(handle),
                            MeshMaterial3d(material),
                            Transform::from_translation(offset),
                        ))
                        .id();
                    commands.entity(map_entity).add_child(chunk_entity);
                }
                (None, Some((entity, _, _))) => {
                    commands.entity(entity).despawn();
                }
                (None, None) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_to_chunk_pos_positive() {
        let pos = world_to_chunk_pos(Vec3::new(20.0, 0.0, 5.0));
        assert_eq!(pos, IVec3::new(1, 0, 0));
    }

    #[test]
    fn world_to_chunk_pos_negative() {
        let pos = world_to_chunk_pos(Vec3::new(-1.0, -17.0, 0.0));
        assert_eq!(pos, IVec3::new(-1, -2, 0));
    }

    #[test]
    fn bounds_check() {
        assert!(is_within_bounds(IVec3::ZERO, Some(IVec3::new(5, 5, 5))));
        assert!(!is_within_bounds(
            IVec3::new(5, 0, 0),
            Some(IVec3::new(5, 5, 5))
        ));
        assert!(is_within_bounds(IVec3::new(100, 100, 100), None));
    }

    #[test]
    fn chunk_world_offset_calculation() {
        let offset = chunk_world_offset(IVec3::new(1, 2, 3));
        assert_eq!(offset, Vec3::new(15.0, 31.0, 47.0));
    }
}
