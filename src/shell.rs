pub mod module;
pub mod network;
pub mod orb;
pub mod tray;

use tray::TrayPixmap;

pub const SYMBOLIC_TINT: [u8; 3] = [0xDC, 0xE3, 0xEC];

pub fn icon_source(name: &str) -> Option<TrayPixmap> {
    let path = tray::icon::resolve(name, None)?;
    let mut pixmap = tray::icon::decode(&path)?;
    if tray::icon::is_symbolic(&path) {
        tray::icon::tint(&mut pixmap, SYMBOLIC_TINT);
    }
    Some(pixmap)
}
