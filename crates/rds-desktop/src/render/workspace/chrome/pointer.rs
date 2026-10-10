//! Pointer ownership follows the last painted geometry, not delayed egui input.
use std::collections::HashSet;

use winit::event::{ElementState, MouseButton, WindowEvent};

#[derive(Default)]
pub(super) struct LocalPointer {
    position: Option<egui::Pos2>,
    held: HashSet<MouseButton>,
}

impl LocalPointer {
    pub fn event(&mut self, event: &WindowEvent, frame: &super::UiFrame) -> bool {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.position = Some(egui::pos2(position.x as f32, position.y as f32));
                self.local(frame)
            }
            WindowEvent::MouseInput { state, button, .. } => self.button(*button, *state, frame),
            WindowEvent::MouseWheel { .. } => self.local(frame),
            WindowEvent::CursorLeft { .. } => {
                self.position = None;
                false
            }
            WindowEvent::Focused(false) => {
                self.position = None;
                self.held.clear();
                false
            }
            _ => false,
        }
    }

    fn local(&self, frame: &super::UiFrame) -> bool {
        !self.held.is_empty()
            || self.position.is_some_and(|point| {
                !frame.content.contains(point) || frame.restore.contains(point)
            })
    }

    fn button(&mut self, button: MouseButton, state: ElementState, frame: &super::UiFrame) -> bool {
        let local = self.local(frame);
        match state {
            ElementState::Pressed if local => {
                self.held.insert(button);
                true
            }
            ElementState::Released => self.held.remove(&button) || local,
            _ => local,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_press_and_release_stay_local_across_layout_and_pointer_changes() {
        let mut frame = super::super::UiFrame::empty(2.);
        frame.content = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(2000., 1400.));
        frame.restore = egui::Rect::from_min_max(egui::pos2(964., 0.), egui::pos2(1036., 52.));
        let mut pointer = LocalPointer {
            position: Some(egui::pos2(1000., 20.)),
            ..Default::default()
        };
        assert!(pointer.button(MouseButton::Left, ElementState::Pressed, &frame));
        frame.restore = egui::Rect::NOTHING;
        frame.content.min.y = 110.;
        pointer.position = Some(egui::pos2(1000., 500.));
        assert!(pointer.local(&frame));
        assert!(pointer.button(MouseButton::Left, ElementState::Released, &frame));
        assert!(!pointer.local(&frame));
        assert!(!pointer.button(MouseButton::Left, ElementState::Pressed, &frame));
        assert!(!pointer.button(MouseButton::Left, ElementState::Released, &frame));
    }

    #[test]
    fn only_the_restore_rectangle_blocks_the_expanded_remote_content() {
        let mut frame = super::super::UiFrame::empty(1.);
        frame.content = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1000., 700.));
        frame.restore = egui::Rect::from_min_max(egui::pos2(482., 0.), egui::pos2(518., 26.));
        let mut pointer = LocalPointer::default();
        for (point, local) in [
            (egui::pos2(500., 13.), true),
            (egui::pos2(480., 13.), false),
            (egui::pos2(520., 13.), false),
            (egui::pos2(500., 28.), false),
            (egui::pos2(999., 699.), false),
            (egui::pos2(-1., 20.), true),
        ] {
            pointer.position = Some(point);
            assert_eq!(pointer.local(&frame), local, "{point:?}");
        }
    }
}
