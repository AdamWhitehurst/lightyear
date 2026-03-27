use std::collections::HashMap;

use avian3d::prelude::LinearVelocity;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use lightyear::prelude::*;
use protocol::billboard::sprite_rig_material::{SpriteRigBillboardExt, SpriteRigMaterial};
use protocol::{CharacterMarker, CharacterType};

use crate::asset::SpriteRigAsset;
use crate::{asset::SpriteAnimSetAsset, RigRegistry};

/// Reference to the rig asset for this character. Triggers rig spawning.
#[derive(Component)]
pub struct SpriteRig(pub Handle<SpriteRigAsset>);

/// Reference to the animset for this character.
#[derive(Component)]
pub struct AnimSetRef(pub Handle<SpriteAnimSetAsset>);

/// Maps bone names to their spawned joint entities.
#[derive(Component, Default)]
pub struct BoneEntities(pub HashMap<String, Entity>);

/// Horizontal facing direction derived from velocity.
#[derive(Component, PartialEq, Clone, Copy)]
pub enum Facing {
    Left,
    Right,
}

/// Marker for the joint root child entity that parents all root bone entities.
#[derive(Component)]
pub struct JointRoot;

/// Cached GPU assets for a rig type, shared across all characters using that rig.
struct RigMeshAssets {
    mesh: Handle<Mesh>,
    inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
    material: Handle<SpriteRigMaterial>,
}

/// Cache of rig mesh assets keyed by rig asset handle ID.
#[derive(Resource, Default)]
pub struct RigMeshCache(HashMap<AssetId<SpriteRigAsset>, RigMeshAssets>);

/// Looks up `RigRegistry` when `CharacterType` is added and inserts rig components.
pub fn resolve_character_rig(
    mut commands: Commands,
    query: Query<
        (Entity, &CharacterType),
        (
            Added<CharacterType>,
            Without<SpriteRig>,
            Or<(With<Predicted>, With<Replicated>, With<Interpolated>)>,
            With<CharacterMarker>,
        ),
    >,
    registry: Res<RigRegistry>,
) {
    for (entity, char_type) in &query {
        let entry = registry
            .entries
            .get(char_type)
            .expect("RigRegistry missing entry for CharacterType");
        commands.entity(entity).insert((
            SpriteRig(entry.rig_handle.clone()),
            AnimSetRef(entry.animset_handle.clone()),
            Facing::Right,
        ));
    }
}

/// Spawns joint hierarchy and a single skinned mesh when `SpriteRig` is added.
pub fn spawn_sprite_rigs(
    mut commands: Commands,
    query: Query<(Entity, &SpriteRig), Added<SpriteRig>>,
    rig_assets: Res<Assets<SpriteRigAsset>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SpriteRigMaterial>>,
    mut bindpose_assets: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut rig_mesh_cache: ResMut<RigMeshCache>,
) {
    for (entity, sprite_rig) in &query {
        let rig = rig_assets
            .get(&sprite_rig.0)
            .expect("SpriteRigAsset must be loaded before gameplay (AppState::Ready)");

        let slot_lookup = build_slot_lookup(rig);
        let sorted_bones = topological_sort_bones(&rig.bones);

        let cached = rig_mesh_cache
            .0
            .entry(sprite_rig.0.id())
            .or_insert_with(|| {
                build_rig_mesh_assets(
                    &sorted_bones,
                    &slot_lookup,
                    &mut meshes,
                    &mut materials,
                    &mut bindpose_assets,
                )
            });

        let mesh_handle = cached.mesh.clone();
        let bindpose_handle = cached.inverse_bindposes.clone();
        let material_handle = cached.material.clone();

        let joint_root_id = commands
            .spawn((JointRoot, Name::new("JointRoot"), Transform::default()))
            .id();
        commands.entity(entity).add_child(joint_root_id);

        let (bone_map, joint_entities) =
            spawn_joints(&mut commands, joint_root_id, &sorted_bones, &slot_lookup);

        commands.entity(entity).with_child((
            Name::new("SkinnedMesh"),
            Mesh3d(mesh_handle),
            MeshMaterial3d(material_handle),
            SkinnedMesh {
                inverse_bindposes: bindpose_handle,
                joints: joint_entities,
            },
            Transform::default(),
        ));

        commands.entity(entity).insert(BoneEntities(bone_map));
    }
}

/// Builds and caches the mesh, inverse bind poses, and material for a rig type.
fn build_rig_mesh_assets(
    sorted_bones: &[&crate::asset::BoneDef],
    slot_lookup: &HashMap<&str, (f32, Vec2)>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<SpriteRigMaterial>,
    bindpose_assets: &mut Assets<SkinnedMeshInverseBindposes>,
) -> RigMeshAssets {
    let bone_index_map = build_bone_index_map(sorted_bones);
    let mesh = build_rig_mesh(sorted_bones, slot_lookup, &bone_index_map);
    let inverse_bindposes = bindpose_assets.add(vec![Mat4::IDENTITY; sorted_bones.len()]);
    let material = materials.add(SpriteRigMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            double_sided: true,
            cull_mode: None,
            ..default()
        },
        extension: SpriteRigBillboardExt {},
    });

    RigMeshAssets {
        mesh: meshes.add(mesh),
        inverse_bindposes,
        material,
    }
}

/// Maps bone name to its index in topological sort order.
fn build_bone_index_map<'a>(sorted_bones: &[&'a crate::asset::BoneDef]) -> HashMap<&'a str, u16> {
    sorted_bones
        .iter()
        .enumerate()
        .map(|(i, bone)| {
            let idx = u16::try_from(i).expect("bone count exceeds u16::MAX");
            (bone.name.as_str(), idx)
        })
        .collect()
}

/// Builds a single merged mesh with one quad per visible bone, skinned to its joint.
fn build_rig_mesh(
    sorted_bones: &[&crate::asset::BoneDef],
    slot_lookup: &HashMap<&str, (f32, Vec2)>,
    bone_index_map: &HashMap<&str, u16>,
) -> Mesh {
    let mut visible_bones = collect_visible_bones(sorted_bones, slot_lookup);
    visible_bones.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut joint_indices: Vec<[u16; 4]> = Vec::new();
    let mut joint_weights: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for (bone_def, _z_order, size) in &visible_bones {
        let joint_idx = *bone_index_map
            .get(bone_def.name.as_str())
            .expect("visible bone must exist in bone_index_map");
        let base_vertex = u32::try_from(positions.len()).expect("vertex count exceeds u32::MAX");

        append_quad_vertices(
            *size,
            joint_idx,
            &mut positions,
            &mut normals,
            &mut uvs,
            &mut joint_indices,
            &mut joint_weights,
        );
        append_quad_indices(base_vertex, &mut indices);
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_JOINT_INDEX,
        VertexAttributeValues::Uint16x4(joint_indices),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, joint_weights)
    .with_inserted_indices(Indices::U32(indices))
}

/// Collects bones that have a slot entry, returning (bone_def, z_order, size).
fn collect_visible_bones<'a>(
    sorted_bones: &[&'a crate::asset::BoneDef],
    slot_lookup: &HashMap<&str, (f32, Vec2)>,
) -> Vec<(&'a crate::asset::BoneDef, f32, Vec2)> {
    sorted_bones
        .iter()
        .filter_map(|bone| {
            slot_lookup
                .get(bone.name.as_str())
                .map(|&(z, size)| (*bone, z, size))
        })
        .collect()
}

/// Appends 4 vertices for a Z-facing quad centered at origin with the given half-extents.
fn append_quad_vertices(
    size: Vec2,
    joint_idx: u16,
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    joint_indices: &mut Vec<[u16; 4]>,
    joint_weights: &mut Vec<[f32; 4]>,
) {
    let hx = size.x / 2.0;
    let hy = size.y / 2.0;

    // Bottom-left, bottom-right, top-right, top-left
    positions.extend_from_slice(&[
        [-hx, -hy, 0.0],
        [hx, -hy, 0.0],
        [hx, hy, 0.0],
        [-hx, hy, 0.0],
    ]);
    normals.extend_from_slice(&[[0.0, 0.0, 1.0]; 4]);
    uvs.extend_from_slice(&[[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);

    let ji = [joint_idx, 0, 0, 0];
    let jw = [1.0, 0.0, 0.0, 0.0];
    joint_indices.extend_from_slice(&[ji; 4]);
    joint_weights.extend_from_slice(&[jw; 4]);
}

/// Appends 6 indices (2 triangles) for a quad starting at `base_vertex`.
fn append_quad_indices(base_vertex: u32, indices: &mut Vec<u32>) {
    indices.extend_from_slice(&[
        base_vertex,
        base_vertex + 1,
        base_vertex + 2,
        base_vertex,
        base_vertex + 2,
        base_vertex + 3,
    ]);
}

/// Spawns transform-only joint entities in topological order, returning the bone map and
/// ordered joint entity list matching the inverse bind pose array.
fn spawn_joints(
    commands: &mut Commands,
    joint_root_id: Entity,
    sorted_bones: &[&crate::asset::BoneDef],
    slot_lookup: &HashMap<&str, (f32, Vec2)>,
) -> (HashMap<String, Entity>, Vec<Entity>) {
    let mut bone_map = HashMap::<String, Entity>::new();
    let mut joint_entities = Vec::with_capacity(sorted_bones.len());

    for bone_def in sorted_bones {
        let slot_info = slot_lookup.get(bone_def.name.as_str());
        let transform = bone_transform_from_def(bone_def, slot_info);

        let joint_entity = commands
            .spawn((Name::new(bone_def.name.clone()), transform))
            .id();

        let parent_entity = bone_def
            .parent
            .as_ref()
            .map(|parent_name| {
                *bone_map
                    .get(parent_name.as_str())
                    .expect("parent bone must be spawned before child (topological sort)")
            })
            .unwrap_or(joint_root_id);

        commands.entity(parent_entity).add_child(joint_entity);
        bone_map.insert(bone_def.name.clone(), joint_entity);
        joint_entities.push(joint_entity);
    }

    (bone_map, joint_entities)
}

/// Builds a `Transform` from a bone definition, using z_order from slot info.
fn bone_transform_from_def(
    bone_def: &crate::asset::BoneDef,
    slot_info: Option<&(f32, Vec2)>,
) -> Transform {
    let z_order = slot_info.map(|(z, _)| *z).unwrap_or(0.0);

    Transform {
        translation: Vec3::new(
            bone_def.default_transform.translation.x,
            bone_def.default_transform.translation.y,
            z_order,
        ),
        rotation: Quat::from_rotation_z(bone_def.default_transform.rotation.to_radians()),
        scale: Vec3::new(
            bone_def.default_transform.scale.x,
            bone_def.default_transform.scale.y,
            1.0,
        ),
    }
}

/// Builds a lookup from bone name to (z_order, attachment size) from the rig's slots and default skin.
fn build_slot_lookup(rig: &SpriteRigAsset) -> HashMap<&str, (f32, Vec2)> {
    let default_skin = rig.skins.get("default");
    let mut lookup = HashMap::new();

    for slot in &rig.slots {
        let size = default_skin
            .and_then(|skin| skin.get(&slot.default_attachment))
            .map(|att| att.size)
            .unwrap_or(Vec2::splat(1.0));

        lookup.insert(slot.bone.as_str(), (slot.z_order, size));
    }

    lookup
}

/// Topological sort: parents before children. Simple iterative approach.
fn topological_sort_bones(bones: &[crate::asset::BoneDef]) -> Vec<&crate::asset::BoneDef> {
    let mut sorted: Vec<&crate::asset::BoneDef> = Vec::with_capacity(bones.len());
    let mut added: std::collections::HashSet<&str> = std::collections::HashSet::new();

    loop {
        let prev_len = sorted.len();
        for bone in bones {
            if added.contains(bone.name.as_str()) {
                continue;
            }
            let parent_satisfied = bone
                .parent
                .as_ref()
                .map_or(true, |p| added.contains(p.as_str()));
            if parent_satisfied {
                sorted.push(bone);
                added.insert(&bone.name);
            }
        }
        if sorted.len() == bones.len() {
            break;
        }
        debug_assert!(
            sorted.len() > prev_len,
            "Bone hierarchy contains a cycle or references a missing parent"
        );
    }

    sorted
}

/// Updates `Facing` based on horizontal velocity projected onto camera's right axis.
pub fn update_facing_from_velocity(
    camera_query: Query<&Transform, With<Camera3d>>,
    mut query: Query<(&mut Facing, &LinearVelocity), With<CharacterMarker>>,
) {
    let Ok(camera_transform) = camera_query.single() else {
        return;
    };
    let camera_right = camera_transform.right().as_vec3();
    let camera_right_xz = Vec3::new(camera_right.x, 0.0, camera_right.z);

    for (mut facing, velocity) in &mut query {
        let lateral = velocity.dot(camera_right_xz);
        if lateral > 0.1 {
            facing.set_if_neq(Facing::Left);
        } else if lateral < -0.1 {
            facing.set_if_neq(Facing::Right);
        }
    }
}

/// Rotates `JointRoot` entities to face the camera (Y-axis locked).
///
/// This keeps the 2D bone hierarchy layout in the camera-facing plane so bone
/// offsets (arm to the side, head above, etc.) project correctly on screen.
/// The GPU billboard shader handles per-quad screen-plane orientation on top.
pub fn billboard_joint_roots(
    camera_query: Query<&GlobalTransform, With<Camera3d>>,
    mut joint_query: Query<(&GlobalTransform, &mut Transform, &ChildOf), With<JointRoot>>,
    parent_query: Query<&GlobalTransform, Without<JointRoot>>,
) {
    let Ok(camera_gt) = camera_query.single() else {
        trace!("Camera not yet spawned during early frames");
        return;
    };
    let camera_pos = camera_gt.translation();

    for (global_transform, mut transform, child_of) in &mut joint_query {
        let pos = global_transform.translation();
        let direction = (camera_pos - pos).with_y(0.0);
        if direction.length_squared() < 0.001 {
            continue;
        }
        let world_rotation = Quat::from_rotation_y(direction.x.atan2(direction.z));
        let parent_rotation = parent_query
            .get(child_of.parent())
            .map(|gt| gt.to_scale_rotation_translation().1)
            .unwrap_or(Quat::IDENTITY);
        transform.rotation = parent_rotation.inverse() * world_rotation;
    }
}

/// Mirrors the joint root horizontally when `Facing` changes.
pub fn apply_facing_to_rig(
    changed_query: Query<(Entity, &Facing), Changed<Facing>>,
    children_query: Query<&Children>,
    mut billboard_query: Query<&mut Transform, With<JointRoot>>,
) {
    for (entity, facing) in &changed_query {
        let Ok(children) = children_query.get(entity) else {
            continue; // children not yet propagated on the frame Facing is first inserted
        };
        for child in children.iter() {
            if let Ok(mut transform) = billboard_query.get_mut(child) {
                transform.scale.x = match facing {
                    Facing::Right => 1.0,
                    Facing::Left => -1.0,
                };
            }
        }
    }
}
