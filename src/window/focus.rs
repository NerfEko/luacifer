use crate::window::model::WindowId;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FocusStack {
    focused: Option<WindowId>,
    mru: Vec<WindowId>,
}

impl FocusStack {
    pub fn focused(&self) -> Option<WindowId> {
        self.focused
    }

    pub fn order(&self) -> &[WindowId] {
        &self.mru
    }

    pub fn focus(&mut self, id: WindowId) {
        self.mru.retain(|existing| *existing != id);
        self.mru.insert(0, id);
        self.focused = Some(id);
    }

    pub fn remove(&mut self, id: WindowId) {
        self.mru.retain(|existing| *existing != id);
        if self.focused == Some(id) {
            self.focused = self.mru.first().copied();
        }
    }

    pub fn remove_without_fallback(&mut self, id: WindowId) {
        self.mru.retain(|existing| *existing != id);
        if self.focused == Some(id) {
            self.focused = None;
        }
    }

    pub fn clear_focus_only(&mut self) {
        self.focused = None;
    }

    pub fn retain<F>(&mut self, mut keep: F)
    where
        F: FnMut(WindowId) -> bool,
    {
        self.mru.retain(|id| keep(*id));
        if self.focused.is_some_and(|id| !keep(id)) {
            self.focused = None;
        }
    }

    pub fn cycle_forward(&mut self) -> Option<WindowId> {
        if self.mru.len() > 1 {
            self.mru.rotate_left(1);
            self.focused = self.mru.first().copied();
        }
        self.focused
    }

    pub fn cycle_backward(&mut self) -> Option<WindowId> {
        if self.mru.len() > 1 {
            self.mru.rotate_right(1);
            self.focused = self.mru.first().copied();
        }
        self.focused
    }
}
