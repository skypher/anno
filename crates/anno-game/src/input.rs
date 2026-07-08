//! Input binding specifications for the game binary.
//!
//! The SDL loop still performs the side effects directly, but these
//! bindings give tests a stable place to catch ordering mistakes such as
//! a plain key arm shadowing its modifier variant.

use sdl2::keyboard::Keycode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputContext {
    pub shift_held: bool,
    pub ctrl_held: bool,
}

impl InputContext {
    pub const fn normal() -> Self {
        Self {
            shift_held: false,
            ctrl_held: false,
        }
    }

    pub const fn shift() -> Self {
        Self {
            shift_held: true,
            ctrl_held: false,
        }
    }

    pub const fn ctrl() -> Self {
        Self {
            shift_held: false,
            ctrl_held: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingGuard {
    Any,
    Shift,
    Ctrl,
}

impl BindingGuard {
    pub fn matches(self, ctx: InputContext) -> bool {
        match self {
            Self::Any => true,
            Self::Shift => ctx.shift_held,
            Self::Ctrl => ctx.ctrl_held,
        }
    }

    pub const fn is_specific(self) -> bool {
        !matches!(self, Self::Any)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    ScrollLeft,
    ScrollRight,
    ScrollUp,
    ScrollDown,
    NextIsland,
    WhiteFlagSurrender,
    PauseGame,
    JumpToActiveObject,
    SpeedNormal,
    SpeedDouble,
    SpeedQuad,
    ToggleBuild,
    ToggleDiplomacyPanel,
    ToggleInfoMode,
    ToggleCombatMode,
    ToggleShipPanel,
    CommitRouteOrChat,
    CycleOwnWarehouse,
    ToggleCitiesPanel,
    ToggleSavePanel,
    ToggleVideoSpeechPanel,
    ToggleOptionsPanel,
    ZoomBirdEye,
    ZoomNormal,
    ZoomDetailed,
    RotateCounterClockwise,
    RotateClockwise,
    StoreTroopAssembly,
    RecallTroopAssembly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBinding {
    pub key: Keycode,
    pub guard: BindingGuard,
    pub action: InputAction,
}

impl KeyBinding {
    pub const fn new(key: Keycode, guard: BindingGuard, action: InputAction) -> Self {
        Self { key, guard, action }
    }
}

use BindingGuard::{Any, Ctrl};
use InputAction::*;

pub const NORMAL_MODE_BINDINGS: &[KeyBinding] = &[
    KeyBinding::new(Keycode::Left, Any, ScrollLeft),
    KeyBinding::new(Keycode::Right, Any, ScrollRight),
    KeyBinding::new(Keycode::Up, Any, ScrollUp),
    KeyBinding::new(Keycode::Down, Any, ScrollDown),
    KeyBinding::new(Keycode::Tab, Any, NextIsland),
    KeyBinding::new(Keycode::W, Any, WhiteFlagSurrender),
    KeyBinding::new(Keycode::Pause, Any, PauseGame),
    KeyBinding::new(Keycode::J, Any, JumpToActiveObject),
    KeyBinding::new(Keycode::F2, Any, ZoomBirdEye),
    KeyBinding::new(Keycode::F3, Any, ZoomNormal),
    KeyBinding::new(Keycode::F4, Any, ZoomDetailed),
    KeyBinding::new(Keycode::F5, Any, SpeedNormal),
    KeyBinding::new(Keycode::F6, Any, SpeedDouble),
    KeyBinding::new(Keycode::F7, Any, SpeedQuad),
    KeyBinding::new(Keycode::F, Any, ToggleVideoSpeechPanel),
    KeyBinding::new(Keycode::B, Any, ToggleBuild),
    KeyBinding::new(Keycode::D, Any, ToggleDiplomacyPanel),
    KeyBinding::new(Keycode::I, Any, ToggleInfoMode),
    KeyBinding::new(Keycode::K, Any, ToggleCombatMode),
    KeyBinding::new(Keycode::S, Any, ToggleShipPanel),
    KeyBinding::new(Keycode::Return, Any, CommitRouteOrChat),
    KeyBinding::new(Keycode::KpEnter, Any, CommitRouteOrChat),
    KeyBinding::new(Keycode::H, Any, CycleOwnWarehouse),
    KeyBinding::new(Keycode::C, Any, ToggleCitiesPanel),
    KeyBinding::new(Keycode::O, Any, ToggleOptionsPanel),
    KeyBinding::new(Keycode::L, Any, ToggleSavePanel),
    KeyBinding::new(Keycode::Z, Any, RotateCounterClockwise),
    KeyBinding::new(Keycode::X, Any, RotateClockwise),
    KeyBinding::new(Keycode::Num1, Ctrl, StoreTroopAssembly),
    KeyBinding::new(Keycode::Num2, Ctrl, StoreTroopAssembly),
    KeyBinding::new(Keycode::Num3, Ctrl, StoreTroopAssembly),
    KeyBinding::new(Keycode::Num4, Ctrl, StoreTroopAssembly),
    KeyBinding::new(Keycode::Num5, Ctrl, StoreTroopAssembly),
    KeyBinding::new(Keycode::Num6, Ctrl, StoreTroopAssembly),
    KeyBinding::new(Keycode::Num7, Ctrl, StoreTroopAssembly),
    KeyBinding::new(Keycode::Num8, Ctrl, StoreTroopAssembly),
    KeyBinding::new(Keycode::Num9, Ctrl, StoreTroopAssembly),
    KeyBinding::new(Keycode::Num1, Any, RecallTroopAssembly),
    KeyBinding::new(Keycode::Num2, Any, RecallTroopAssembly),
    KeyBinding::new(Keycode::Num3, Any, RecallTroopAssembly),
    KeyBinding::new(Keycode::Num4, Any, RecallTroopAssembly),
    KeyBinding::new(Keycode::Num5, Any, RecallTroopAssembly),
    KeyBinding::new(Keycode::Num6, Any, RecallTroopAssembly),
    KeyBinding::new(Keycode::Num7, Any, RecallTroopAssembly),
    KeyBinding::new(Keycode::Num8, Any, RecallTroopAssembly),
    KeyBinding::new(Keycode::Num9, Any, RecallTroopAssembly),
];

pub fn resolve_normal_mode_key(key: Keycode, ctx: InputContext) -> Option<InputAction> {
    NORMAL_MODE_BINDINGS
        .iter()
        .find(|binding| binding.key == key && binding.guard.matches(ctx))
        .map(|binding| binding.action)
}

pub fn shadowed_specific_bindings(bindings: &[KeyBinding]) -> Vec<(usize, KeyBinding)> {
    let mut out = Vec::new();
    for (idx, binding) in bindings.iter().enumerate() {
        if !binding.guard.is_specific() {
            continue;
        }
        let shadowed = bindings[..idx]
            .iter()
            .any(|earlier| earlier.key == binding.key && earlier.guard == BindingGuard::Any);
        if shadowed {
            out.push((idx, *binding));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_keys_match_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::F5, InputContext::normal()),
            Some(SpeedNormal),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::F6, InputContext::normal()),
            Some(SpeedDouble),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::F7, InputContext::normal()),
            Some(SpeedQuad),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::G, InputContext::normal()),
            None,
        );
    }

    #[test]
    fn pause_key_matches_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::Pause, InputContext::normal()),
            Some(PauseGame),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::Space, InputContext::normal()),
            None,
        );
    }

    #[test]
    fn white_flag_key_matches_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::W, InputContext::normal()),
            Some(WhiteFlagSurrender),
        );
    }

    #[test]
    fn video_speech_key_matches_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::F, InputContext::normal()),
            Some(ToggleVideoSpeechPanel),
        );
    }

    #[test]
    fn jump_key_matches_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::J, InputContext::normal()),
            Some(JumpToActiveObject),
        );
    }

    #[test]
    fn options_key_matches_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::O, InputContext::normal()),
            Some(ToggleOptionsPanel),
        );
    }

    #[test]
    fn zoom_and_save_keys_match_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::F2, InputContext::normal()),
            Some(ZoomBirdEye),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::F3, InputContext::normal()),
            Some(ZoomNormal),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::F4, InputContext::normal()),
            Some(ZoomDetailed),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::L, InputContext::normal()),
            Some(ToggleSavePanel),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::Equals, InputContext::normal()),
            None,
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::Minus, InputContext::normal()),
            None,
        );
    }

    #[test]
    fn h_key_cycles_own_warehouses_per_manual() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::H, InputContext::normal()),
            Some(CycleOwnWarehouse),
        );
    }

    #[test]
    fn diplomacy_key_matches_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::D, InputContext::normal()),
            Some(ToggleDiplomacyPanel),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::Y, InputContext::normal()),
            None,
        );
    }

    #[test]
    fn info_key_matches_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::I, InputContext::normal()),
            Some(ToggleInfoMode),
        );
    }

    #[test]
    fn combat_key_matches_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::K, InputContext::normal()),
            Some(ToggleCombatMode),
        );
    }

    #[test]
    fn rotation_keys_match_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::Z, InputContext::normal()),
            Some(RotateCounterClockwise),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::X, InputContext::normal()),
            Some(RotateClockwise),
        );
    }

    #[test]
    fn cities_key_matches_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::C, InputContext::normal()),
            Some(ToggleCitiesPanel),
        );
    }

    #[test]
    fn ships_key_matches_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::S, InputContext::normal()),
            Some(ToggleShipPanel),
        );
    }

    #[test]
    fn troop_assembly_keys_match_manual_appendix() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::Num1, InputContext::ctrl()),
            Some(StoreTroopAssembly),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::Num9, InputContext::ctrl()),
            Some(StoreTroopAssembly),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::Num1, InputContext::normal()),
            Some(RecallTroopAssembly),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::Num9, InputContext::normal()),
            Some(RecallTroopAssembly),
        );
    }

    #[test]
    fn non_manual_audio_shortcuts_are_not_bound() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::M, InputContext::normal()),
            None,
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::N, InputContext::normal()),
            None,
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::V, InputContext::normal()),
            None,
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::V, InputContext::shift()),
            None,
        );
    }

    #[test]
    fn non_manual_objectives_shortcuts_are_not_bound() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::Question, InputContext::normal()),
            None,
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::Slash, InputContext::normal()),
            None,
        );
    }

    #[test]
    fn route_list_shortcut_is_not_bound() {
        assert!(!NORMAL_MODE_BINDINGS
            .iter()
            .any(|binding| binding.key == Keycode::R && binding.guard == BindingGuard::Shift));
    }

    #[test]
    fn trade_route_editor_shortcuts_are_not_bound() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::R, InputContext::normal()),
            None,
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::U, InputContext::normal()),
            None,
        );
    }

    #[test]
    fn market_shortcut_is_not_bound() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::A, InputContext::normal()),
            None,
        );
    }

    #[test]
    fn tax_shortcut_is_not_bound() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::T, InputContext::normal()),
            None,
        );
    }

    #[test]
    fn plain_bindings_still_resolve_without_context_flags() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::F6, InputContext::normal()),
            Some(SpeedDouble),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::B, InputContext::normal()),
            Some(ToggleBuild),
        );
    }

    #[test]
    fn no_specific_binding_is_shadowed_by_an_earlier_plain_binding() {
        assert_eq!(shadowed_specific_bindings(NORMAL_MODE_BINDINGS), Vec::new());
    }
}
