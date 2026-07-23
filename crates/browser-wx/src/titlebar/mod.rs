//! Custom title bar support — wxDragon has no equivalent feature on either
//! platform (wxWidgets' abstraction layer doesn't expose GTK's
//! header-bar-as-titlebar concept, nor Windows' non-client hit-testing, on
//! any OS), so this reaches past it into raw platform APIs. The two
//! platforms' mechanisms are fundamentally different — GTK wholesale
//! replaces the title bar with an app-drawn one; Windows keeps the existing
//! toolbar row and instead makes the *window* treat part of it as the
//! caption — so there's no single shared implementation here, just a
//! `windows`/`linux` submodule each, gated to the platform they apply to.

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "linux")]
pub mod linux;

/// The address bar's value, abstracted over its platform-specific widget
/// type: a real `gtk::Entry` on Linux (part of the raw GTK header bar built
/// in `titlebar::linux`, entirely separate from wxWidgets' own widget tree),
/// a wx `TextCtrl` everywhere else (the existing toolbar row, unchanged).
pub trait AddressBarValue {
    fn set_address_value(&self, value: &str);
}

#[cfg(target_os = "linux")]
pub type AddressBarHandle = gtk::Entry;
#[cfg(not(target_os = "linux"))]
pub type AddressBarHandle = wxdragon::widgets::textctrl::TextCtrl;

#[cfg(target_os = "linux")]
impl AddressBarValue for gtk::Entry {
    fn set_address_value(&self, value: &str) {
        use gtk::prelude::EntryExt;
        self.set_text(value);
    }
}

#[cfg(not(target_os = "linux"))]
impl AddressBarValue for wxdragon::widgets::textctrl::TextCtrl {
    fn set_address_value(&self, value: &str) {
        self.set_value(value);
    }
}
