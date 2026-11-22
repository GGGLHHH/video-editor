//! 裁剪窗口里的框（剪映「裁剪」）：框用素材像素坐标；拖角、拖边改大小，拖中间挪位置；
//! 选了比例时按比例改（拖角以对角为锚，拖边以对边为锚、另一个方向围着中心）。

use ve_core::Crop;

/// 按住的是框的哪一部分。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Handle {
    /// 框里面：整个挪。
    Move,
    /// 左边。
    Left,
    /// 右边。
    Right,
    /// 上边。
    Top,
    /// 下边。
    Bottom,
    /// 左上角。
    TopLeft,
    /// 右上角。
    TopRight,
    /// 左下角。
    BottomLeft,
    /// 右下角。
    BottomRight,
}

impl Handle {
    /// 动的是哪几条边：左、右、上、下。
    const fn sides(self) -> (bool, bool, bool, bool) {
        match self {
            Self::Move => (false, false, false, false),
            Self::Left => (true, false, false, false),
            Self::Right => (false, true, false, false),
            Self::Top => (false, false, true, false),
            Self::Bottom => (false, false, false, true),
            Self::TopLeft => (true, false, true, false),
            Self::TopRight => (false, true, true, false),
            Self::BottomLeft => (true, false, false, true),
            Self::BottomRight => (false, true, false, true),
        }
    }
}

/// 框（素材像素，`x0 < x1`、`y0 < y1`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    /// 左。
    pub x0: f32,
    /// 上。
    pub y0: f32,
    /// 右。
    pub x1: f32,
    /// 下。
    pub y1: f32,
}

/// 框最小占素材的比例（每个方向）。
pub(crate) const MIN_FRACTION: f32 = 0.05;

impl Rect {
    /// 宽。
    #[must_use]
    pub(crate) fn width(self) -> f32 {
        self.x1 - self.x0
    }

    /// 高。
    #[must_use]
    pub(crate) fn height(self) -> f32 {
        self.y1 - self.y0
    }

    /// 裁剪值换成框。
    #[must_use]
    pub(crate) fn from_crop(c: Crop, (w, h): (f32, f32)) -> Self {
        Self {
            x0: c.left * w,
            y0: c.top * h,
            x1: (1.0 - c.right) * w,
            y1: (1.0 - c.bottom) * h,
        }
    }

    /// 框换成裁剪值（四边各自夹到 0–1）。
    #[must_use]
    pub(crate) fn to_crop(self, (w, h): (f32, f32)) -> Crop {
        let f = |v: f32| v.clamp(0.0, 1.0);
        Crop {
            left: f(self.x0 / w),
            top: f(self.y0 / h),
            right: f(1.0 - self.x1 / w),
            bottom: f(1.0 - self.y1 / h),
        }
    }
}

/// 点 `p` 按住的是框的哪里（`slop` 是角和边能点中的宽度）；框外面 `None`。
#[must_use]
pub(crate) fn hit(rect: Rect, (px, py): (f32, f32), slop: f32) -> Option<Handle> {
    let near = |v: f32, edge: f32| (v - edge).abs() <= slop;
    let in_x = px >= rect.x0 - slop && px <= rect.x1 + slop;
    let in_y = py >= rect.y0 - slop && py <= rect.y1 + slop;
    if !(in_x && in_y) {
        return None;
    }
    let sides = (
        near(px, rect.x0),
        near(px, rect.x1),
        near(py, rect.y0),
        near(py, rect.y1),
    );
    Some(match sides {
        (true, _, true, _) => Handle::TopLeft,
        (_, true, true, _) => Handle::TopRight,
        (true, _, _, true) => Handle::BottomLeft,
        (_, true, _, true) => Handle::BottomRight,
        (true, ..) => Handle::Left,
        (_, true, ..) => Handle::Right,
        (_, _, true, _) => Handle::Top,
        (.., true) => Handle::Bottom,
        _ => Handle::Move,
    })
}

/// 从 `start` 按住 `handle` 拖了 `(dx, dy)`（素材像素）后的框；`size` 是素材大小，
/// `ratio` 是锁定的宽高比（像素宽 ÷ 像素高）。结果总在素材里、不小于 [`MIN_FRACTION`]。
#[must_use]
pub(crate) fn drag(
    start: Rect,
    handle: Handle,
    (dx, dy): (f32, f32),
    size: (f32, f32),
    ratio: Option<f32>,
) -> Rect {
    let (w, h) = size;
    let (min_w, min_h) = (w * MIN_FRACTION, h * MIN_FRACTION);
    if handle == Handle::Move {
        let (bw, bh) = (start.width(), start.height());
        let x0 = (start.x0 + dx).clamp(0.0, (w - bw).max(0.0));
        let y0 = (start.y0 + dy).clamp(0.0, (h - bh).max(0.0));
        return Rect {
            x0,
            y0,
            x1: x0 + bw,
            y1: y0 + bh,
        };
    }
    let (left, right, top, bottom) = handle.sides();
    let mut r = start;
    if left {
        r.x0 = (start.x0 + dx).clamp(0.0, start.x1 - min_w);
    }
    if right {
        r.x1 = (start.x1 + dx).clamp(start.x0 + min_w, w);
    }
    if top {
        r.y0 = (start.y0 + dy).clamp(0.0, start.y1 - min_h);
    }
    if bottom {
        r.y1 = (start.y1 + dy).clamp(start.y0 + min_h, h);
    }
    match ratio {
        Some(a) if a > 0.0 => fit_ratio(start, r, handle, a, size),
        _ => r,
    }
}

/// 锁比例：按拖出来的大小取宽，再按比例定高，放不下就整体缩到放得下。
fn fit_ratio(start: Rect, r: Rect, handle: Handle, a: f32, (w, h): (f32, f32)) -> Rect {
    let (left, right, top, bottom) = handle.sides();
    let min_w = (w * MIN_FRACTION).max(h * MIN_FRACTION * a);
    let horizontal = left || right;
    let vertical = top || bottom;
    // 横向的锚：拖左边锚在右边，拖右边锚在左边；只拖上下边时围着中心
    let (cx, cy) = (
        f32::midpoint(start.x0, start.x1),
        f32::midpoint(start.y0, start.y1),
    );
    let room_x = if left {
        start.x1
    } else if right {
        w - start.x0
    } else {
        2.0 * cx.min(w - cx)
    };
    let room_y = if top {
        start.y1
    } else if bottom {
        h - start.y0
    } else {
        2.0 * cy.min(h - cy)
    };
    let wanted = match (horizontal, vertical) {
        (true, true) => r.width().max(r.height() * a),
        (true, false) => r.width(),
        _ => r.height() * a,
    };
    let bw = wanted.max(min_w).min(room_x).min(room_y * a);
    let bh = bw / a;
    let (x0, x1) = if left {
        (start.x1 - bw, start.x1)
    } else if right {
        (start.x0, start.x0 + bw)
    } else {
        (cx - bw / 2.0, cx + bw / 2.0)
    };
    let (y0, y1) = if top {
        (start.y1 - bh, start.y1)
    } else if bottom {
        (start.y0, start.y0 + bh)
    } else {
        (cy - bh / 2.0, cy + bh / 2.0)
    };
    Rect { x0, y0, x1, y1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (f32, f32) = (1920.0, 1080.0);
    const FULL: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 1920.0,
        y1: 1080.0,
    };

    fn close(a: Rect, b: Rect) -> bool {
        [a.x0 - b.x0, a.y0 - b.y0, a.x1 - b.x1, a.y1 - b.y1]
            .iter()
            .all(|d| d.abs() < 0.01)
    }

    #[test]
    fn crop_and_box_convert_both_ways() {
        let c = Crop {
            left: 0.1,
            top: 0.2,
            right: 0.25,
            bottom: 0.05,
        };
        let r = Rect::from_crop(c, SIZE);
        assert!((r.x0 - 192.0).abs() < 1e-3 && (r.y1 - 1026.0).abs() < 1e-3);
        let back = r.to_crop(SIZE);
        assert!((back.left - 0.1).abs() < 1e-6 && (back.right - 0.25).abs() < 1e-6);
        assert!((back.top - 0.2).abs() < 1e-6 && (back.bottom - 0.05).abs() < 1e-6);
    }

    #[test]
    fn corners_edges_and_inside_are_hit_apart() {
        let r = Rect {
            x0: 100.0,
            y0: 100.0,
            x1: 500.0,
            y1: 400.0,
        };
        assert_eq!(hit(r, (102.0, 98.0), 8.0), Some(Handle::TopLeft));
        assert_eq!(hit(r, (505.0, 400.0), 8.0), Some(Handle::BottomRight));
        assert_eq!(hit(r, (300.0, 395.0), 8.0), Some(Handle::Bottom));
        assert_eq!(hit(r, (96.0, 250.0), 8.0), Some(Handle::Left));
        assert_eq!(hit(r, (300.0, 250.0), 8.0), Some(Handle::Move));
        assert_eq!(hit(r, (50.0, 250.0), 8.0), None);
    }

    #[test]
    fn free_drag_moves_only_the_grabbed_sides_and_stays_inside() {
        let r = drag(FULL, Handle::BottomRight, (-480.0, -270.0), SIZE, None);
        assert!(close(
            r,
            Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 1440.0,
                y1: 810.0
            }
        ));
        // 拖过头：夹在素材里、不小于最小尺寸
        let past = drag(FULL, Handle::Left, (5000.0, 0.0), SIZE, None);
        assert!(
            (past.width() - 1920.0 * MIN_FRACTION).abs() < 1e-3,
            "{past:?}"
        );
        let out = drag(r, Handle::TopLeft, (-100.0, -100.0), SIZE, None);
        assert!(out.x0.abs() < 0.01 && out.y0.abs() < 0.01);
    }

    #[test]
    fn moving_keeps_the_size_and_stops_at_the_edges() {
        let r = Rect {
            x0: 100.0,
            y0: 100.0,
            x1: 500.0,
            y1: 400.0,
        };
        let moved = drag(r, Handle::Move, (5000.0, -50.0), SIZE, None);
        assert!(close(
            moved,
            Rect {
                x0: 1520.0,
                y0: 50.0,
                x1: 1920.0,
                y1: 350.0
            }
        ));
    }

    #[test]
    fn a_locked_ratio_holds_while_dragging_corners_and_edges() {
        let square = Rect {
            x0: 420.0,
            y0: 0.0,
            x1: 1500.0,
            y1: 1080.0,
        };
        // 拖右下角往里：以左上角为锚，仍是正方形
        let corner = drag(
            square,
            Handle::BottomRight,
            (-300.0, -100.0),
            SIZE,
            Some(1.0),
        );
        assert!(
            (corner.width() - corner.height()).abs() < 0.01,
            "{corner:?}"
        );
        assert!((corner.x0 - 420.0).abs() < 0.01 && corner.y0.abs() < 0.01);
        assert!(
            (corner.width() - 980.0).abs() < 0.01,
            "取拖得少的那边：{corner:?}"
        );
        // 拖右边：高围着中心跟着变
        let edge = drag(corner, Handle::Right, (-200.0, 0.0), SIZE, Some(1.0));
        assert!((edge.width() - 780.0).abs() < 0.01 && (edge.height() - 780.0).abs() < 0.01);
        let mid = f32::midpoint(corner.y0, corner.y1);
        assert!((f32::midpoint(edge.y0, edge.y1) - mid).abs() < 0.01);
        // 往外拖过头：缩到放得下（高最多 1080）
        let big = drag(
            edge,
            Handle::BottomRight,
            (3000.0, 3000.0),
            SIZE,
            Some(16.0 / 9.0),
        );
        assert!(
            big.x1 <= 1920.0 + 0.01 && big.y1 <= 1080.0 + 0.01,
            "{big:?}"
        );
        assert!((big.width() / big.height() - 16.0 / 9.0).abs() < 1e-3);
    }
}
