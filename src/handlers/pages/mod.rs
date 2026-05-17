pub mod api;
pub mod assets;
pub mod auth;
pub mod base;

use crate::{assets as embedded_assets, dev};

pub fn shell() -> String {
    if dev::is_dev() {
        dev::shell(dev::vite_port())
    } else {
        embedded_assets::index_html()
    }
}
