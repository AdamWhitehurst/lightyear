use bevy::prelude::*;
use voxel_map_engine::prelude::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, setup)
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.3, 0.6, 0.2),
        perceptual_roughness: 0.9,
        ..default()
    });

    let mesher = SurfaceNetsMesher;
    let map_entity = commands.spawn(VoxelMapInstance::new(4)).id();

    let chunk_size = CHUNK_SIZE as i32;

    for x in -8..=8 {
        for z in -8..=8 {
            for y in [-1, 0] {
                let chunk_pos = IVec3::new(x, y, z);
                let sdf = flat_terrain_sdf(chunk_pos);

                if let Some(mesh) = mesher.mesh_chunk(&sdf) {
                    let offset = Vec3::new(
                        (x * chunk_size) as f32,
                        (y * chunk_size) as f32,
                        (z * chunk_size) as f32,
                    );
                    let mesh_handle = meshes.add(mesh);
                    let child = commands
                        .spawn((
                            Mesh3d(mesh_handle),
                            MeshMaterial3d(material.clone()),
                            Transform::from_translation(offset),
                        ))
                        .id();
                    commands.entity(map_entity).add_child(child);
                }
            }
        }
    }

    commands.spawn((
        Camera3d::default(),
        Transform::from_translation(Vec3::new(30.0, 20.0, 30.0)).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 10000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.5, 0.5, 0.0)),
    ));
}
