use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext};
use bevy::prelude::*;
use bevy::reflect::{TypePath, TypeRegistryArc};

use super::types::{WorldObjectDef, WorldObjectLoadError};
use crate::reflect_loader;

/// Custom asset loader that uses `TypeRegistry` for reflect-based component deserialization.
#[derive(TypePath)]
pub(super) struct WorldObjectLoader {
    type_registry: TypeRegistryArc,
}

impl FromWorld for WorldObjectLoader {
    fn from_world(world: &mut World) -> Self {
        Self {
            type_registry: world.resource::<AppTypeRegistry>().0.clone(),
        }
    }
}

impl AssetLoader for WorldObjectLoader {
    type Asset = WorldObjectDef;
    type Settings = ();
    type Error = WorldObjectLoadError;

    fn extensions(&self) -> &[&str] {
        &["object.ron"]
    }

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let registry = self.type_registry.read();
        deserialize_world_object(&bytes, &registry)
    }
}

/// Deserializes a `WorldObjectDef` from RON bytes using the given `TypeRegistry`.
///
/// The RON format is a flat map of type paths to component data:
/// ```ron
/// {
///     "protocol::world_object::ObjectCategory": Scenery,
///     "protocol::world_object::VisualKind": Vox("models/trees/tree.vox"),
///     "protocol::Health": (current: 50.0, max: 50.0),
/// }
/// ```
pub fn deserialize_world_object(
    bytes: &[u8],
    registry: &bevy::reflect::TypeRegistry,
) -> Result<WorldObjectDef, WorldObjectLoadError> {
    let components = reflect_loader::deserialize_component_map(bytes, registry)?;
    Ok(WorldObjectDef { components })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_object::types::{ObjectCategory, VisualKind};
    use crate::Health;
    use bevy::reflect::TypeRegistry;

    fn test_registry() -> TypeRegistry {
        let mut registry = TypeRegistry::default();
        registry.register::<Health>();
        registry.register::<ObjectCategory>();
        registry.register::<VisualKind>();
        registry
    }

    #[test]
    fn deserialize_valid_world_object() {
        let registry = test_registry();
        let ron = br#"{
            "protocol::world_object::types::ObjectCategory": Scenery,
            "protocol::world_object::types::VisualKind": Vox("models/trees/tree_circle.vox"),
            "protocol::Health": (current: 50.0, max: 50.0),
        }"#;
        let def = deserialize_world_object(ron, &registry).unwrap();
        assert_eq!(def.components.len(), 3);
    }

    #[test]
    fn deserialize_empty_components() {
        let registry = test_registry();
        let ron = br#"{}"#;
        let def = deserialize_world_object(ron, &registry).unwrap();
        assert!(def.components.is_empty());
    }

    #[test]
    fn deserialize_unregistered_type_errors() {
        let registry = TypeRegistry::default();
        let ron = br#"{
            "protocol::Health": (current: 1.0, max: 1.0),
        }"#;
        assert!(deserialize_world_object(ron, &registry).is_err());
    }

    #[test]
    fn deserialize_malformed_ron_errors() {
        let registry = test_registry();
        assert!(deserialize_world_object(b"not valid ron {{{", &registry).is_err());
    }
}
