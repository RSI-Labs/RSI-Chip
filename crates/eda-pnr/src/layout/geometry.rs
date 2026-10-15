//! Shared layout geometry, overlap, segment, SVG-escape, and pointer helpers.

use super::editor::LayoutDrag;
use super::*;

pub(super) fn dragged_bbox(drag: LayoutDrag) -> LayoutRect {
    LayoutRect::new(
        drag.bbox.x1 + drag.pointer_current.x - drag.pointer_start.x,
        drag.bbox.y1 + drag.pointer_current.y - drag.pointer_start.y,
        drag.bbox.x2 + drag.pointer_current.x - drag.pointer_start.x,
        drag.bbox.y2 + drag.pointer_current.y - drag.pointer_start.y,
    )
    .unwrap_or(drag.bbox)
}

pub(super) fn polygon_next_point(points: &[LayoutPoint], click: LayoutPoint) -> LayoutPoint {
    let Some(previous) = points.last() else {
        return click;
    };
    if points.len() % 2 == 1 {
        LayoutPoint::new(click.x, previous.y)
    } else {
        LayoutPoint::new(previous.x, click.y)
    }
}

pub(super) fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(super) fn bbox_intersects(a: LayoutRect, b: LayoutRect) -> bool {
    a.x1 <= b.x2 && b.x1 <= a.x2 && a.y1 <= b.y2 && b.y1 <= a.y2
}

pub(super) fn orientation(a: LayoutPoint, b: LayoutPoint, c: LayoutPoint) -> i64 {
    (b.x - a.x).saturating_mul(c.y - b.y) - (b.y - a.y).saturating_mul(c.x - b.x)
}

pub(super) fn on_segment(a: LayoutPoint, b: LayoutPoint, point: LayoutPoint) -> bool {
    point.x >= a.x.min(b.x)
        && point.x <= a.x.max(b.x)
        && point.y >= a.y.min(b.y)
        && point.y <= a.y.max(b.y)
}

pub(super) fn segments_intersect(
    a: LayoutPoint,
    b: LayoutPoint,
    c: LayoutPoint,
    d: LayoutPoint,
) -> bool {
    let abc = orientation(a, b, c);
    let abd = orientation(a, b, d);
    let cda = orientation(c, d, a);
    let cdb = orientation(c, d, b);
    (abc.signum() != abd.signum() && cda.signum() != cdb.signum())
        || (abc == 0 && on_segment(a, b, c))
        || (abd == 0 && on_segment(a, b, d))
        || (cda == 0 && on_segment(c, d, a))
        || (cdb == 0 && on_segment(c, d, b))
}

pub(super) fn segments_properly_cross(
    a: LayoutPoint,
    b: LayoutPoint,
    c: LayoutPoint,
    d: LayoutPoint,
) -> bool {
    let abc = orientation(a, b, c);
    let abd = orientation(a, b, d);
    let cda = orientation(c, d, a);
    let cdb = orientation(c, d, b);
    abc.signum() * abd.signum() < 0 && cda.signum() * cdb.signum() < 0
}

pub(super) fn positive_area_overlap(
    left: &LayoutGeometry,
    right: &LayoutGeometry,
) -> Result<bool, String> {
    if !matches!(left, LayoutGeometry::AnyAngle(_)) && !matches!(right, LayoutGeometry::AnyAngle(_))
    {
        return manhattan_boolean(left, right, ManhattanBooleanOp::Intersection)
            .map(|result| result.area > 0);
    }
    let left_polygon = left.try_into_any_angle()?;
    let right_polygon = right.try_into_any_angle()?;
    if !left_polygon.positive_area_intersects(&right_polygon) {
        return Ok(false);
    }
    any_angle_boolean(
        &left_polygon,
        &right_polygon,
        AnyAngleBooleanOperation::Intersection,
    )
    .map(|result| result.total_area > 0.0)
}
