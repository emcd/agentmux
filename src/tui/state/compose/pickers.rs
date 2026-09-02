use crate::runtime::error::RuntimeError;

use super::{AppState, PickerColumn, ScreenMode, append_recipient_token};

impl AppState {
    /// Opens the unified picker focused on the session column. Used by the
    /// recipient-oriented entry points: the session-picker action, and entering
    /// Interaction mode without a target.
    pub fn open_picker(&mut self) {
        self.open_picker_focused(PickerColumn::Sessions);
    }

    /// Opens the unified picker focused on the bundle column. The bundle
    /// column drives active-bundle switching; selecting a bundle re-enumerates
    /// the session column in the same window.
    pub fn open_bundle_picker(&mut self) {
        self.open_picker_focused(PickerColumn::Bundles);
    }

    fn open_picker_focused(&mut self, focus: PickerColumn) {
        self.picker_open = true;
        self.events_overlay_open = false;
        self.help_overlay_open = false;
        self.picker_focus = focus;
        self.picker_filter.clear();
        self.sync_bundle_selection_to_active();
        self.apply_recipient_list_update();
    }

    pub fn close_picker(&mut self) {
        self.picker_open = false;
        self.picker_filter.clear();
    }

    /// Closes the picker and both overlays. A surface-switching behavior applies
    /// it first so the change lands on the mode beneath, whichever surface the
    /// operator invoked it from.
    pub fn dismiss_surfaces(&mut self) {
        self.close_picker();
        self.events_overlay_open = false;
        self.help_overlay_open = false;
    }

    /// Moves picker focus to the session column and clears the column-scoped
    /// filter. Used after a bundle commit so the freshly enumerated sessions are
    /// immediately navigable in the same window.
    pub(crate) fn focus_picker_session_column(&mut self) {
        self.picker_focus = PickerColumn::Sessions;
        self.picker_filter.clear();
    }

    /// Switches keyboard focus between the bundle and session columns. The
    /// filter is column-scoped, so the focused column's selection is first
    /// resolved to a real (unfiltered) index before the filter clears.
    pub fn toggle_picker_focus(&mut self) {
        self.commit_focused_selection_to_real_index();
        self.picker_filter.clear();
        self.picker_focus = match self.picker_focus {
            PickerColumn::Bundles => PickerColumn::Sessions,
            PickerColumn::Sessions => PickerColumn::Bundles,
        };
    }

    pub fn picker_filter_push(&mut self, character: char) {
        self.picker_filter.push(character);
        self.reselect_focused_after_filter_change();
    }

    pub fn picker_filter_backspace(&mut self) {
        if self.picker_filter.pop().is_some() {
            self.reselect_focused_after_filter_change();
        }
    }

    pub fn toggle_events_overlay(&mut self) {
        self.events_overlay_open = !self.events_overlay_open;
        if self.events_overlay_open {
            self.close_picker();
            self.help_overlay_open = false;
            self.ensure_pending_choice_selection();
        }
    }

    pub fn toggle_help_overlay(&mut self) {
        self.help_overlay_open = !self.help_overlay_open;
        if self.help_overlay_open {
            self.close_picker();
            self.events_overlay_open = false;
        }
    }

    pub fn move_picker_selection(&mut self, delta: isize) {
        match self.picker_focus {
            PickerColumn::Bundles => self.move_bundle_selection(delta),
            PickerColumn::Sessions => self.move_session_selection(delta),
        }
    }

    fn move_session_selection(&mut self, delta: isize) {
        let visible = self.visible_session_indices();
        if visible.is_empty() {
            self.picker_session_state.select(None);
            return;
        }
        let current = self.picker_session_state.selected().unwrap_or(0);
        let next = super::text_util::wrap_index(current, delta, visible.len());
        self.picker_session_state.select(Some(next));
    }

    fn move_bundle_selection(&mut self, delta: isize) {
        let visible = self.visible_bundle_indices();
        if visible.is_empty() {
            self.picker_bundle_state.select(None);
            return;
        }
        let current = self.picker_bundle_state.selected().unwrap_or(0);
        let next = super::text_util::wrap_index(current, delta, visible.len());
        self.picker_bundle_state.select(Some(next));
    }

    /// Real indices into `recipients` for the currently visible session rows.
    /// The filter applies only when the session column is focused; otherwise the
    /// full list is visible.
    pub(crate) fn visible_session_indices(&self) -> Vec<usize> {
        let filter = self.column_filter(PickerColumn::Sessions);
        filter_matching_indices(
            self.recipients.iter().map(|recipient| {
                (
                    recipient.session_name.as_str(),
                    recipient.display_name.as_deref(),
                )
            }),
            filter,
        )
    }

    /// Real indices into `available_bundles` for the currently visible bundle
    /// rows, honoring the column-scoped filter.
    pub(crate) fn visible_bundle_indices(&self) -> Vec<usize> {
        let filter = self.column_filter(PickerColumn::Bundles);
        filter_matching_indices(
            self.available_bundles
                .iter()
                .map(|name| (name.as_str(), None)),
            filter,
        )
    }

    fn column_filter(&self, column: PickerColumn) -> &str {
        if self.picker_focus == column {
            self.picker_filter.as_str()
        } else {
            ""
        }
    }

    /// Commits the picker's selected recipient to the `To` field. Insertion does
    /// not depend on compose focus: the picker is the recipient affordance and
    /// `To` is the only field a recipient can land in, so an operator who opens
    /// it from the message body gets the recipient they asked for and keeps
    /// their compose focus where it was.
    pub fn insert_picker_selection(&mut self) {
        let Some(session_name) = self.selected_picker_recipient_id() else {
            self.push_status(
                Some("validation_unknown_target".to_string()),
                "picker has no selected recipient",
            );
            return;
        };
        self.to_field = append_recipient_token(&self.to_field, session_name.as_str());
        self.snap_to_field_cursor_to_end();
        self.last_selected_recipient = Some(session_name.clone());
        self.close_picker();
        self.push_status(None, format!("Inserted recipient {session_name}."));
    }

    /// Commits the selected session-column row, which the active screen mode
    /// interprets: Communication inserts the recipient into `To`, Interaction
    /// opens the Interaction screen on it.
    ///
    /// # Errors
    ///
    /// Returns the `RuntimeError` from the relay `Look` that Interaction-mode
    /// entry performs.
    pub fn commit_selected_picker_session(&mut self) -> Result<(), RuntimeError> {
        match self.mode {
            ScreenMode::Communication => {
                self.insert_picker_selection();
                Ok(())
            }
            ScreenMode::Interaction => self.enter_interaction_from_picker(),
        }
    }

    pub fn apply_recipient_list_update(&mut self) {
        if self.recipients.is_empty() {
            self.picker_session_state.select(None);
            return;
        }
        let index = self.resolve_last_selected_recipient_index().unwrap_or(0);
        self.picker_session_state.select(Some(index));
    }

    pub(super) fn selected_picker_recipient_id(&self) -> Option<String> {
        let real = self.selected_session_real_index()?;
        self.recipients
            .get(real)
            .map(|recipient| recipient.session_name.clone())
    }

    pub(in crate::tui::state) fn selected_picker_bundle(&self) -> Option<String> {
        let real = self.selected_bundle_real_index()?;
        self.available_bundles.get(real).cloned()
    }

    fn selected_session_real_index(&self) -> Option<usize> {
        let position = self.picker_session_state.selected()?;
        self.visible_session_indices().get(position).copied()
    }

    fn selected_bundle_real_index(&self) -> Option<usize> {
        let position = self.picker_bundle_state.selected()?;
        self.visible_bundle_indices().get(position).copied()
    }

    fn sync_bundle_selection_to_active(&mut self) {
        if self.available_bundles.is_empty() {
            self.picker_bundle_state.select(None);
            return;
        }
        let active_index = self
            .available_bundles
            .iter()
            .position(|name| name == &self.namespace)
            .unwrap_or(0);
        self.picker_bundle_state.select(Some(active_index));
    }

    /// After the filter changes, the visible list shrinks or grows. Reset the
    /// focused column to the first match (or clear it when nothing matches), the
    /// conventional filter-box behavior.
    fn reselect_focused_after_filter_change(&mut self) {
        match self.picker_focus {
            PickerColumn::Sessions => {
                let any = !self.visible_session_indices().is_empty();
                self.picker_session_state.select(any.then_some(0));
            }
            PickerColumn::Bundles => {
                let any = !self.visible_bundle_indices().is_empty();
                self.picker_bundle_state.select(any.then_some(0));
            }
        }
    }

    /// Converts the focused column's filtered selection position into the real
    /// (unfiltered) index. Called before clearing the filter on a focus switch
    /// so the selection survives the list returning to its full length.
    fn commit_focused_selection_to_real_index(&mut self) {
        match self.picker_focus {
            PickerColumn::Sessions => {
                let real = self.selected_session_real_index();
                self.picker_session_state.select(real);
            }
            PickerColumn::Bundles => {
                let real = self.selected_bundle_real_index();
                self.picker_bundle_state.select(real);
            }
        }
    }

    fn resolve_last_selected_recipient_index(&self) -> Option<usize> {
        let name = self.last_selected_recipient.as_deref()?;
        self.recipients
            .iter()
            .position(|recipient| recipient.session_name == name)
    }
}

/// Returns the indices of items whose primary or display label contains the
/// filter, case-insensitively. An empty filter matches everything.
fn filter_matching_indices<'a>(
    items: impl Iterator<Item = (&'a str, Option<&'a str>)>,
    filter: &str,
) -> Vec<usize> {
    if filter.is_empty() {
        return items.enumerate().map(|(index, _)| index).collect();
    }
    let needle = filter.to_ascii_lowercase();
    items
        .enumerate()
        .filter_map(|(index, (primary, secondary))| {
            let primary_hit = primary.to_ascii_lowercase().contains(&needle);
            let secondary_hit = secondary
                .map(|value| value.to_ascii_lowercase().contains(&needle))
                .unwrap_or(false);
            (primary_hit || secondary_hit).then_some(index)
        })
        .collect()
}
