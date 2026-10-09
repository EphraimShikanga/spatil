use std::cell::RefCell;
use std::sync::{Mutex, OnceLock};

use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent, Wrap,
};

pub const MAX_LABEL_CHARS: usize = 256;
const ELLIPSIS: char = '…';
const LINE_HEIGHT: f32 = 1.25;

static FONT_SYSTEM: OnceLock<Mutex<FontSystem>> = OnceLock::new();

thread_local! {
    static SWASH_CACHE: RefCell<SwashCache> = RefCell::new(SwashCache::new());
}

#[derive(Debug, Clone, Copy)]
pub struct PlacedGlyph {
    pub key: CacheKey,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Default)]
pub struct ShapedText {
    pub glyphs: Vec<PlacedGlyph>,
    pub width: f32,
    pub ascent: f32,
}

pub struct GlyphImage {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub color: bool,
    pub rgba: Vec<u8>,
}

/// Starts the system font scan on a background thread. Idempotent.
pub fn warm_fonts(on_loaded: impl FnOnce() + Send + 'static) {
    if FONT_SYSTEM.get().is_some() {
        on_loaded();
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("spatil-font-warm".into())
        .spawn(move || {
            let _ = FONT_SYSTEM.set(Mutex::new(FontSystem::new()));
            on_loaded();
        });
    if let Err(err) = spawned {
        tracing::warn!("failed to spawn font-warm thread: {err}");
    }
}

pub fn fonts_ready() -> bool {
    FONT_SYSTEM.get().is_some()
}

pub fn fonts_available() -> bool {
    with_font_system(|fs| fs.db().faces().next().is_some()).unwrap_or(false)
}

pub fn line_height(size: f32) -> f32 {
    size * LINE_HEIGHT
}

/// Flattens an externally-supplied string to one printable line of bounded length.
pub fn sanitize(input: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for ch in input.chars() {
        if out.chars().count() >= MAX_LABEL_CHARS {
            break;
        }
        if ch.is_whitespace() || ch == '\u{2028}' || ch == '\u{2029}' {
            pending_space = !out.is_empty();
            continue;
        }
        if ch.is_control() {
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    out
}

pub fn measure(text: &str, font: &str, size: f32) -> f32 {
    if text.is_empty() || !fonts_available() {
        return 0.0;
    }
    with_font_system(|fs| {
        shape_line(fs, text, font, size)
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0_f32, f32::max)
    })
    .unwrap_or(0.0)
}

/// Tail-truncates with an ellipsis to fit `max_width`; empty when not even that fits.
pub fn fit(text: &str, font: &str, size: f32, max_width: f32) -> (String, f32) {
    if !fonts_available() {
        return (String::new(), 0.0);
    }
    let full = measure(text, font, size);
    if full <= max_width {
        return (text.to_string(), full);
    }
    let ellipsis = ELLIPSIS.to_string();
    if measure(&ellipsis, font, size) > max_width {
        return (String::new(), 0.0);
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        let candidate: String = chars[..mid].iter().collect::<String>() + &ellipsis;
        if measure(&candidate, font, size) <= max_width {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let fitted: String = chars[..lo].iter().collect::<String>() + &ellipsis;
    let width = measure(&fitted, font, size);
    (fitted, width)
}

/// Glyph positions relative to a pen at the origin, baseline at `ascent`.
pub fn shape(text: &str, font: &str, size: f32) -> ShapedText {
    if text.is_empty() || !fonts_available() {
        return ShapedText::default();
    }
    with_font_system(|fs| {
        let mut buffer = shape_line(fs, text, font, size);
        let (ascent, _descent) = buffer
            .line_layout(fs, 0)
            .and_then(|lines| lines.first())
            .map(|line| (line.max_ascent, line.max_descent))
            .unwrap_or((size * 0.8, size * 0.2));
        let mut glyphs = Vec::new();
        let mut width = 0.0_f32;
        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            for glyph in run.glyphs {
                let physical = glyph.physical((0.0, 0.0), 1.0);
                glyphs.push(PlacedGlyph {
                    key: physical.cache_key,
                    x: physical.x,
                    y: physical.y,
                });
            }
        }
        ShapedText {
            glyphs,
            width,
            ascent,
        }
    })
    .unwrap_or_default()
}

/// Rasterizes one glyph to straight-alpha RGBA. `color` marks an emoji bitmap.
pub fn rasterize(key: CacheKey) -> Option<GlyphImage> {
    with_font_system(|fs| {
        SWASH_CACHE.with_borrow_mut(|cache| {
            let image = cache.get_image_uncached(fs, key)?;
            let (w, h) = (image.placement.width, image.placement.height);
            if w == 0 || h == 0 {
                return None;
            }
            let pixels = (w * h) as usize;
            let mut rgba = vec![0u8; pixels * 4];
            match image.content {
                SwashContent::Mask => {
                    if image.data.len() < pixels {
                        return None;
                    }
                    for i in 0..pixels {
                        rgba[i * 4] = 255;
                        rgba[i * 4 + 1] = 255;
                        rgba[i * 4 + 2] = 255;
                        rgba[i * 4 + 3] = image.data[i];
                    }
                }
                SwashContent::Color => {
                    if image.data.len() < pixels * 4 {
                        return None;
                    }
                    rgba.copy_from_slice(&image.data[..pixels * 4]);
                }
                SwashContent::SubpixelMask => {
                    if image.data.len() < pixels * 4 {
                        return None;
                    }
                    for i in 0..pixels {
                        let rgb = &image.data[i * 4..i * 4 + 3];
                        let coverage =
                            ((rgb[0] as u16 + rgb[1] as u16 + rgb[2] as u16) / 3) as u8;
                        rgba[i * 4] = 255;
                        rgba[i * 4 + 1] = 255;
                        rgba[i * 4 + 2] = 255;
                        rgba[i * 4 + 3] = coverage;
                    }
                }
            }
            Some(GlyphImage {
                width: w,
                height: h,
                left: image.placement.left,
                top: image.placement.top,
                color: matches!(image.content, SwashContent::Color),
                rgba,
            })
        })
    })
    .flatten()
}

pub fn probe(font: &str, size: f32) {
    let started = std::time::Instant::now();
    let _ = FONT_SYSTEM.set(Mutex::new(FontSystem::new()));
    let scan = started.elapsed();
    let faces = with_font_system(|fs| fs.db().len()).unwrap_or(0);
    println!("font scan: {scan:?} for {faces} faces");
    if !fonts_available() {
        println!("no fonts installed; every text surface will render empty");
        return;
    }

    for sample in [
        "WiFi (70%)",
        "Battery 63% (charging)",
        "日本語のテキスト",
        "العربية",
        "emoji 🔋📶",
    ] {
        let shaped = shape(sample, font, size);
        let colored = shaped
            .glyphs
            .iter()
            .filter(|g| rasterize(g.key).is_some_and(|i| i.color))
            .count();
        println!(
            "{sample:>24} -> {:>3} glyphs, {:>6.1}px wide, ascent {:.1}, {colored} colour",
            shaped.glyphs.len(),
            shaped.width,
            shaped.ascent,
        );
    }

    let long = "A network name far too long to fit on the arc beside the orb";
    let budget = measure(long, font, size) / 3.0;
    let (fitted, width) = fit(long, font, size, budget);
    println!("ellipsised to {width:.1}px (budget {budget:.1}): {fitted:?}");
}

fn with_font_system<R>(f: impl FnOnce(&mut FontSystem) -> R) -> Option<R> {
    let mut fs = FONT_SYSTEM.get()?.lock().ok()?;
    Some(f(&mut fs))
}

fn family_of(name: &str) -> Family<'_> {
    match name {
        "monospace" => Family::Monospace,
        "sans-serif" | "" => Family::SansSerif,
        "serif" => Family::Serif,
        "cursive" => Family::Cursive,
        "fantasy" => Family::Fantasy,
        _ => Family::Name(name),
    }
}

fn shape_line(fs: &mut FontSystem, text: &str, font: &str, size: f32) -> Buffer {
    let size = size.max(1.0);
    let mut buffer = Buffer::new(fs, Metrics::new(size, line_height(size)));
    buffer.set_wrap(fs, Wrap::None);
    buffer.set_size(fs, None, Some(size * 4.0));
    let attrs = Attrs::new().family(family_of(font));
    buffer.set_text(fs, text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(fs, false);
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    const FONT: &str = "sans-serif";
    const SIZE: f32 = 13.0;

    fn ensure_fonts() {
        let _ = FONT_SYSTEM.set(Mutex::new(FontSystem::new()));
    }

    #[test]
    fn sanitize_strips_control_characters_and_collapses_whitespace() {
        assert_eq!(sanitize("Wi\u{0000}Fi"), "WiFi");
        assert_eq!(sanitize("a\nb\tc"), "a b c");
        assert_eq!(sanitize("  padded  "), "padded");
        assert_eq!(sanitize(""), "");
    }

    #[test]
    fn sanitize_clamps_length() {
        let long = "x".repeat(MAX_LABEL_CHARS * 4);
        assert_eq!(sanitize(&long).chars().count(), MAX_LABEL_CHARS);
    }

    #[test]
    fn sanitize_keeps_non_latin_text() {
        assert_eq!(sanitize("日本語"), "日本語");
        assert_eq!(sanitize("العربية"), "العربية");
    }

    #[test]
    fn measure_empty_is_zero_without_touching_fonts() {
        assert_eq!(measure("", FONT, SIZE), 0.0);
    }

    #[test]
    fn measure_grows_with_text_length() {
        ensure_fonts();
        if !fonts_available() {
            return;
        }
        let short = measure("i", FONT, SIZE);
        let long = measure("iiiiiiiiii", FONT, SIZE);
        assert!(short > 0.0);
        assert!(long > short);
    }

    #[test]
    fn fit_returns_the_whole_string_when_it_fits() {
        ensure_fonts();
        if !fonts_available() {
            return;
        }
        let (text, width) = fit("Wi-Fi", FONT, SIZE, 10_000.0);
        assert_eq!(text, "Wi-Fi");
        assert!(width > 0.0);
    }

    #[test]
    fn fit_ellipsizes_and_respects_the_budget() {
        ensure_fonts();
        if !fonts_available() {
            return;
        }
        let long = "A network name far too long for the arc to show in full";
        let budget = measure(long, FONT, SIZE) / 2.0;
        let (text, width) = fit(long, FONT, SIZE, budget);
        assert!(text.ends_with(ELLIPSIS));
        assert!(text.chars().count() < long.chars().count());
        assert!(width <= budget);
    }

    #[test]
    fn fit_yields_nothing_when_even_an_ellipsis_overflows() {
        ensure_fonts();
        if !fonts_available() {
            return;
        }
        assert_eq!(fit("Battery", FONT, SIZE, 0.5), (String::new(), 0.0));
    }

    #[test]
    fn shape_places_one_glyph_per_character_left_to_right() {
        ensure_fonts();
        if !fonts_available() {
            return;
        }
        let shaped = shape("abc", FONT, SIZE);
        assert_eq!(shaped.glyphs.len(), 3);
        assert!(shaped.width > 0.0);
        assert!(shaped.ascent > 0.0);
        let xs: Vec<i32> = shaped.glyphs.iter().map(|g| g.x).collect();
        assert!(xs.windows(2).all(|w| w[0] <= w[1]), "{xs:?}");
    }

    #[test]
    fn shaped_glyphs_rasterize_to_straight_alpha_rgba() {
        ensure_fonts();
        if !fonts_available() {
            return;
        }
        let shaped = shape("H", FONT, SIZE);
        let glyph = shaped.glyphs.first().expect("one glyph");
        let Some(image) = rasterize(glyph.key) else {
            return;
        };
        assert_eq!(image.rgba.len(), (image.width * image.height * 4) as usize);
        assert!(image.rgba.chunks(4).any(|px| px[3] > 0));
    }

    #[test]
    fn everything_degrades_to_empty_without_a_font_system() {
        assert_eq!(measure("", FONT, SIZE), 0.0);
        assert!(shape("", FONT, SIZE).glyphs.is_empty());
    }
}
