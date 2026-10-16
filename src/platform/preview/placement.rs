//! Preview geometry uses top-left screen coordinates, including negative display origins.

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PreviewRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl PreviewRect {
    /// Reject missing or invalid native geometry before it reaches window placement.
    pub(crate) fn new(x: f64, y: f64, width: f64, height: f64) -> Option<Self> {
        ([x, y, width, height, x + width, y + height]
            .iter()
            .all(|v| v.is_finite())
            && width >= 0.0
            && height > 0.0)
            .then_some(Self {
                x,
                y,
                width,
                height,
            })
    }
}

/// Prefer below the caret, flip above near the bottom, and keep the bubble in the work area.
pub(crate) fn preview_origin(
    anchor: PreviewRect,
    size: (f64, f64),
    work: PreviewRect,
) -> (f64, f64) {
    let (width, height) = size;
    let min_x = work.x + 8.0;
    let min_y = work.y + 8.0;
    let max_x = (work.x + work.width - width - 8.0).max(min_x);
    let max_y = (work.y + work.height - height - 8.0).max(min_y);
    let below = anchor.y + anchor.height + 6.0;
    let above = anchor.y - height - 6.0;
    let y = if below <= max_y {
        below
    } else if above >= min_y {
        above
    } else {
        below
    };
    (anchor.x.clamp(min_x, max_x), y.clamp(min_y, max_y))
}

#[cfg(test)]
mod tests {
    use super::{PreviewRect, preview_origin};

    #[test]
    fn places_below_caret_and_avoids_bottom_and_right_edges() {
        // Near the bottom-right corner a growing candidate must remain visible
        // and leave the original insertion line unobscured.
        let work = PreviewRect::new(0.0, 0.0, 1920.0, 1080.0).unwrap();
        let caret = PreviewRect::new(100.0, 200.0, 1.0, 18.0).unwrap();
        assert_eq!(preview_origin(caret, (200.0, 60.0), work), (100.0, 224.0));
        let caret = PreviewRect::new(1910.0, 1050.0, 1.0, 18.0).unwrap();
        assert_eq!(preview_origin(caret, (420.0, 220.0), work), (1492.0, 824.0));
    }

    #[test]
    fn placement_supports_displays_left_of_and_above_the_primary() {
        // Auxiliary displays can have negative coordinates on either axis.
        let work = PreviewRect::new(-1280.0, -200.0, 1280.0, 1024.0).unwrap();
        let caret = PreviewRect::new(-30.0, -100.0, 0.0, 18.0).unwrap();
        assert_eq!(preview_origin(caret, (300.0, 80.0), work), (-308.0, -76.0));
    }

    #[test]
    fn invalid_native_bounds_cannot_enter_placement() {
        assert!(PreviewRect::new(f64::NAN, 0.0, 1.0, 18.0).is_none());
        assert!(PreviewRect::new(0.0, f64::INFINITY, 1.0, 18.0).is_none());
        assert!(PreviewRect::new(0.0, 0.0, -1.0, 18.0).is_none());
        assert!(PreviewRect::new(0.0, 0.0, 0.0, 0.0).is_none());
    }
}
