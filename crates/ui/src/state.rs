use bevy::prelude::*;

/// Client application state
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, States)]
pub enum ClientState {
    /// Main menu - not connected to server
    #[default]
    MainMenu,
    /// Connecting to server - loading screen
    Connecting,
    /// Connected and in-game
    InGame,
}

/// Sub-state for map transition flow while in-game
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, SubStates)]
#[source(ClientState = ClientState::InGame)]
pub enum MapTransitionState {
    #[default]
    Playing,
    Transitioning,
}
