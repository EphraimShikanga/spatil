use std::path::{Path, PathBuf};

use super::TrayPixmap;

const WANTED: u16 = 32;
const MAX_EDGE: u32 = 256;

pub fn resolve(name: &str, theme_path: Option<&str>) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }

    let direct = Path::new(name);
    if direct.is_absolute() && direct.is_file() {
        return Some(direct.to_path_buf());
    }

    if let Some(dir) = theme_path
        && let Some(found) = search_dir(Path::new(dir), name)
    {
        return Some(found);
    }

    for theme in theme_candidates() {
        if let Some(found) = freedesktop_icons::lookup(name)
            .with_size(WANTED)
            .with_theme(&theme)
            .with_cache()
            .find()
        {
            return Some(found);
        }
    }
    freedesktop_icons::lookup(name).with_size(WANTED).with_cache().find()
}

fn theme_candidates() -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |name: String| {
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    };
    if let Ok(theme) = std::env::var("SPATIL_ICON_THEME") {
        push(theme);
    }
    if let Some(theme) = gtk_icon_theme() {
        push(theme);
    }
    for fallback in ["Adwaita", "breeze", "Papirus", "hicolor"] {
        push(fallback.to_string());
    }
    out
}

fn gtk_icon_theme() -> Option<String> {
    let base = dirs::config_dir()?;
    for version in ["gtk-4.0", "gtk-3.0"] {
        let Ok(text) = std::fs::read_to_string(base.join(version).join("settings.ini")) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("gtk-icon-theme-name") {
                let value = value.trim_start_matches([' ', '=']).trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn search_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    for ext in ["png", "svg", "xpm"] {
        let direct = dir.join(format!("{name}.{ext}"));
        if direct.is_file() {
            return Some(direct);
        }
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir()
            && let Some(found) = search_dir(&path, name)
        {
            return Some(found);
        }
    }
    None
}

pub fn decode(path: &Path) -> Option<TrayPixmap> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => decode_png(path),
        "svg" | "svgz" => render_svg(path),
        _ => {
            tracing::debug!("no decoder for {}", path.display());
            None
        }
    }
}

pub fn is_symbolic(path: &Path) -> bool {
    path.file_stem()
        .is_some_and(|s| s.to_string_lossy().ends_with("-symbolic"))
}

pub fn tint(pixmap: &mut TrayPixmap, rgb: [u8; 3]) {
    for px in pixmap.bgra.chunks_exact_mut(4) {
        px[0] = rgb[2];
        px[1] = rgb[1];
        px[2] = rgb[0];
    }
}

fn render_svg(path: &Path) -> Option<TrayPixmap> {
    let data = std::fs::read(path).ok()?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default())
        .map_err(|err| tracing::debug!("unparseable svg {}: {err}", path.display()))
        .ok()?;

    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let edge = WANTED as f32;
    let scale = (edge / w.max(h)).min(8.0);
    let (out_w, out_h) = (
        ((w * scale).round() as u32).clamp(1, MAX_EDGE),
        ((h * scale).round() as u32).clamp(1, MAX_EDGE),
    );

    let mut pixmap = resvg::tiny_skia::Pixmap::new(out_w, out_h)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let mut bgra = Vec::with_capacity((out_w * out_h) as usize * 4);
    for px in pixmap.data().chunks_exact(4) {
        bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    Some(TrayPixmap { width: out_w, height: out_h, bgra })
}

fn decode_png(path: &Path) -> Option<TrayPixmap> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;

    let info = reader.info();
    let (width, height) = (info.width, info.height);
    if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
        tracing::debug!("skipping {}: {width}x{height} is out of range", path.display());
        return None;
    }
    let color = info.color_type;

    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buf).ok()?;
    let data = &buf[..frame.buffer_size()];

    let channels = match color {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        png::ColorType::Indexed => return None,
    };
    let expected = (width as usize) * (height as usize) * channels;
    if data.len() < expected {
        return None;
    }

    let mut bgra = Vec::with_capacity((width * height) as usize * 4);
    for px in data[..expected].chunks_exact(channels) {
        let (r, g, b, a) = match channels {
            4 => (px[0], px[1], px[2], px[3]),
            3 => (px[0], px[1], px[2], 0xFF),
            2 => (px[0], px[0], px[0], px[1]),
            _ => (px[0], px[0], px[0], 0xFF),
        };
        bgra.extend_from_slice(&[b, g, r, a]);
    }

    Some(TrayPixmap { width, height, bgra })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_chain_always_ends_somewhere_useful() {
        let themes = theme_candidates();
        assert!(themes.contains(&"hicolor".to_string()), "{themes:?}");
        assert!(themes.contains(&"Adwaita".to_string()), "{themes:?}");
        let unique = themes.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), themes.len(), "no theme is tried twice: {themes:?}");
    }

    #[test]
    fn a_stock_icon_name_resolves_against_an_installed_theme() {
        let Some(found) = resolve("network-wireless", None) else {
            eprintln!("no icon theme installed here; skipping");
            return;
        };
        assert!(found.is_file(), "{found:?}");
        assert!(decode(&found).is_some(), "the resolved icon must decode: {found:?}");
    }

    #[test]
    fn an_empty_name_resolves_to_nothing() {
        assert!(resolve("", None).is_none());
    }

    #[test]
    fn a_nonexistent_absolute_path_is_not_returned() {
        assert!(resolve("/definitely/not/here/icon.png", None).is_none());
    }

    #[test]
    fn a_written_png_round_trips_into_bgra() {
        let dir = std::env::temp_dir().join("spatil-icon-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("probe.png");

        let file = std::fs::File::create(&path).expect("create");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 2, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        writer
            .write_image_data(&[
                0xFF, 0x00, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF,
                0xFF, 0x80,
            ])
            .expect("write");
        drop(writer);

        let decoded = decode(&path).expect("decode");
        assert_eq!((decoded.width, decoded.height), (2, 2));
        assert_eq!(decoded.bgra.len(), 16);
        assert_eq!(&decoded.bgra[0..4], &[0x00, 0x00, 0xFF, 0xFF]);
        assert_eq!(&decoded.bgra[12..16], &[0xFF, 0xFF, 0xFF, 0x80]);

        let found = resolve(path.to_str().expect("utf8"), None).expect("absolute path");
        assert_eq!(found, path);
        let by_name = resolve("probe", dir.to_str()).expect("theme path search");
        assert_eq!(by_name, path);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_svg_is_rasterised_to_premultiplied_bgra() {
        let dir = std::env::temp_dir().join("spatil-icon-test-svg");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("square.svg");
        std::fs::write(
            &path,
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">
                 <rect width="16" height="16" fill="#0000ff"/></svg>"##,
        )
        .expect("write");

        let decoded = decode(&path).expect("render");
        assert!(decoded.width > 0 && decoded.height > 0);
        assert_eq!(decoded.bgra.len(), (decoded.width * decoded.height) as usize * 4);
        assert_eq!(&decoded.bgra[0..4], &[0xFF, 0x00, 0x00, 0xFF], "blue, as BGRA");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_svg_is_refused_rather_than_panicking() {
        let dir = std::env::temp_dir().join("spatil-icon-test-bad");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("broken.svg");
        std::fs::write(&path, b"not markup at all").expect("write");
        assert!(decode(&path).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn symbolic_icons_are_recognised_and_recoloured() {
        assert!(is_symbolic(Path::new("/x/network-wireless-symbolic.svg")));
        assert!(!is_symbolic(Path::new("/x/network-wireless.png")));

        let mut pixmap = TrayPixmap {
            width: 1,
            height: 1,
            bgra: vec![0x00, 0x00, 0x00, 0x99],
        };
        tint(&mut pixmap, [0x4A, 0x90, 0xE2]);
        assert_eq!(pixmap.bgra, vec![0xE2, 0x90, 0x4A, 0x99], "alpha survives the tint");
    }
}

