//! Raster-coordinate marks and editing history; no browser or wire-format state.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Point {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Rectangle,
    Arrow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Shape {
    pub(crate) kind: Kind,
    pub(crate) start: Point,
    pub(crate) end: Point,
}

impl Shape {
    pub(crate) fn bounds(self) -> (Point, Point) {
        (
            Point {
                x: self.start.x.min(self.end.x),
                y: self.start.y.min(self.end.y),
            },
            Point {
                x: self.start.x.max(self.end.x),
                y: self.start.y.max(self.end.y),
            },
        )
    }

    pub(crate) fn translated(self, dx: i32, dy: i32, width: i32, height: i32) -> Self {
        let (low, high) = self.bounds();
        let dx = dx.clamp(-low.x, width - high.x);
        let dy = dy.clamp(-low.y, height - high.y);
        Self {
            start: Point {
                x: self.start.x + dx,
                y: self.start.y + dy,
            },
            end: Point {
                x: self.end.x + dx,
                y: self.end.y + dy,
            },
            ..self
        }
    }

    pub(crate) fn visible(self) -> bool {
        match self.kind {
            Kind::Rectangle => self.start.x != self.end.x && self.start.y != self.end.y,
            Kind::Arrow => self.start != self.end,
        }
    }

    /// The same arrowhead geometry is used by SVG presentation and PNG export.
    pub(crate) fn arrowhead(self, stroke: f64) -> [(f64, f64); 3] {
        let dx = f64::from(self.end.x - self.start.x);
        let dy = f64::from(self.end.y - self.start.y);
        let distance = dx.hypot(dy);
        let length = (stroke * 4.0).min(distance);
        let angle = dy.atan2(dx);
        let (x, y) = (f64::from(self.end.x), f64::from(self.end.y));
        [
            (x, y),
            (
                x - length * (angle - 0.45).cos(),
                y - length * (angle - 0.45).sin(),
            ),
            (
                x - length * (angle + 0.45).cos(),
                y - length * (angle + 0.45).sin(),
            ),
        ]
    }
}

pub(crate) fn stroke_width(width: u32, height: u32) -> f64 {
    (f64::from(width.max(height)) / 250.0).clamp(3.0, 14.0)
}

#[derive(Default, PartialEq, Eq)]
pub(crate) struct Editor {
    pub(crate) shapes: Vec<Shape>,
    pub(crate) selected: Option<usize>,
    undo: Vec<Vec<Shape>>,
    redo: Vec<Vec<Shape>>,
}

impl Editor {
    pub(crate) fn new(shapes: Vec<Shape>) -> Self {
        Self {
            shapes,
            ..Self::default()
        }
    }

    pub(crate) fn commit(&mut self, before: Vec<Shape>) {
        if self.shapes != before {
            self.undo.push(before);
            self.redo.clear();
        }
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub(crate) fn undo(&mut self) {
        if let Some(previous) = self.undo.pop() {
            self.redo
                .push(std::mem::replace(&mut self.shapes, previous));
            self.selected = None;
        }
    }

    pub(crate) fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.shapes, next));
            self.selected = None;
        }
    }

    pub(crate) fn delete(&mut self) {
        if let Some(index) = self.selected.take() {
            let before = self.shapes.clone();
            self.shapes.remove(index);
            self.commit(before);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversed_rectangles_and_arrows_move_without_crossing_image_edges() {
        for kind in [Kind::Rectangle, Kind::Arrow] {
            let shape = Shape {
                kind,
                start: Point { x: 90, y: 70 },
                end: Point { x: 20, y: 10 },
            };
            let moved = shape.translated(100, -100, 120, 80);
            assert_eq!(moved.start, Point { x: 120, y: 60 });
            assert_eq!(moved.end, Point { x: 50, y: 0 });
            assert_eq!(moved.end.x - moved.start.x, shape.end.x - shape.start.x);
            assert_eq!(moved.end.y - moved.start.y, shape.end.y - shape.start.y);
        }
    }

    #[test]
    fn gesture_history_delete_and_new_edit_preserve_only_reachable_states() {
        let rectangle = Shape {
            kind: Kind::Rectangle,
            start: Point { x: 10, y: 10 },
            end: Point { x: 50, y: 40 },
        };
        let arrow = Shape {
            kind: Kind::Arrow,
            start: Point { x: 60, y: 20 },
            end: Point { x: 80, y: 50 },
        };
        let mut editor = Editor::new(vec![rectangle]);
        let before = editor.shapes.clone();
        editor.shapes.push(arrow);
        editor.commit(before);
        assert!(editor.can_undo());
        editor.undo();
        assert_eq!(editor.shapes, [rectangle]);
        editor.redo();
        assert_eq!(editor.shapes, [rectangle, arrow]);
        editor.selected = Some(0);
        editor.delete();
        assert_eq!(editor.shapes, [arrow]);
        editor.undo();
        assert_eq!(editor.shapes, [rectangle, arrow]);
        let before = editor.shapes.clone();
        editor.shapes[1].end = Point { x: 100, y: 70 };
        editor.commit(before);
        assert!(!editor.can_redo());
        editor.undo();
        assert_eq!(editor.shapes, [rectangle, arrow]);
    }
}

#[cfg(test)]
mod boundaries {
    use super::*;

    #[test]
    fn degenerate_marks_are_discarded_and_short_arrows_have_bounded_heads() {
        let point = Point { x: 10, y: 20 };
        for kind in [Kind::Rectangle, Kind::Arrow] {
            assert!(
                !Shape {
                    kind,
                    start: point,
                    end: point
                }
                .visible()
            );
        }
        assert!(
            !Shape {
                kind: Kind::Rectangle,
                start: point,
                end: Point { x: 10, y: 80 }
            }
            .visible()
        );
        let arrow = Shape {
            kind: Kind::Arrow,
            start: point,
            end: Point { x: 12, y: 22 },
        };
        assert!(arrow.visible());
        let head = arrow.arrowhead(stroke_width(1200, 800));
        assert_eq!(head[0], (12.0, 22.0));
        for (x, y) in head {
            assert!((x - 12.0).hypot(y - 22.0) <= 2.0_f64.sqrt() * 2.0 + f64::EPSILON * 16.0);
        }
    }
}
