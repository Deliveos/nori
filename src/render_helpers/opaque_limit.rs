//! A limit on the opaque regions that the elements of one render report.

use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::Renderer;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Transform};

/// Maximum number of opaque rectangles that the elements of one render report together.
///
/// smithay's damage tracker and DRM compositor put the opaque rectangles of all elements that are
/// on the output and not hidden into one list (#182). On every render, they subtract the rectangles
/// in front of each element from its area to find out whether it is hidden, and the damage tracker
/// subtracts the whole list from the previous render's. How long that takes depends on how the
/// rectangles cut each other. In a standalone measurement of the subtraction in nori's smithay
/// version, per subtraction, for an output of 3840 × 2160:
///
/// - n rectangles crossing each other as 1 px stripes cut an element that covers the output into
///   (n/2+1)² pieces. For each element behind them, that took 10.7–19.5 µs with 32 rectangles,
///   57–103 µs with 64, and 3.5–6.1 ms with 256.
/// - The previous render's list can hold n large overlapping rectangles, such as the opaque regions
///   of n nearly output-sized surfaces offset by 1 px. Subtracting n crossing stripes from them
///   made 9,248 pieces and took 0.40–0.66 ms for 32, 69,531 pieces and 4.3–7.4 ms for 64, and
///   3,991,518 pieces and 1.8–1.9 s for 256.
///
/// Clients decide how long the list gets: one opaque region of `MAX_OPAQUE_REGION_RECTS`
/// rectangles can still make hundreds of them, and a client can make any number of surfaces.
pub const MAX_OPAQUE_RECTS_PER_RENDER: usize = 32;

/// Element that reports its opaque regions only if they fit in [`MAX_OPAQUE_RECTS_PER_RENDER`].
///
/// See [`limit_opaque_regions()`].
#[derive(Debug)]
pub struct OpaqueLimitRenderElement<E> {
    inner: E,
    keep_opaque: bool,
}

/// Wraps `elements` so that together they report at most [`MAX_OPAQUE_RECTS_PER_RENDER`] opaque
/// rectangles at `scale`.
///
/// `elements` go front to back. An element whose opaque rectangles don't fit in what is left of the
/// limit reports none, and the elements after it still keep theirs if they fit.
///
/// Reporting fewer opaque regions doesn't change what the frame shows, as long as the opaque
/// regions are right. But what is hidden only behind the regions left out is drawn anyway and no
/// longer counts as hidden: its surfaces keep their primary scanout output, so they get frame
/// callbacks on every refresh, their idle inhibitors stay active and a window with on-demand VRR
/// keeps it on, and the DRM compositor can't use those regions to skip the elements behind or to
/// pick an element for direct scan-out.
///
/// Every element counts toward the limit, also one that is off the output or hidden, which the
/// damage tracker and the DRM compositor would leave out of their list.
///
/// The wrapped elements are otherwise the same as `elements`. Pass every element list that goes to
/// a damage tracker or a DRM compositor through this, so that they always get the same view of
/// the same elements.
pub fn limit_opaque_regions<E: Element>(
    elements: &[E],
    scale: Scale<f64>,
) -> Vec<OpaqueLimitRenderElement<&E>> {
    limit_opaque_regions_to(elements, scale, MAX_OPAQUE_RECTS_PER_RENDER)
}

fn limit_opaque_regions_to<E: Element>(
    elements: &[E],
    scale: Scale<f64>,
    max_rects: usize,
) -> Vec<OpaqueLimitRenderElement<&E>> {
    let mut left = max_rects;
    elements
        .iter()
        .map(|inner| {
            // Once nothing is left, no element can keep any rectangles, so don't compute them.
            let mut keep_opaque = false;
            if left > 0 {
                let count = inner.opaque_regions(scale).len();
                if count <= left {
                    left -= count;
                    keep_opaque = true;
                }
            }
            OpaqueLimitRenderElement { inner, keep_opaque }
        })
        .collect()
}

impl<E: Element> Element for OpaqueLimitRenderElement<E> {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        if self.keep_opaque {
            self.inner.opaque_regions(scale)
        } else {
            OpaqueRegions::default()
        }
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }

    fn is_framebuffer_effect(&self) -> bool {
        self.inner.is_framebuffer_effect()
    }
}

impl<R: Renderer, E: RenderElement<R>> RenderElement<R> for OpaqueLimitRenderElement<E> {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), R::Error> {
        self.inner
            .draw(frame, src, dst, damage, opaque_regions, cache)
    }

    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        self.inner.underlying_storage(renderer)
    }

    fn capture_framebuffer(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), R::Error> {
        self.inner.capture_framebuffer(frame, src, dst, cache)
    }
}

#[cfg(test)]
mod tests {
    use smithay::utils::Size;

    use super::*;

    /// Element with `rects` opaque rectangles.
    #[derive(Debug)]
    struct TestElement {
        id: Id,
        rects: usize,
    }

    impl Element for TestElement {
        fn id(&self) -> &Id {
            &self.id
        }

        fn current_commit(&self) -> CommitCounter {
            CommitCounter::default()
        }

        fn src(&self) -> Rectangle<f64, Buffer> {
            Rectangle::from_size(Size::from((1., 1.)))
        }

        fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
            Rectangle::from_size(Size::from((100, 100)))
        }

        fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
            (0..self.rects as i32)
                .map(|i| Rectangle::new(Point::from((i % 100, i / 100)), Size::from((1, 1))))
                .collect()
        }
    }

    /// Numbers of opaque rectangles that elements with `rects` rectangles each report under
    /// `max_rects`.
    fn kept(rects: &[usize], max_rects: usize) -> Vec<usize> {
        let elements: Vec<_> = rects
            .iter()
            .map(|&rects| TestElement {
                id: Id::new(),
                rects,
            })
            .collect();
        let scale = Scale::from(1.);
        let limited = limit_opaque_regions_to(&elements, scale, max_rects);
        for (elem, limited) in elements.iter().zip(&limited) {
            assert_eq!(limited.id(), elem.id());
        }
        limited
            .iter()
            .map(|elem| elem.opaque_regions(scale).len())
            .collect()
    }

    #[test]
    fn keeps_opaque_regions_front_to_back_while_they_fit() {
        assert_eq!(kept(&[], 6), [0; 0]);
        assert_eq!(kept(&[1, 2, 3], 6), [1, 2, 3]);
        // The third doesn't fit; the fourth still does, and then nothing is left for the fifth.
        assert_eq!(kept(&[1, 2, 4, 3, 1], 6), [1, 2, 0, 3, 0]);
        // An element over the whole limit.
        assert_eq!(kept(&[7, 6], 6), [0, 6]);
        assert_eq!(kept(&[0, 6, 0], 6), [0, 6, 0]);
        assert_eq!(kept(&[300], MAX_OPAQUE_RECTS_PER_RENDER), [0]);
        assert_eq!(kept(&[30, 14, 2], MAX_OPAQUE_RECTS_PER_RENDER), [30, 0, 2]);
    }
}
