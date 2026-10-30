#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PauseAction {
    Resume,
    Help,
    Restart,
    Leave,
}

impl PauseAction {
    pub(super) fn label(self, network: bool) -> &'static str {
        match self {
            Self::Resume => "Resume battle",
            Self::Help => "Help",
            Self::Restart => "Restart match",
            Self::Leave if network => "Leave game",
            Self::Leave => "Main menu",
        }
    }
}

pub(super) fn pause_actions(network_host: Option<bool>) -> &'static [PauseAction] {
    use PauseAction::*;
    if network_host == Some(false) {
        &[Resume, Help, Leave]
    } else {
        &[Resume, Help, Restart, Leave]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResultsAction {
    Rematch,
    Configure,
    Leave,
}

impl ResultsAction {
    pub(super) fn label(self, network: bool, gold: bool) -> &'static str {
        match self {
            Self::Rematch if gold => "Rematch / Gold roulette",
            Self::Rematch => "Rematch",
            Self::Configure if network => "Return to lobby",
            Self::Configure => "Change players and arena",
            Self::Leave if network => "Leave game",
            Self::Leave => "Main menu",
        }
    }
}

pub(super) fn results_actions(network_host: Option<bool>) -> &'static [ResultsAction] {
    use ResultsAction::*;
    if network_host == Some(false) {
        &[Leave]
    } else {
        &[Rematch, Configure, Leave]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtered_client_rows_keep_their_actions_after_host_entries_are_removed() {
        assert_eq!(pause_actions(Some(false))[2], PauseAction::Leave);
        assert_eq!(results_actions(Some(false)), &[ResultsAction::Leave]);
        assert!(pause_actions(Some(false)).contains(&PauseAction::Resume));
        assert!(pause_actions(Some(false)).contains(&PauseAction::Help));
        assert!(!pause_actions(Some(false)).contains(&PauseAction::Restart));
        for role in [None, Some(true)] {
            assert_eq!(pause_actions(role)[2], PauseAction::Restart);
            assert_eq!(results_actions(role)[0], ResultsAction::Rematch);
            assert_eq!(results_actions(role)[1], ResultsAction::Configure);
        }
    }
}
