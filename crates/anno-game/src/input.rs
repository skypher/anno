//! Input binding specifications for the game binary.
//!
//! The SDL loop still performs the side effects directly, but these
//! bindings give tests a stable place to catch ordering mistakes such as
//! a plain key arm shadowing its Shift or editor-mode variant.

use sdl2::keyboard::Keycode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputContext {
    pub shift_held: bool,
    pub editor_mode: bool,
    pub trade_route_mode: bool,
}

impl InputContext {
    pub const fn normal() -> Self {
        Self {
            shift_held: false,
            editor_mode: false,
            trade_route_mode: false,
        }
    }

    pub const fn shift() -> Self {
        Self {
            shift_held: true,
            editor_mode: false,
            trade_route_mode: false,
        }
    }

    pub const fn editor() -> Self {
        Self {
            shift_held: false,
            editor_mode: true,
            trade_route_mode: false,
        }
    }

    pub const fn trade_route() -> Self {
        Self {
            shift_held: false,
            editor_mode: false,
            trade_route_mode: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingGuard {
    Any,
    Shift,
    EditorMode,
    TradeRouteMode,
}

impl BindingGuard {
    pub fn matches(self, ctx: InputContext) -> bool {
        match self {
            Self::Any => true,
            Self::Shift => ctx.shift_held,
            Self::EditorMode => ctx.editor_mode,
            Self::TradeRouteMode => ctx.trade_route_mode,
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
    ToggleWorld,
    TogglePause,
    SpeedDown,
    SpeedUp,
    RouteStopLoadOnly,
    RouteStopUnloadOnly,
    RouteStopBoth,
    ToggleBuild,
    ToggleDemolish,
    ToggleTaxPanel,
    ToggleDiplomacyPanel,
    ToggleRouteList,
    ToggleTradeRouteDraft,
    CommitRouteOrChat,
    ToggleMusic,
    NextTrack,
    ToggleEvaluationPanel,
    CycleVolume,
    SaveScreenshot,
    ToggleHud,
    ToggleCitiesPanel,
    ToggleCoverage,
    ToggleMarketPanel,
    ToggleMusicPanel,
    ToggleShipPanel,
    ToggleObjectivesPanel,
    ToggleScenarioPicker,
    ToggleSavePanel,
    ToggleSettingsPanel,
    ToggleHelpPanel,
    TogglePerf,
    EditorAddGoldObjective,
    EditorAddPopulationObjective,
    EditorRemoveObjective,
    ToggleEditor,
    EditorPrevOwner,
    EditorNextOwner,
    ExportScenario,
    FoundColony,
    BuildTradeShip,
    TogglePathOverlay,
    Quicksave,
    Quickload,
    ZoomIn,
    ZoomOut,
    FormationHorizontal,
    FormationVertical,
    FormationQuad,
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

use BindingGuard::{Any, EditorMode, Shift, TradeRouteMode};
use InputAction::*;

pub const NORMAL_MODE_BINDINGS: &[KeyBinding] = &[
    KeyBinding::new(Keycode::Left, Any, ScrollLeft),
    KeyBinding::new(Keycode::Right, Any, ScrollRight),
    KeyBinding::new(Keycode::Up, Any, ScrollUp),
    KeyBinding::new(Keycode::Down, Any, ScrollDown),
    KeyBinding::new(Keycode::Tab, Any, NextIsland),
    KeyBinding::new(Keycode::W, Any, ToggleWorld),
    KeyBinding::new(Keycode::Space, Any, TogglePause),
    KeyBinding::new(Keycode::F, Any, SpeedDown),
    KeyBinding::new(Keycode::G, EditorMode, EditorAddGoldObjective),
    KeyBinding::new(Keycode::G, Any, SpeedUp),
    KeyBinding::new(Keycode::L, TradeRouteMode, RouteStopLoadOnly),
    KeyBinding::new(Keycode::U, TradeRouteMode, RouteStopUnloadOnly),
    KeyBinding::new(Keycode::B, TradeRouteMode, RouteStopBoth),
    KeyBinding::new(Keycode::B, Any, ToggleBuild),
    KeyBinding::new(Keycode::D, Any, ToggleDemolish),
    KeyBinding::new(Keycode::T, Any, ToggleTaxPanel),
    KeyBinding::new(Keycode::Y, Any, ToggleDiplomacyPanel),
    KeyBinding::new(Keycode::R, Shift, ToggleRouteList),
    KeyBinding::new(Keycode::R, Any, ToggleTradeRouteDraft),
    KeyBinding::new(Keycode::Return, Any, CommitRouteOrChat),
    KeyBinding::new(Keycode::KpEnter, Any, CommitRouteOrChat),
    KeyBinding::new(Keycode::M, Any, ToggleMusic),
    KeyBinding::new(Keycode::N, Any, NextTrack),
    KeyBinding::new(Keycode::V, Shift, ToggleEvaluationPanel),
    KeyBinding::new(Keycode::V, Any, CycleVolume),
    KeyBinding::new(Keycode::S, Any, SaveScreenshot),
    KeyBinding::new(Keycode::H, Any, ToggleHud),
    KeyBinding::new(Keycode::C, Shift, ToggleCitiesPanel),
    KeyBinding::new(Keycode::C, Any, ToggleCoverage),
    KeyBinding::new(Keycode::A, Any, ToggleMarketPanel),
    KeyBinding::new(Keycode::J, Shift, ToggleMusicPanel),
    KeyBinding::new(Keycode::J, Any, ToggleShipPanel),
    KeyBinding::new(Keycode::Question, Any, ToggleObjectivesPanel),
    KeyBinding::new(Keycode::Slash, Any, ToggleObjectivesPanel),
    KeyBinding::new(Keycode::F2, Any, ToggleScenarioPicker),
    KeyBinding::new(Keycode::F3, Any, ToggleSavePanel),
    KeyBinding::new(Keycode::F10, Any, ToggleSettingsPanel),
    KeyBinding::new(Keycode::F11, Any, ToggleHelpPanel),
    KeyBinding::new(Keycode::F12, Any, TogglePerf),
    KeyBinding::new(Keycode::P, EditorMode, EditorAddPopulationObjective),
    KeyBinding::new(Keycode::Backspace, EditorMode, EditorRemoveObjective),
    KeyBinding::new(Keycode::E, Shift, ToggleEditor),
    KeyBinding::new(Keycode::LeftBracket, EditorMode, EditorPrevOwner),
    KeyBinding::new(Keycode::RightBracket, EditorMode, EditorNextOwner),
    KeyBinding::new(Keycode::F8, Any, ExportScenario),
    KeyBinding::new(Keycode::F7, Any, FoundColony),
    KeyBinding::new(Keycode::F4, Any, BuildTradeShip),
    KeyBinding::new(Keycode::F6, Any, TogglePathOverlay),
    KeyBinding::new(Keycode::F5, Any, Quicksave),
    KeyBinding::new(Keycode::F9, Any, Quickload),
    KeyBinding::new(Keycode::Equals, Any, ZoomIn),
    KeyBinding::new(Keycode::Plus, Any, ZoomIn),
    KeyBinding::new(Keycode::KpPlus, Any, ZoomIn),
    KeyBinding::new(Keycode::Minus, Any, ZoomOut),
    KeyBinding::new(Keycode::KpMinus, Any, ZoomOut),
    KeyBinding::new(Keycode::Num1, Any, FormationHorizontal),
    KeyBinding::new(Keycode::Num2, Any, FormationVertical),
    KeyBinding::new(Keycode::Num3, Any, FormationQuad),
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
    fn shifted_bindings_resolve_before_plain_bindings() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::V, InputContext::shift()),
            Some(ToggleEvaluationPanel),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::C, InputContext::shift()),
            Some(ToggleCitiesPanel),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::J, InputContext::shift()),
            Some(ToggleMusicPanel),
        );
    }

    #[test]
    fn editor_and_trade_route_bindings_resolve_before_plain_bindings() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::G, InputContext::editor()),
            Some(EditorAddGoldObjective),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::B, InputContext::trade_route()),
            Some(RouteStopBoth),
        );
    }

    #[test]
    fn plain_bindings_still_resolve_without_context_flags() {
        assert_eq!(
            resolve_normal_mode_key(Keycode::V, InputContext::normal()),
            Some(CycleVolume),
        );
        assert_eq!(
            resolve_normal_mode_key(Keycode::G, InputContext::normal()),
            Some(SpeedUp),
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
