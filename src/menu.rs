//! Native macOS menu-bar integration. winit builds the app menu but offers no API to extend
//! it, so MacAi inserts its own "Settings…" item (Command+,) through AppKit.

use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static SETTINGS_REQUESTED: AtomicBool = AtomicBool::new(false);
static CONTEXT: OnceLock<egui::Context> = OnceLock::new();

/// Records a request to open Settings and wakes the UI so it is handled on the next frame.
pub fn request_settings() {
    SETTINGS_REQUESTED.store(true, Ordering::SeqCst);
    if let Some(ctx) = CONTEXT.get() {
        ctx.request_repaint();
    }
}

/// Returns true once for each Settings request.
pub fn take_settings_request() -> bool {
    SETTINGS_REQUESTED.swap(false, Ordering::SeqCst)
}

/// Adds "Settings…" below "About MacAi" in the app menu.
pub fn install(ctx: &egui::Context) {
    let _ = CONTEXT.set(ctx.clone());
    #[cfg(target_os = "macos")]
    native::install();
}

/// Development screenshots: choose "Settings…" through the real app menu.
#[cfg(feature = "screenshot")]
pub fn choose_settings_for_screenshot() {
    #[cfg(target_os = "macos")]
    native::choose_settings();
}

#[cfg(target_os = "macos")]
mod native {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSMenuItem};
    use objc2_foundation::NSString;

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "MacAiSettingsMenuTarget"]
        struct SettingsTarget;

        impl SettingsTarget {
            #[unsafe(method(openSettings:))]
            fn open_settings(&self, _sender: Option<&AnyObject>) {
                super::request_settings();
            }
        }
    );

    impl SettingsTarget {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = mtm.alloc::<Self>().set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    pub fn install() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let Some(app_menu) = app
            .mainMenu()
            .and_then(|bar| bar.itemAtIndex(0))
            .and_then(|item| item.submenu())
        else {
            eprintln!("MacAi: the app menu is missing, so Settings was not added to it.");
            return;
        };
        let title = NSString::from_str("Settings…");
        if app_menu.itemWithTitle(&title).is_some() {
            return;
        }
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &title,
                Some(sel!(openSettings:)),
                &NSString::from_str(","),
            )
        };
        let target = SettingsTarget::new(mtm);
        unsafe { item.setTarget(Some(&target)) };
        // The menu item holds its target weakly; keep the target alive for the app's lifetime.
        std::mem::forget(target);
        let index = if app_menu.numberOfItems() > 0 { 1 } else { 0 };
        app_menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), index);
        app_menu.insertItem_atIndex(&item, index + 1);
    }

    #[cfg(feature = "screenshot")]
    pub fn choose_settings() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(app_menu) = NSApplication::sharedApplication(mtm)
            .mainMenu()
            .and_then(|bar| bar.itemAtIndex(0))
            .and_then(|item| item.submenu())
        else {
            return;
        };
        let titles: Vec<String> = (0..app_menu.numberOfItems())
            .filter_map(|index| app_menu.itemAtIndex(index))
            .map(|item| format!("{} [{}]", item.title(), item.keyEquivalent()))
            .collect();
        eprintln!("MacAi app menu: {titles:?}");
        let index = app_menu.indexOfItemWithTitle(&NSString::from_str("Settings…"));
        if index >= 0 {
            app_menu.performActionForItemAtIndex(index);
        }
    }
}
