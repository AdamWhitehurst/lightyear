use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use lightyear::prelude::{DisableRollback, Room, RoomEvent, RoomPlugin, RoomTarget};
use protocol::map::{MapInstanceId, PendingTransition};
use server::map::{RoomRegistry, TransitionUnfreezeTimer};
use voxel_map_engine::prelude::{VoxelMapInstance, WorldVoxel};

use std::sync::Arc;

fn dummy_generator() -> Arc<dyn Fn(IVec3) -> Vec<WorldVoxel> + Send + Sync> {
    Arc::new(|_| vec![WorldVoxel::Air; 1])
}

fn transition_test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(RoomPlugin);
    app.init_resource::<RoomRegistry>();
    app.add_systems(Update, server::map::tick_transition_unfreeze);
    app
}

#[test]
fn unfreeze_timer_removes_components_after_expiry() {
    let mut app = transition_test_app();

    let entity = app
        .world_mut()
        .spawn((
            avian3d::prelude::RigidBodyDisabled,
            DisableRollback,
            PendingTransition(MapInstanceId::Overworld),
            TransitionUnfreezeTimer(Timer::from_seconds(0.016, TimerMode::Once)),
        ))
        .id();

    // MinimalPlugins uses real time; sleep enough for timer to expire
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.update();
    }

    assert!(
        app.world()
            .get::<avian3d::prelude::RigidBodyDisabled>(entity)
            .is_none(),
        "RigidBodyDisabled should be removed after timer expires"
    );
    assert!(
        app.world().get::<DisableRollback>(entity).is_none(),
        "DisableRollback should be removed after timer expires"
    );
    assert!(
        app.world().get::<PendingTransition>(entity).is_none(),
        "PendingTransition should be removed after timer expires"
    );
    assert!(
        app.world().get::<TransitionUnfreezeTimer>(entity).is_none(),
        "TransitionUnfreezeTimer should be removed after timer expires"
    );
}

#[test]
fn unfreeze_timer_preserves_components_before_expiry() {
    let mut app = transition_test_app();

    let entity = app
        .world_mut()
        .spawn((
            avian3d::prelude::RigidBodyDisabled,
            DisableRollback,
            PendingTransition(MapInstanceId::Overworld),
            TransitionUnfreezeTimer(Timer::from_seconds(999.0, TimerMode::Once)),
        ))
        .id();

    for _ in 0..5 {
        app.update();
    }

    assert!(
        app.world().get::<PendingTransition>(entity).is_some(),
        "PendingTransition should remain before timer expires"
    );
    assert!(
        app.world()
            .get::<avian3d::prelude::RigidBodyDisabled>(entity)
            .is_some(),
        "RigidBodyDisabled should remain before timer expires"
    );
    assert!(
        app.world().get::<DisableRollback>(entity).is_some(),
        "DisableRollback should remain before timer expires"
    );
}

#[test]
fn room_registry_creates_separate_rooms_for_different_maps() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(RoomPlugin);
    app.init_resource::<RoomRegistry>();

    app.world_mut()
        .run_system_once(
            |mut registry: ResMut<RoomRegistry>, mut commands: Commands| {
                let ow = registry.get_or_create(&MapInstanceId::Overworld, &mut commands);
                let hb =
                    registry.get_or_create(&MapInstanceId::Homebase { owner: 42 }, &mut commands);
                assert_ne!(ow, hb, "Different maps should have different rooms");

                let ow2 = registry.get_or_create(&MapInstanceId::Overworld, &mut commands);
                assert_eq!(ow, ow2, "Same map should return same room");
            },
        )
        .unwrap();
}

#[test]
fn room_transfer_moves_entity_between_rooms() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(RoomPlugin);

    let room_a = app.world_mut().spawn(Room::default()).id();
    let room_b = app.world_mut().spawn(Room::default()).id();
    let entity = app.world_mut().spawn_empty().id();

    app.world_mut().trigger(RoomEvent {
        room: room_a,
        target: RoomTarget::AddEntity(entity),
    });
    app.update();

    assert!(
        app.world()
            .get::<Room>(room_a)
            .unwrap()
            .entities
            .contains(&entity),
        "Entity should be in room A initially"
    );

    // Same-frame transfer
    app.world_mut().trigger(RoomEvent {
        room: room_a,
        target: RoomTarget::RemoveEntity(entity),
    });
    app.world_mut().trigger(RoomEvent {
        room: room_b,
        target: RoomTarget::AddEntity(entity),
    });
    app.update();

    assert!(
        !app.world()
            .get::<Room>(room_a)
            .unwrap()
            .entities
            .contains(&entity),
        "Entity should leave old room"
    );
    assert!(
        app.world()
            .get::<Room>(room_b)
            .unwrap()
            .entities
            .contains(&entity),
        "Entity should be in new room"
    );
}

#[test]
fn pending_transition_marker_can_be_added_and_removed() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);

    let entity = app
        .world_mut()
        .spawn(PendingTransition(MapInstanceId::Overworld))
        .id();
    app.update();
    assert!(app.world().get::<PendingTransition>(entity).is_some());

    app.world_mut()
        .entity_mut(entity)
        .remove::<PendingTransition>();
    app.update();
    assert!(app.world().get::<PendingTransition>(entity).is_none());
}

#[test]
fn different_homebase_owners_produce_different_seeds() {
    let bounds = IVec3::new(4, 4, 4);
    let (_, config_a, _) = VoxelMapInstance::homebase(111, bounds, dummy_generator());
    let (_, config_b, _) = VoxelMapInstance::homebase(222, bounds, dummy_generator());
    assert_ne!(
        config_a.seed, config_b.seed,
        "Different owners must produce different seeds"
    );
}
