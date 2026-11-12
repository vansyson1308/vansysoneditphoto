use std::sync::Arc;

use magicpic_color::{Color, PixelFormat, SampleType};
use magicpic_compose::shape_split;
use magicpic_doc::Fill;
use magicpic_doc::vector::{Path, ShapeLayer, ShapeStroke, Subpath};
use magicpic_geom::{Rect, TileCoord};
use magicpic_raster::Surface;

/// Source pixels retained to identify a split must count toward its cache budget too.
#[test]
fn oversized_source_pixels_are_not_pinned_by_the_split_cache() {
    shape_split::purge_cache();
    let mut source = Surface::new(PixelFormat::RGBA8);
    // Shared tiles represent 256 MiB of source pixels without a large test allocation.
    source.fill_rect(Rect::new(0, 0, 8192, 8192), &[1.0, 1.0, 1.0, 1.0]);
    let source_tile = Arc::downgrade(source.tile(TileCoord::new(0, 0)).expect("source tile"));
    let shape = ShapeLayer {
        path: Path::new(vec![Subpath::polygon(&[(10.0, 10.0), (30.0, 10.0), (30.0, 30.0), (10.0, 30.0)])]),
        fill: Some(Fill::Solid(Color::rgb(0.2, 0.4, 0.6))),
        stroke: Some(ShapeStroke { width: 2.0, ..ShapeStroke::default() }),
        cache: Some(source),
        ..ShapeLayer::default()
    };
    let parts = shape_split::split(&shape, Rect::new(0, 0, 64, 64), SampleType::U8).expect("stroked shape");
    drop(parts);
    drop(shape);
    assert!(source_tile.upgrade().is_none(), "the source plus split parts exceed the cache budget");
    shape_split::purge_cache();
}
