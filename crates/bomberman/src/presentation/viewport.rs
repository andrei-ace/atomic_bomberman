//! Aspect-preserving presentation of the original 640 by 480 canvas.
//!
//! Coordinates use the same window units as mouse input. Decorative margins
//! belong to presentation only; they never enlarge the playable board.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasViewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

impl CanvasViewport {
    pub fn fit(window_width: f32, window_height: f32) -> Self {
        let width = extent(window_width);
        let height = extent(window_height);
        let scale = (width / 640.).min(height / 480.);
        let scale_f32 = scale as f32;
        if scale_f32 <= 0. {
            return Self {
                x: (width * 0.5) as f32,
                y: (height * 0.5) as f32,
                width: 0.,
                height: 0.,
                scale: 0.,
            };
        }
        let canvas_width = 640. * scale;
        let canvas_height = 480. * scale;
        Self {
            x: ((width - canvas_width) * 0.5) as f32,
            y: ((height - canvas_height) * 0.5) as f32,
            width: canvas_width as f32,
            height: canvas_height as f32,
            scale: scale_f32,
        }
    }

    /// Map window input back to the original UI. Outside points remain outside
    /// rather than being clamped onto a clickable canvas edge.
    pub fn to_canvas(self, x: f32, y: f32) -> (f32, f32) {
        if !self.scale.is_finite()
            || self.scale <= 0.
            || !self.x.is_finite()
            || !self.y.is_finite()
            || !x.is_finite()
            || !y.is_finite()
        {
            return (0., 0.);
        }
        (
            finite_coordinate((f64::from(x) - f64::from(self.x)) / f64::from(self.scale)),
            finite_coordinate((f64::from(y) - f64::from(self.y)) / f64::from(self.scale)),
        )
    }

    /// The right and bottom edges are excluded, matching ordinary pixel bounds.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.scale.is_finite()
            && self.scale > 0.
            && self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.
            && self.height > 0.
            && x.is_finite()
            && y.is_finite()
            && x >= self.x
            && y >= self.y
            && f64::from(x) < f64::from(self.x) + f64::from(self.width)
            && f64::from(y) < f64::from(self.y) + f64::from(self.height)
    }

    /// Return left, right, top and bottom decorative rectangles. They stay
    /// within the window and never overlap; empty rectangles are omitted.
    pub fn margins(&self, window_width: f32, window_height: f32) -> [Option<[f32; 4]>; 4] {
        let width = extent(window_width);
        let height = extent(window_height);
        if width == 0.
            || height == 0.
            || ![self.x, self.y, self.width, self.height]
                .iter()
                .all(|value| value.is_finite())
            || self.width < 0.
            || self.height < 0.
        {
            return [None; 4];
        }
        let left = f64::from(self.x).clamp(0., width);
        let right = (f64::from(self.x) + f64::from(self.width)).clamp(left, width);
        let top = f64::from(self.y).clamp(0., height);
        let bottom = (f64::from(self.y) + f64::from(self.height)).clamp(top, height);
        [
            rectangle(0., 0., left, height),
            rectangle(right, 0., width - right, height),
            rectangle(left, 0., right - left, top),
            rectangle(left, bottom, right - left, height - bottom),
        ]
    }
}

/// Cover the whole window uniformly with `source`, then return the source
/// subsection corresponding to one decorative destination rectangle. Drawing
/// those subsections avoids both interior overdraw and GPU scissor ambiguity.
///
/// `destination` must lie inside the window. Empty, nonfinite or invalid input
/// returns an empty source rectangle, so minimized windows cannot produce NaN.
pub fn backdrop_source(source: [f32; 4], window: [f32; 2], destination: [f32; 4]) -> [f32; 4] {
    let width = extent(window[0]);
    let height = extent(window[1]);
    if width == 0.
        || height == 0.
        || !source
            .iter()
            .chain(&destination)
            .all(|value| value.is_finite())
        || source[2] <= 0.
        || source[3] <= 0.
        || destination[2] <= 0.
        || destination[3] <= 0.
        || destination[0] < 0.
        || destination[1] < 0.
    {
        return [0.; 4];
    }
    let [sx, sy, sw, sh] = source.map(f64::from);
    let [dx, dy, dw, dh] = destination.map(f64::from);
    // Margin edges have already passed through f32. Permit their rounding
    // error, while rejecting destinations that actually leave the window.
    let tolerance = width.max(height).max(1.) * f64::from(f32::EPSILON) * 4.;
    if dx + dw > width + tolerance || dy + dh > height + tolerance {
        return [0.; 4];
    }
    let scale = (width / sw).max(height / sh);
    let crop_width = (width / scale).min(sw);
    let crop_height = (height / scale).min(sh);
    let crop_x = sx + (sw - crop_width) * 0.5;
    let crop_y = sy + (sh - crop_height) * 0.5;
    let x = (crop_x + dx / scale).clamp(sx, sx + sw);
    let y = (crop_y + dy / scale).clamp(sy, sy + sh);
    let right = (crop_x + (dx + dw).min(width) / scale).clamp(x, sx + sw);
    let bottom = (crop_y + (dy + dh).min(height) / scale).clamp(y, sy + sh);
    let result = [x, y, right - x, bottom - y].map(|value| value as f32);
    if result.iter().all(|value| value.is_finite()) && result[2] > 0. && result[3] > 0. {
        result
    } else {
        [0.; 4]
    }
}

fn extent(value: f32) -> f64 {
    if value.is_finite() && value > 0. {
        f64::from(value)
    } else {
        0.
    }
}

fn finite_coordinate(value: f64) -> f32 {
    value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
}

fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Option<[f32; 4]> {
    let result = [x, y, width, height].map(|value| value as f32);
    (result[2] > 0. && result[3] > 0.).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.0002, "{actual} != {expected}");
    }

    fn close_rect(actual: [f32; 4], expected: [f32; 4]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            close(actual, expected);
        }
    }

    #[test]
    fn widescreen_fits_the_complete_original_canvas() {
        let viewport = CanvasViewport::fit(1920., 1080.);
        assert_eq!(
            viewport,
            CanvasViewport {
                x: 240.,
                y: 0.,
                width: 1440.,
                height: 1080.,
                scale: 2.25,
            }
        );
        assert_eq!(
            viewport.margins(1920., 1080.),
            [
                Some([0., 0., 240., 1080.]),
                Some([1680., 0., 240., 1080.]),
                None,
                None,
            ]
        );
    }

    #[test]
    fn original_aspect_has_no_decorative_margins() {
        for (width, height) in [(640., 480.), (1280., 960.), (1920., 1440.)] {
            let viewport = CanvasViewport::fit(width, height);
            assert_eq!(viewport.x, 0.);
            assert_eq!(viewport.y, 0.);
            assert_eq!(viewport.margins(width, height), [None; 4]);
        }
    }

    #[test]
    fn ultrawide_and_portrait_fill_only_the_unused_area() {
        let wide = CanvasViewport::fit(2520., 1080.);
        assert_eq!(wide.x, 540.);
        assert_eq!(wide.width, 1440.);
        assert_eq!(wide.margins(2520., 1080.)[0], Some([0., 0., 540., 1080.]));
        let portrait = CanvasViewport::fit(1080., 1920.);
        assert_eq!(portrait.x, 0.);
        assert_eq!(portrait.y, 555.);
        assert_eq!(portrait.width, 1080.);
        assert_eq!(portrait.height, 810.);
        assert_eq!(
            portrait.margins(1080., 1920.),
            [
                None,
                None,
                Some([0., 0., 1080., 555.]),
                Some([0., 1365., 1080., 555.]),
            ]
        );
    }

    #[test]
    fn input_roundtrips_and_margin_clicks_stay_outside() {
        for (width, height) in [(1920., 1080.), (1080., 1920.), (1365.5, 767.25)] {
            let viewport = CanvasViewport::fit(width, height);
            for (x, y) in [(0., 0.), (55., 133.), (320., 240.), (639., 479.)] {
                let wx = viewport.x + x * viewport.scale;
                let wy = viewport.y + y * viewport.scale;
                assert!(viewport.contains(wx, wy));
                let (actual_x, actual_y) = viewport.to_canvas(wx, wy);
                close(actual_x, x);
                close(actual_y, y);
            }
            assert!(!viewport.contains(viewport.x + viewport.width, viewport.y));
            assert!(!viewport.contains(viewport.x, viewport.y + viewport.height));
            if viewport.x > 0. {
                assert!(!viewport.contains(viewport.x - 1., viewport.y));
                assert!(viewport.to_canvas(viewport.x - 1., viewport.y).0 < 0.);
            }
            if viewport.y > 0. {
                assert!(!viewport.contains(viewport.x, viewport.y - 1.));
                assert!(viewport.to_canvas(viewport.x, viewport.y - 1.).1 < 0.);
            }
        }
    }

    #[test]
    fn side_crops_share_one_cover_mapping_without_stretching() {
        let source = [0., 48., 640., 432.];
        let viewport = CanvasViewport::fit(1920., 1080.);
        let [left, right, _, _] = viewport.margins(1920., 1080.);
        close_rect(
            backdrop_source(source, [1920., 1080.], left.unwrap()),
            [0., 84., 80., 360.],
        );
        close_rect(
            backdrop_source(source, [1920., 1080.], right.unwrap()),
            [560., 84., 80., 360.],
        );
    }

    #[test]
    fn fractional_and_portrait_crops_stay_inside_the_source() {
        let source = [12.5, 48.25, 637.75, 431.5];
        for window in [[1365.5, 767.25], [1080., 1920.], [2520., 1080.], [1., 0.75]] {
            let viewport = CanvasViewport::fit(window[0], window[1]);
            for destination in viewport.margins(window[0], window[1]).into_iter().flatten() {
                let crop = backdrop_source(source, window, destination);
                assert!(crop[2] > 0. && crop[3] > 0.);
                assert!(crop[0] >= source[0] && crop[1] >= source[1]);
                assert!(crop[0] + crop[2] <= source[0] + source[2] + 0.0001);
                assert!(crop[1] + crop[3] <= source[1] + source[3] + 0.0001);
                close(destination[2] / crop[2], destination[3] / crop[3]);
            }
        }
    }

    #[test]
    fn margins_do_not_overlap_even_when_both_axes_have_padding() {
        let viewport = CanvasViewport {
            x: 10.,
            y: 20.,
            width: 80.,
            height: 60.,
            scale: 0.125,
        };
        let margins = viewport.margins(100., 100.);
        assert_eq!(
            margins,
            [
                Some([0., 0., 10., 100.]),
                Some([90., 0., 10., 100.]),
                Some([10., 0., 80., 20.]),
                Some([10., 80., 80., 20.]),
            ]
        );
        let margin_area: f32 = margins.into_iter().flatten().map(|r| r[2] * r[3]).sum();
        assert_eq!(margin_area + viewport.width * viewport.height, 10_000.);
    }

    #[test]
    fn minimized_and_invalid_dimensions_are_finite_and_empty() {
        for window in [
            [0., 0.],
            [0., 1080.],
            [1920., 0.],
            [-1., 480.],
            [f32::NAN, 480.],
            [640., f32::INFINITY],
        ] {
            let viewport = CanvasViewport::fit(window[0], window[1]);
            assert!([
                viewport.x,
                viewport.y,
                viewport.width,
                viewport.height,
                viewport.scale
            ]
            .iter()
            .all(|v| v.is_finite()));
            assert_eq!(viewport.scale, 0.);
            assert!(!viewport.contains(0., 0.));
            assert_eq!(viewport.to_canvas(0., 0.), (0., 0.));
            assert_eq!(viewport.margins(window[0], window[1]), [None; 4]);
            assert_eq!(
                backdrop_source([0., 48., 640., 432.], window, [0., 0., 1., 1.]),
                [0.; 4]
            );
        }
    }

    #[test]
    fn invalid_crop_requests_never_produce_nonfinite_source_coordinates() {
        let source = [0., 48., 640., 432.];
        for destination in [
            [0., 0., 0., 1.],
            [-1., 0., 1., 1.],
            [0., 0., 1921., 1080.],
            [f32::NAN, 0., 10., 10.],
        ] {
            assert_eq!(
                backdrop_source(source, [1920., 1080.], destination),
                [0.; 4]
            );
        }
        for source in [
            [0., 0., 0., 1.],
            [0., 0., 1., -1.],
            [0., 0., f32::INFINITY, 1.],
        ] {
            assert_eq!(
                backdrop_source(source, [1920., 1080.], [0., 0., 240., 1080.]),
                [0.; 4]
            );
        }
    }
}
