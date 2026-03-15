# Archetype-from-Asset: Component Bundles in RON for Bevy

A system where game entities (abilities, items, enemies, etc.) are defined as composable sets of reflected components in RON files, loaded through Bevy's asset system, and spawned at runtime using reflection.

---

## 1. Define Small, Orthogonal Components

Each component represents a single mechanic or property. All must derive `Reflect` and be serialization-friendly.

```rust
// components.rs
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Component, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct Damage {
    pub value: f32,
}

#[derive(Component, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct Cooldown {
    pub duration: f32,
    #[serde(default)]
    pub remaining: f32,
}

#[derive(Component, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct Projectile {
    pub speed: f32,
    #[serde(default)]
    pub gravity: f32,
}

#[derive(Component, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct AreaOfEffect {
    pub radius: f32,
    pub falloff: Falloff,
}

#[derive(Reflect, Serialize, Deserialize, Default)]
pub enum Falloff {
    #[default]
    None,
    Linear,
    Quadratic,
}

#[derive(Component, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct DamageOverTime {
    pub tick_damage: f32,
    pub duration: f32,
}

/// References another archetype asset by path
#[derive(Component, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct CastsOnHit(pub String);

/// A display name for the entity
#[derive(Component, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct DisplayName(pub String);
```

### Key points

- `#[reflect(Component, Serialize, Deserialize)]` registers the reflect trait data so `ReflectComponent` and `ReflectDeserialize` are available at runtime.
- `Default` is needed for `FromReflect` derivation.
- Keep components small and single-purpose — the RON files compose them.

---

## 2. Register All Types

Every component and supporting type must be registered in the `AppTypeRegistry`.

```rust
// plugin.rs
pub struct ArchetypePlugin;

impl Plugin for ArchetypePlugin {
    fn build(&self, app: &mut App) {
        app
            // Register all component types
            .register_type::<Damage>()
            .register_type::<Cooldown>()
            .register_type::<Projectile>()
            .register_type::<AreaOfEffect>()
            .register_type::<Falloff>()
            .register_type::<DamageOverTime>()
            .register_type::<CastsOnHit>()
            .register_type::<DisplayName>()
            // Register the asset + loader
            .init_asset::<ArchetypeAsset>()
            .init_asset_loader::<ArchetypeAssetLoader>()
            // Systems
            .add_systems(Update, spawn_archetypes);
    }
}
```

> **If a type is not registered, deserialization will fail at runtime with "Unknown type".**
> For a multi-crate workspace, consider a shared `register_all_types(app)` function in your `protocol` crate.

---

## 3. Define the Asset

```rust
// asset.rs
use bevy::prelude::*;
use bevy::reflect::PartialReflect;

#[derive(Asset, TypePath)]
pub struct ArchetypeAsset {
    /// Each entry is a reflected component, ready to be inserted onto an entity.
    pub components: Vec<Box<dyn PartialReflect>>,
}
```

> In newer Bevy versions (0.15+), `ReflectDeserializer` returns `Box<dyn PartialReflect>`.
> In older versions it returns `Box<dyn Reflect>`. Adjust accordingly.

---

## 4. Implement the Asset Loader

This is the core of the system. The loader reads a RON map of `"TypeName": (fields...)` and uses `TypedReflectDeserializer` to deserialize each value with full type awareness.

### 4a. The Loader struct

```rust
// loader.rs
use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext};
use bevy::prelude::*;
use bevy::reflect::serde::TypedReflectDeserializer;
use bevy::reflect::TypeRegistry;
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};
use std::sync::{Arc, RwLock};

#[derive(Default)]
pub struct ArchetypeAssetLoader;

impl AssetLoader for ArchetypeAssetLoader {
    type Asset = ArchetypeAsset;
    type Settings = ();
    type Error = Box<dyn std::error::Error + Send + Sync>;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        // IMPORTANT: How you access the type registry depends on your Bevy version.
        // Option A (if LoadContext provides world access):
        //   let registry = load_context.world().resource::<AppTypeRegistry>();
        //
        // Option B (store Arc<RwLock<TypeRegistry>> on the loader — see section 4b):
        //   let registry = self.registry.read().unwrap();

        // For this outline, we assume access to the registry somehow:
        let registry = get_type_registry(load_context)?;
        let registry_guard = registry.read().unwrap();

        let mut deserializer = ron::de::Deserializer::from_bytes(&bytes)?;
        let components = deserializer.deserialize_map(ArchetypeVisitor {
            registry: &registry_guard,
        })?;

        Ok(ArchetypeAsset { components })
    }

    fn extensions(&self) -> &[&str] {
        &["archetype.ron"]
    }
}
```

### 4b. Accessing the TypeRegistry in the Loader

The `AppTypeRegistry` lives in the Bevy `World`, but `AssetLoader::load` is async and doesn't always have direct world access. Common workarounds:

**Option A — Clone the registry Arc at startup:**

```rust
pub struct ArchetypeAssetLoader {
    registry: Arc<RwLock<TypeRegistry>>,
}

// In plugin build:
fn build(&self, app: &mut App) {
    // Register types FIRST, then clone the Arc
    app.register_type::<Damage>()
       // ... all types ...
       ;

    let registry = app
        .world()
        .resource::<AppTypeRegistry>()
        .0
        .clone(); // Arc clone — cheap

    app.init_asset::<ArchetypeAsset>()
       .register_asset_loader(ArchetypeAssetLoader { registry });
}
```

**Option B — Use `FromWorld` to initialize the loader:**

```rust
impl FromWorld for ArchetypeAssetLoader {
    fn from_world(world: &mut World) -> Self {
        let registry = world.resource::<AppTypeRegistry>().0.clone();
        Self { registry }
    }
}
```

Then use `app.init_asset_loader::<ArchetypeAssetLoader>()` which calls `FromWorld`.

> **Gotcha:** Make sure all types are registered *before* the loader is initialized.
> Order your plugin's `build()` accordingly.

### 4c. The Visitor (type-aware deserialization)

```rust
struct ArchetypeVisitor<'a> {
    registry: &'a TypeRegistry,
}

impl<'a, 'de> Visitor<'de> for ArchetypeVisitor<'a> {
    type Value = Vec<Box<dyn PartialReflect>>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "a map of component type names to component data")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut components = Vec::new();

        while let Some(type_name) = map.next_key::<String>()? {
            // Resolve the type by short name first, then full path
            let registration = self
                .registry
                .get_with_short_type_path(&type_name)
                .or_else(|| self.registry.get_with_type_path(&type_name))
                .ok_or_else(|| {
                    de::Error::custom(format!(
                        "Type '{}' not found in registry. Did you call register_type?",
                        type_name
                    ))
                })?;

            // TypedReflectDeserializer knows the concrete type and drives
            // the RON deserializer with correct struct/enum expectations.
            let seed = TypedReflectDeserializer::new(registration, self.registry);
            let component = map.next_value_seed(seed)?;

            components.push(component);
        }

        Ok(components)
    }
}
```

### Why this works

`TypedReflectDeserializer` implements `DeserializeSeed`. When serde asks "what is the next value?", the seed says "it's a struct with fields `duration: f32`" — so RON correctly parses `(duration: 1.5)` as a named struct rather than trying to interpret it as a generic `ron::Value`. Enums, tuple structs, Options, Vecs — they all work because the deserializer has the full type information.

---

## 5. RON File Format

```ron
// assets/abilities/fireball.archetype.ron
{
    "DisplayName": ("Fireball"),
    "Damage": (value: 40.0),
    "Cooldown": (duration: 1.5),
    "Projectile": (speed: 20.0, gravity: -9.8),
    "AreaOfEffect": (radius: 3.0, falloff: Linear),
}
```

```ron
// assets/abilities/poison_dart.archetype.ron
{
    "DisplayName": ("Poison Dart"),
    "Damage": (value: 10.0),
    "Cooldown": (duration: 0.8),
    "Projectile": (speed: 30.0),
    "CastsOnHit": ("abilities/poison_dot.archetype.ron"),
}
```

```ron
// assets/abilities/poison_dot.archetype.ron
{
    "DisplayName": ("Poison"),
    "DamageOverTime": (tick_damage: 5.0, duration: 6.0),
}
```

### Naming conventions

- Use the **short type name** (e.g. `"Damage"`) when unambiguous.
- Use the **full type path** (e.g. `"my_game::combat::Damage"`) if there are name collisions.
- File extension `.archetype.ron` maps to the loader's `extensions()`.

---

## 6. Spawning Entities from the Asset

### 6a. A marker component to trigger spawning

```rust
#[derive(Component)]
pub struct SpawnArchetype {
    pub handle: Handle<ArchetypeAsset>,
    pub spawned: bool,
}
```

### 6b. The spawn system

```rust
fn spawn_archetypes(
    mut commands: Commands,
    query: Query<(Entity, &SpawnArchetype), Changed<SpawnArchetype>>,
    assets: Res<Assets<ArchetypeAsset>>,
    registry: Res<AppTypeRegistry>,
) {
    let registry = registry.read();

    for (entity, spawn) in &query {
        if spawn.spawned {
            continue;
        }

        let Some(archetype) = assets.get(&spawn.handle) else {
            continue; // Not loaded yet — will re-trigger on change
        };

        for reflected_component in &archetype.components {
            // Get the TypeId of this reflected value
            let type_info = reflected_component
                .get_represented_type_info()
                .expect("Component missing type info");
            let type_id = type_info.type_id();

            // Look up the registration to get ReflectComponent
            let registration = registry
                .get(type_id)
                .expect("Type not registered");
            let reflect_component = registration
                .data::<ReflectComponent>()
                .expect("Type missing #[reflect(Component)]");

            // Apply the component to the entity
            reflect_component.apply_or_insert(
                &mut commands.entity(entity),
                reflected_component.as_ref(),
                &registry,
            );
        }

        // Mark as spawned (or remove the SpawnArchetype component)
        commands.entity(entity).remove::<SpawnArchetype>();
    }
}
```

> **Note:** The exact API for `ReflectComponent::apply_or_insert` / `insert` varies by Bevy version.
> In some versions you use `reflect_component.insert(&mut world, entity, &**reflected)` with direct world access.
> Check your version's docs. An exclusive system (`fn(world: &mut World)`) may be needed for direct world mutation.

### 6c. Usage

```rust
// Spawn a fireball ability entity
fn create_fireball(mut commands: Commands, asset_server: Res<AssetServer>) {
    let handle = asset_server.load("abilities/fireball.archetype.ron");
    commands.spawn(SpawnArchetype {
        handle,
        spawned: false,
    });
}
```

---

## 7. Resolving Cross-References

When one archetype references another (e.g. `CastsOnHit("abilities/poison_dot.archetype.ron")`), the referencing is by asset path string. Resolution happens in gameplay systems:

```rust
fn resolve_casts_on_hit(
    query: Query<&CastsOnHit>,
    asset_server: Res<AssetServer>,
    // ... hit events, etc.
) {
    for casts in &query {
        // When a hit occurs, load and spawn the referenced archetype
        let handle = asset_server.load::<ArchetypeAsset>(&casts.0);
        // Spawn a new entity with SpawnArchetype { handle, ... }
    }
}
```

### Optional: preload references in the asset loader

To make Bevy track dependencies (and preload referenced assets):

```rust
// In the asset loader, after deserializing components:
for component in &archetype.components {
    // Check if it's a CastsOnHit or other reference type
    // and call load_context.load(path) to register the dependency
}
```

### Optional: an AbilityRegistry resource

For complex graphs (combo chains, talent trees):

```rust
#[derive(Resource, Default)]
pub struct AbilityRegistry {
    pub abilities: HashMap<String, Handle<ArchetypeAsset>>,
}

// A startup system that loads all .archetype.ron files from a directory
fn load_all_abilities(
    mut registry: ResMut<AbilityRegistry>,
    asset_server: Res<AssetServer>,
) {
    // Load all ability files (you'd enumerate the directory or use a manifest)
    for path in &["abilities/fireball", "abilities/poison_dart", "abilities/poison_dot"] {
        let handle = asset_server.load(format!("{}.archetype.ron", path));
        registry.abilities.insert(path.to_string(), handle);
    }
}
```

---

## 8. Hot Reloading

Bevy's asset system gives you hot reloading almost for free:

```rust
fn hot_reload_archetypes(
    mut events: EventReader<AssetEvent<ArchetypeAsset>>,
    // Re-apply components when the asset changes
) {
    for event in events.read() {
        if let AssetEvent::Modified { id } = event {
            // Find all entities using this asset and re-apply components
        }
    }
}
```

This means you can edit a RON file, save, and see changes in-game without restarting.

---

## 9. Project Structure

Suggested layout for a multi-crate workspace:

```
bevy-lightyear-template/
├── crates/
│   ├── protocol/
│   │   ├── src/
│   │   │   ├── components.rs      # All reflected components
│   │   │   ├── archetype/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── asset.rs       # ArchetypeAsset definition
│   │   │   │   ├── loader.rs      # ArchetypeAssetLoader + Visitor
│   │   │   │   ├── spawn.rs       # Spawn system
│   │   │   │   └── registry.rs    # Optional AbilityRegistry
│   │   │   └── lib.rs
│   ├── client/
│   ├── server/
│   └── ...
└── assets/
    └── abilities/
        ├── fireball.archetype.ron
        ├── poison_dart.archetype.ron
        └── poison_dot.archetype.ron
```

The `protocol` crate owns component definitions and type registration so both client and server share the same types.

---

## 10. Checklist

- [ ] All components derive `Component`, `Reflect`, `Serialize`, `Deserialize`, `Default`
- [ ] All components have `#[reflect(Component, Serialize, Deserialize)]`
- [ ] All types (including enums, newtypes) are registered via `register_type`
- [ ] Types are registered *before* the asset loader is initialized
- [ ] Asset loader has access to `TypeRegistry` (via `Arc` clone or `FromWorld`)
- [ ] Loader uses `TypedReflectDeserializer` (not `ron::Value`)
- [ ] Spawn system uses `ReflectComponent` to insert components onto entities
- [ ] RON files use type names matching what's in the registry (short or full path)
- [ ] Cross-references between archetypes use asset path strings
