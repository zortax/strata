//! Pure tab-state logic of the planning context panel (design §3.4).

/// The context panel's tabs, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContextTab {
    /// The explorer's selection cards, unchanged.
    #[default]
    Inspect,
    /// Route weather briefing view (METAR/TAF, winds aloft, SIGMETs).
    Weather,
    /// W&B loading editor + CG envelope plot.
    Loading,
    /// Fuel ladder + policy editor.
    Fuel,
    /// NOTAM briefing + export actions (Briefing PDF, ICAO FPL).
    Briefing,
}

impl ContextTab {
    /// Tabs in display order — drives the segmented tab bar.
    pub const ALL: [ContextTab; 5] = [
        ContextTab::Inspect,
        ContextTab::Weather,
        ContextTab::Loading,
        ContextTab::Fuel,
        ContextTab::Briefing,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ContextTab::Inspect => "Inspect",
            ContextTab::Weather => "Weather",
            ContextTab::Loading => "Loading",
            ContextTab::Fuel => "Fuel",
            ContextTab::Briefing => "Briefing",
        }
    }

    /// Position in [`Self::ALL`] (the tab bar's selected index).
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|t| *t == self)
            .expect("every tab is in ALL")
    }

    /// Inverse of [`Self::index`]; out-of-range clicks (impossible from the
    /// tab bar, defensive anyway) keep the current tab.
    pub fn from_index(index: usize) -> Option<ContextTab> {
        Self::ALL.get(index).copied()
    }
}

/// Tab to activate after a selection change: a fresh map selection pulls
/// the panel to Inspect (the click means "show me this feature" — also the
/// path design §3.3 takes for "click an airspace band → its info card");
/// a cleared selection leaves the user where they were.
pub fn tab_after_selection_change(current: ContextTab, selection_is_empty: bool) -> ContextTab {
    if selection_is_empty {
        current
    } else {
        ContextTab::Inspect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_round_trip_in_display_order() {
        for (i, tab) in ContextTab::ALL.into_iter().enumerate() {
            assert_eq!(tab.index(), i);
            assert_eq!(ContextTab::from_index(i), Some(tab));
        }
        assert_eq!(ContextTab::from_index(ContextTab::ALL.len()), None);
    }

    #[test]
    fn inspect_is_the_default_and_first_tab() {
        assert_eq!(ContextTab::default(), ContextTab::Inspect);
        assert_eq!(ContextTab::ALL[0], ContextTab::Inspect);
    }

    #[test]
    fn labels_match_the_design_doc() {
        let labels: Vec<&str> = ContextTab::ALL.iter().map(|t| t.label()).collect();
        assert_eq!(
            labels,
            ["Inspect", "Weather", "Loading", "Fuel", "Briefing"]
        );
    }

    #[test]
    fn a_new_selection_pulls_the_panel_to_inspect() {
        assert_eq!(
            tab_after_selection_change(ContextTab::Fuel, false),
            ContextTab::Inspect
        );
        assert_eq!(
            tab_after_selection_change(ContextTab::Inspect, false),
            ContextTab::Inspect
        );
    }

    #[test]
    fn a_cleared_selection_keeps_the_current_tab() {
        assert_eq!(
            tab_after_selection_change(ContextTab::Weather, true),
            ContextTab::Weather
        );
        assert_eq!(
            tab_after_selection_change(ContextTab::Loading, true),
            ContextTab::Loading
        );
    }
}
