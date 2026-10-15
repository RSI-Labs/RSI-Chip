//! Layout editor undo/redo and interactive pointer-session state machine.

use super::*;

impl LayoutEditor {
    pub fn new(document: LayoutDocument) -> Result<Self, String> {
        Ok(Self {
            document,
            undo: Vec::new(),
            redo: Vec::new(),
        })
    }

    pub fn document(&self) -> &LayoutDocument {
        &self.document
    }

    pub fn apply(&mut self, edit: LayoutEdit) -> Result<(), String> {
        let old = self.document.clone();
        match edit {
            LayoutEdit::SetLayerRule(rule) => self.document.set_layer_rule(rule)?,
            LayoutEdit::SetViaRule(rule) => self.document.set_via_rule(rule)?,
            LayoutEdit::AddShape(shape) => self.document.add_shape(shape)?,
            LayoutEdit::RemoveShape { id } => {
                self.document.remove_shape(&id)?;
            }
            LayoutEdit::MoveShape { id, dx, dy } => self.document.move_shape(&id, dx, dy)?,
            LayoutEdit::SetShapeGeometry { id, geometry } => {
                self.document.set_shape_geometry(&id, geometry)?;
            }
            LayoutEdit::AddVia(via) => self.document.add_via(via)?,
            LayoutEdit::RemoveVia { id } => {
                self.document.remove_via(&id)?;
            }
        }
        self.undo.push(old);
        self.redo.clear();
        Ok(())
    }

    pub fn undo(&mut self) -> Result<bool, String> {
        let Some(previous) = self.undo.pop() else {
            return Ok(false);
        };
        self.redo
            .push(std::mem::replace(&mut self.document, previous));
        Ok(true)
    }

    pub fn redo(&mut self) -> Result<bool, String> {
        let Some(next) = self.redo.pop() else {
            return Ok(false);
        };
        self.undo.push(std::mem::replace(&mut self.document, next));
        Ok(true)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutSelection {
    None,
    Shape { id: String },
    ShapeVertex { id: String, vertex: usize },
    Via { id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutTool {
    Select,
    Rectangle,
    Polygon,
    AnyAnglePolygon,
    Via,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayoutEvent {
    PointerDown { point: LayoutPoint },
    PointerMove { point: LayoutPoint },
    PointerUp { point: LayoutPoint },
    SetTool { tool: LayoutTool },
    FinishPolygon,
    Undo,
    Redo,
    Delete,
    CheckDrc,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayoutEventOutcome {
    Ignored,
    SelectionChanged,
    Dragging {
        id: String,
        bbox: LayoutRect,
    },
    ShapeMoved {
        id: String,
    },
    ShapeVertexMoved {
        id: String,
        vertex: usize,
        point: LayoutPoint,
    },
    RectanglePreviewChanged {
        start: LayoutPoint,
        end: LayoutPoint,
    },
    PolygonPreviewChanged {
        points: Vec<LayoutPoint>,
    },
    ShapeAdded {
        id: String,
    },
    ViaAdded {
        id: String,
    },
    ToolChanged {
        tool: LayoutTool,
    },
    UndoApplied {
        changed: bool,
    },
    RedoApplied {
        changed: bool,
    },
    Deleted {
        selection: LayoutSelection,
    },
    DrcChecked {
        clean: bool,
        error_count: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutSession {
    editor: LayoutEditor,
    tool: LayoutTool,
    active_layer: String,
    active_net: String,
    active_via_rule: String,
    selection: LayoutSelection,
    drag: Option<LayoutDrag>,
    rectangle_start: Option<LayoutPoint>,
    polygon_points: Vec<LayoutPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LayoutDrag {
    pub(super) bbox: LayoutRect,
    pub(super) pointer_start: LayoutPoint,
    pub(super) pointer_current: LayoutPoint,
}

impl LayoutSession {
    pub fn new(
        document: LayoutDocument,
        active_layer: impl Into<String>,
        active_net: impl Into<String>,
        active_via_rule: impl Into<String>,
    ) -> Result<Self, String> {
        let active_layer = active_layer.into();
        let active_net = active_net.into();
        let active_via_rule = active_via_rule.into();
        if active_layer.trim().is_empty() || active_net.trim().is_empty() {
            return Err("layout session requires an active layer and net".into());
        }
        if document.layer_rule(&active_layer).is_none() {
            return Err(format!(
                "active layout layer '{}' has no rule",
                active_layer
            ));
        }
        if !active_via_rule.trim().is_empty() && document.via_rule(&active_via_rule).is_none() {
            return Err(format!("active via rule '{}' is unknown", active_via_rule));
        }
        Ok(Self {
            editor: LayoutEditor::new(document)?,
            tool: LayoutTool::Select,
            active_layer,
            active_net,
            active_via_rule,
            selection: LayoutSelection::None,
            drag: None,
            rectangle_start: None,
            polygon_points: Vec::new(),
        })
    }

    pub fn document(&self) -> &LayoutDocument {
        self.editor.document()
    }

    pub fn tool(&self) -> LayoutTool {
        self.tool
    }

    pub fn selection(&self) -> &LayoutSelection {
        &self.selection
    }

    pub fn properties(&self) -> BTreeMap<String, String> {
        let mut properties = BTreeMap::new();
        properties.insert(
            "tool".into(),
            match self.tool {
                LayoutTool::Select => "select".into(),
                LayoutTool::Rectangle => "rectangle".into(),
                LayoutTool::Polygon => "polygon".into(),
                LayoutTool::AnyAnglePolygon => "any-angle polygon".into(),
                LayoutTool::Via => "via".into(),
            },
        );
        properties.insert("activeLayer".into(), self.active_layer.clone());
        properties.insert("activeNet".into(), self.active_net.clone());
        if !self.active_via_rule.is_empty() {
            properties.insert("activeViaRule".into(), self.active_via_rule.clone());
        }
        match &self.selection {
            LayoutSelection::None => {
                properties.insert("selection".into(), "none".into());
            }
            LayoutSelection::Shape { id } => {
                properties.insert("selection".into(), "shape".into());
                properties.insert("id".into(), id.clone());
                if let Some(shape) = self
                    .document()
                    .shapes()
                    .iter()
                    .find(|shape| &shape.id == id)
                {
                    properties.insert("layer".into(), shape.layer.clone());
                    properties.insert("net".into(), shape.net.clone());
                    properties.insert(
                        "geometry".into(),
                        match &shape.geometry {
                            LayoutGeometry::Rect(_) => "rectangle".into(),
                            LayoutGeometry::Polygon(_) => "polygon".into(),
                            LayoutGeometry::AnyAngle(_) => "any-angle polygon".into(),
                        },
                    );
                    let bbox = shape.geometry.bbox();
                    properties.insert(
                        "bbox".into(),
                        format!("({}, {}) .. ({}, {})", bbox.x1, bbox.y1, bbox.x2, bbox.y2),
                    );
                    properties.insert(
                        "area".into(),
                        shape
                            .geometry
                            .area2()
                            .map(|area2| format!("{}", area2 as f64 / 2.0))
                            .unwrap_or_else(|error| format!("invalid: {error}")),
                    );
                }
            }
            LayoutSelection::ShapeVertex { id, vertex } => {
                properties.insert("selection".into(), "shape-vertex".into());
                properties.insert("id".into(), id.clone());
                properties.insert("vertex".into(), vertex.to_string());
                if let Some(geometry) = self
                    .document()
                    .shapes()
                    .iter()
                    .find(|shape| &shape.id == id)
                    .map(|shape| shape.geometry.clone())
                {
                    let points = match &geometry {
                        LayoutGeometry::Polygon(polygon) => Some(polygon.points.clone()),
                        LayoutGeometry::AnyAngle(polygon) => Some(polygon.points().to_vec()),
                        _ => None,
                    };
                    if let Some(points) = points {
                        if let Some(point) = points.get(*vertex) {
                            properties
                                .insert("point".into(), format!("({}, {})", point.x, point.y));
                        }
                        properties.insert("vertices".into(), points.len().to_string());
                    }
                }
            }
            LayoutSelection::Via { id } => {
                properties.insert("selection".into(), "via".into());
                properties.insert("id".into(), id.clone());
                if let Some(via) = self.document().vias().iter().find(|via| &via.id == id) {
                    properties.insert("rule".into(), via.rule.clone());
                    properties.insert("net".into(), via.net.clone());
                    properties.insert("cutLayer".into(), via.cut_layer.clone());
                    properties.insert("bottomLayer".into(), via.bottom_layer.clone());
                    properties.insert("topLayer".into(), via.top_layer.clone());
                    properties.insert(
                        "cut".into(),
                        format!(
                            "({}, {}) .. ({}, {})",
                            via.cut.x1, via.cut.y1, via.cut.x2, via.cut.y2
                        ),
                    );
                }
            }
        }
        properties
    }

    pub fn render_svg(&self) -> Result<String, String> {
        self.document().write_svg()
    }

    pub fn process(&mut self, event: LayoutEvent) -> Result<LayoutEventOutcome, String> {
        match event {
            LayoutEvent::SetTool { tool } => {
                self.cancel_pending();
                let changed = self.tool != tool;
                self.tool = tool;
                if changed {
                    Ok(LayoutEventOutcome::ToolChanged { tool })
                } else {
                    Ok(LayoutEventOutcome::Ignored)
                }
            }
            LayoutEvent::PointerDown { point } => self.pointer_down(point),
            LayoutEvent::PointerMove { point } => self.pointer_move(point),
            LayoutEvent::PointerUp { point } => self.pointer_up(point),
            LayoutEvent::FinishPolygon => self.finish_polygon(),
            LayoutEvent::Undo => {
                self.cancel_pending();
                let changed = self.editor.undo()?;
                if changed {
                    self.selection = LayoutSelection::None;
                }
                Ok(LayoutEventOutcome::UndoApplied { changed })
            }
            LayoutEvent::Redo => {
                self.cancel_pending();
                let changed = self.editor.redo()?;
                if changed {
                    self.selection = LayoutSelection::None;
                }
                Ok(LayoutEventOutcome::RedoApplied { changed })
            }
            LayoutEvent::Delete => self.delete_selection(),
            LayoutEvent::CheckDrc => {
                let result = self.document().check_layout_drc();
                Ok(LayoutEventOutcome::DrcChecked {
                    clean: result.is_clean(),
                    error_count: result.error_count(),
                })
            }
        }
    }

    fn cancel_pending(&mut self) {
        self.drag = None;
        self.rectangle_start = None;
        self.polygon_points.clear();
    }

    fn cancel_transient(&mut self) {
        self.drag = None;
        self.rectangle_start = None;
    }

    fn pointer_down(&mut self, point: LayoutPoint) -> Result<LayoutEventOutcome, String> {
        self.cancel_transient();
        match self.tool {
            LayoutTool::Rectangle => {
                self.rectangle_start = Some(point);
                Ok(LayoutEventOutcome::RectanglePreviewChanged {
                    start: point,
                    end: point,
                })
            }
            LayoutTool::Via => Ok(LayoutEventOutcome::Ignored),
            LayoutTool::Polygon => {
                let next = polygon_next_point(&self.polygon_points, point);
                if self.polygon_points.last() == Some(&next) {
                    return Ok(LayoutEventOutcome::PolygonPreviewChanged {
                        points: self.polygon_points.clone(),
                    });
                }
                self.polygon_points.push(next);
                Ok(LayoutEventOutcome::PolygonPreviewChanged {
                    points: self.polygon_points.clone(),
                })
            }
            LayoutTool::AnyAnglePolygon => {
                if self.polygon_points.last() == Some(&point) {
                    return Ok(LayoutEventOutcome::PolygonPreviewChanged {
                        points: self.polygon_points.clone(),
                    });
                }
                self.polygon_points.push(point);
                Ok(LayoutEventOutcome::PolygonPreviewChanged {
                    points: self.polygon_points.clone(),
                })
            }
            LayoutTool::Select => {
                let selection = self
                    .document()
                    .hit_layout(point)
                    .unwrap_or(LayoutSelection::None);
                let changed = selection != self.selection;
                if let LayoutSelection::Shape { id } | LayoutSelection::ShapeVertex { id, .. } =
                    &selection
                {
                    let shape = self
                        .document()
                        .shapes()
                        .iter()
                        .find(|shape| &shape.id == id)
                        .ok_or_else(|| format!("selected shape '{}' disappeared", id))?;
                    self.drag = Some(LayoutDrag {
                        bbox: shape.geometry.bbox(),
                        pointer_start: point,
                        pointer_current: point,
                    });
                }
                self.selection = selection;
                if changed {
                    Ok(LayoutEventOutcome::SelectionChanged)
                } else {
                    Ok(LayoutEventOutcome::Ignored)
                }
            }
        }
    }

    fn pointer_move(&mut self, point: LayoutPoint) -> Result<LayoutEventOutcome, String> {
        if let Some(start) = self.rectangle_start {
            return Ok(LayoutEventOutcome::RectanglePreviewChanged { start, end: point });
        }
        let Some(drag) = self.drag.as_mut() else {
            return Ok(LayoutEventOutcome::Ignored);
        };
        drag.pointer_current = point;
        match &self.selection {
            LayoutSelection::Shape { id } | LayoutSelection::ShapeVertex { id, .. } => {
                Ok(LayoutEventOutcome::Dragging {
                    id: id.clone(),
                    bbox: dragged_bbox(*drag),
                })
            }
            _ => Ok(LayoutEventOutcome::Ignored),
        }
    }

    fn pointer_up(&mut self, point: LayoutPoint) -> Result<LayoutEventOutcome, String> {
        match self.tool {
            LayoutTool::Rectangle => {
                let Some(start) = self.rectangle_start.take() else {
                    return Ok(LayoutEventOutcome::Ignored);
                };
                if start == point {
                    return Ok(LayoutEventOutcome::Ignored);
                }
                let id = self.unique_id("shape");
                self.editor.apply(LayoutEdit::AddShape(LayoutShape {
                    id: id.clone(),
                    layer: self.active_layer.clone(),
                    net: self.active_net.clone(),
                    geometry: LayoutGeometry::Rect(LayoutRect::new(
                        start.x, start.y, point.x, point.y,
                    )?),
                }))?;
                self.selection = LayoutSelection::Shape { id: id.clone() };
                Ok(LayoutEventOutcome::ShapeAdded { id })
            }
            LayoutTool::Via => {
                if self.active_via_rule.trim().is_empty() {
                    return Err("via tool requires an active via rule".into());
                }
                let rule = self
                    .document()
                    .via_rule(&self.active_via_rule)
                    .ok_or_else(|| format!("unknown active via rule '{}'", self.active_via_rule))?;
                let x1 = point.x - rule.cut_width / 2;
                let x2 = x1 + rule.cut_width;
                let y1 = point.y - rule.cut_height / 2;
                let y2 = y1 + rule.cut_height;
                let id = self.unique_id("via");
                self.editor.apply(LayoutEdit::AddVia(LayoutVia {
                    id: id.clone(),
                    rule: rule.via.clone(),
                    net: self.active_net.clone(),
                    cut_layer: rule.cut_layer.clone(),
                    bottom_layer: rule.bottom_layer.clone(),
                    top_layer: rule.top_layer.clone(),
                    cut: LayoutRect::new(x1, y1, x2, y2)?,
                }))?;
                self.selection = LayoutSelection::Via { id: id.clone() };
                Ok(LayoutEventOutcome::ViaAdded { id })
            }
            LayoutTool::Polygon | LayoutTool::AnyAnglePolygon => Ok(LayoutEventOutcome::Ignored),
            LayoutTool::Select => {
                let Some(drag) = self.drag.take() else {
                    return Ok(LayoutEventOutcome::Ignored);
                };
                let mut drag = drag;
                drag.pointer_current = point;
                let target = dragged_bbox(drag);
                if let LayoutSelection::Shape { id } = &self.selection {
                    let id = id.clone();
                    let geometry = self
                        .document()
                        .shapes()
                        .iter()
                        .find(|shape| shape.id == id)
                        .ok_or_else(|| format!("dragged shape '{}' disappeared", id))?
                        .geometry
                        .clone();
                    let current = geometry.bbox();
                    let dx = target.x1 - current.x1;
                    let dy = target.y1 - current.y1;
                    self.editor.apply(LayoutEdit::MoveShape {
                        id: id.clone(),
                        dx,
                        dy,
                    })?;
                    Ok(LayoutEventOutcome::ShapeMoved { id })
                } else if let LayoutSelection::ShapeVertex { id, vertex } = &self.selection {
                    let id = id.clone();
                    let vertex = *vertex;
                    let geometry = self
                        .document()
                        .shapes()
                        .iter()
                        .find(|shape| shape.id == id)
                        .ok_or_else(|| format!("dragged shape '{}' disappeared", id))?
                        .geometry
                        .clone();
                    let mut reported_point = point;
                    let geometry = match geometry {
                        LayoutGeometry::Polygon(mut polygon) => {
                            let previous = polygon.points
                                [(vertex + polygon.points.len() - 1) % polygon.points.len()];
                            let next_index = (vertex + 1) % polygon.points.len();
                            let constrained = if vertex % 2 == 1 {
                                polygon.points[next_index].x = point.x;
                                LayoutPoint::new(point.x, previous.y)
                            } else {
                                polygon.points[next_index].y = point.y;
                                LayoutPoint::new(previous.x, point.y)
                            };
                            polygon.points[vertex] = constrained;
                            reported_point = constrained;
                            LayoutGeometry::Polygon(LayoutPolygon::new(polygon.points)?)
                        }
                        LayoutGeometry::AnyAngle(polygon) => {
                            let mut points = polygon.points().to_vec();
                            if vertex >= points.len() {
                                return Err("selected any-angle vertex is out of range".into());
                            }
                            points[vertex] = point;
                            LayoutGeometry::AnyAngle(AnyAnglePolygon::new(points)?)
                        }
                        _ => {
                            return Err("selected vertex does not belong to a polygon".into());
                        }
                    };
                    self.editor.apply(LayoutEdit::SetShapeGeometry {
                        id: id.clone(),
                        geometry,
                    })?;
                    Ok(LayoutEventOutcome::ShapeVertexMoved {
                        id,
                        vertex,
                        point: reported_point,
                    })
                } else {
                    Ok(LayoutEventOutcome::Ignored)
                }
            }
        }
    }

    fn finish_polygon_any_angle(&mut self) -> Result<LayoutEventOutcome, String> {
        if self.tool != LayoutTool::AnyAnglePolygon {
            return Ok(LayoutEventOutcome::Ignored);
        }
        let points = std::mem::take(&mut self.polygon_points);
        let polygon = AnyAnglePolygon::new(points)?;
        let id = self.unique_id("shape");
        self.editor.apply(LayoutEdit::AddShape(LayoutShape {
            id: id.clone(),
            layer: self.active_layer.clone(),
            net: self.active_net.clone(),
            geometry: LayoutGeometry::AnyAngle(polygon),
        }))?;
        self.selection = LayoutSelection::Shape { id: id.clone() };
        Ok(LayoutEventOutcome::ShapeAdded { id })
    }

    fn finish_polygon(&mut self) -> Result<LayoutEventOutcome, String> {
        if self.tool == LayoutTool::AnyAnglePolygon {
            return self.finish_polygon_any_angle();
        }
        if self.tool != LayoutTool::Polygon {
            return Ok(LayoutEventOutcome::Ignored);
        }
        let points = std::mem::take(&mut self.polygon_points);
        let polygon = LayoutPolygon::new(points)?;
        let id = self.unique_id("shape");
        self.editor.apply(LayoutEdit::AddShape(LayoutShape {
            id: id.clone(),
            layer: self.active_layer.clone(),
            net: self.active_net.clone(),
            geometry: LayoutGeometry::Polygon(polygon),
        }))?;
        self.selection = LayoutSelection::Shape { id: id.clone() };
        Ok(LayoutEventOutcome::ShapeAdded { id })
    }
    fn delete_selection(&mut self) -> Result<LayoutEventOutcome, String> {
        self.cancel_pending();
        let selection = self.selection.clone();
        match selection {
            LayoutSelection::Shape { ref id } => {
                self.editor
                    .apply(LayoutEdit::RemoveShape { id: id.clone() })?;
            }
            LayoutSelection::Via { ref id } => {
                self.editor
                    .apply(LayoutEdit::RemoveVia { id: id.clone() })?;
            }
            LayoutSelection::ShapeVertex { .. } => return Ok(LayoutEventOutcome::Ignored),
            LayoutSelection::None => return Ok(LayoutEventOutcome::Ignored),
        }
        self.selection = LayoutSelection::None;
        Ok(LayoutEventOutcome::Deleted { selection })
    }

    fn unique_id(&self, prefix: &str) -> String {
        let mut index = 1_usize;
        loop {
            let candidate = format!("{prefix}{index}");
            let exists = self
                .document()
                .shapes()
                .iter()
                .any(|shape| shape.id == candidate)
                || self.document().vias().iter().any(|via| via.id == candidate);
            if !exists {
                return candidate;
            }
            index += 1;
        }
    }
}
