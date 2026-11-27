//! The design's own icons (`dev/familiar-design/source/Main.dc.html`), rasterised with resvg.

use crate::shell::tray::TrayPixmap;

/// Destinations: 19px, stroke `#F2F4F8` 1.8.
pub const ALERTS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="#F2F4F8" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15L6 16z"/><path d="M10 20a2 2 0 0 0 4 0"/></svg>"##;
pub const CALENDAR: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="#F2F4F8" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="5" width="17" height="15" rx="2.5"/><path d="M3.5 10h17M8 3v4M16 3v4"/></svg>"##;
pub const WINDOWS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="#F2F4F8" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="4" width="7" height="7" rx="1.5"/><rect x="13" y="4" width="7" height="7" rx="1.5"/><rect x="4" y="13" width="7" height="7" rx="1.5"/><rect x="13" y="13" width="7" height="7" rx="1.5"/></svg>"##;
pub const MAP: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="#F2F4F8" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7.5L9 5l6 2.5L21 5v11.5L15 19l-6-2.5L3 19V7.5z"/><path d="M9 5v11.5M15 7.5V19"/></svg>"##;

/// Tile chips: 16px, stroke `#F2F4F8`.
pub const WIFI: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#F2F4F8" stroke-width="2.2" stroke-linecap="round"><path d="M2 8.5a15 15 0 0 1 20 0M5 12a10 10 0 0 1 14 0M8.5 15.5a5 5 0 0 1 7 0"/><circle cx="12" cy="19" r="1" fill="#F2F4F8"/></svg>"##;
pub const BLUETOOTH: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#F2F4F8" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M7 7l10 10-5 4V3l5 4L7 17"/></svg>"##;
pub const BATTERY: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#F2F4F8" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="7" width="16" height="10" rx="2"/><path d="M21 11v2"/></svg>"##;
/// The volume row's speaker, drawn in a tile chip the way the other tiles are.
pub const SOUND: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#F2F4F8" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 5L6 9H3v6h3l5 4V5z"/><path d="M15.5 8.5a5 5 0 0 1 0 7"/></svg>"##;

pub const DEST_SIZE: f32 = 19.0;
pub const TILE_SIZE: f32 = 16.0;

/// Rasterises at `scale` times the icon's own size, into the BGRA the HUD quads take.
pub fn rasterize(svg: &str, scale: f32) -> Option<TrayPixmap> {
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default())
        .map_err(|err| tracing::warn!("design icon does not parse: {err}"))
        .ok()?;
    let size = tree.size();
    let (w, h) = ((size.width() * scale).round() as u32, (size.height() * scale).round() as u32);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1))?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for px in pixmap.data().chunks_exact(4) {
        bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    Some(TrayPixmap { width: w.max(1), height: h.max(1), bgra })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_design_icon_rasterises_at_its_own_size() {
        for (svg, size) in [
            (ALERTS, 19), (CALENDAR, 19), (WINDOWS, 19), (MAP, 19),
            (WIFI, 16), (BLUETOOTH, 16), (BATTERY, 16), (SOUND, 16),
        ] {
            let icon = rasterize(svg, 1.0).expect("parses");
            assert_eq!((icon.width, icon.height), (size, size));
            assert!(icon.bgra.chunks(4).any(|px| px[3] > 0), "draws something");
        }
    }
}
