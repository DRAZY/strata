//! macOS: files opened from Finder, the Dock or `open -a Strata file` arrive
//! as an "open documents" Apple Event, which winit doesn't surface. We
//! install our own handler for it and queue the paths for the next frame.
//!
//! AppKit installs its default handlers while the app finishes launching, so
//! ours goes in from the `NSApplicationWillFinishLaunching` notification,
//! which is still before the cold-start event is delivered.
//!
//! The Dock icon's menu (New Window and recent files and folders) comes from
//! `applicationDockMenu:` on the application delegate, which is winit's; the
//! method is added to its class at the same moment (see `add_dock_menu`).

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, NSObject, Sel};
use objc2::{AnyThread, class, define_class, msg_send, sel};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSString};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock};

static QUEUE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CTX: OnceLock<egui::Context> = OnceLock::new();
/// The Dock menu's recent files and folders (see `set_dock_recents`).
static RECENTS: Mutex<(Vec<PathBuf>, Vec<PathBuf>)> = Mutex::new((Vec::new(), Vec::new()));
/// What the Dock menu's items open, by tag - 1, as when it was last shown.
static SHOWN: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// New Window was chosen in the Dock menu.
static NEW_WINDOW: AtomicBool = AtomicBool::new(false);
/// The handler, kept for good: the Dock menu's items target it.
static HANDLER: AtomicPtr<Handler> = AtomicPtr::new(std::ptr::null_mut());
/// Recent files and folders in the Dock menu, each.
const DOCK_RECENTS: usize = 10;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

const fn four(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class has no Drop.
    #[unsafe(super(NSObject))]
    #[name = "StrataOpenDocumentsHandler"]
    struct Handler;

    impl Handler {
        /// A Dock menu item: tag 0 is New Window, the rest index `SHOWN`.
        #[unsafe(method(dockItem:))]
        fn dock_item(&self, sender: &AnyObject) {
            let tag: isize = unsafe { msg_send![sender, tag] };
            if tag == 0 {
                NEW_WINDOW.store(true, Ordering::Relaxed);
            } else if let Some(p) = lock(&SHOWN).get(tag as usize - 1).cloned() {
                lock(&QUEUE).push(p);
            }
            if let Some(ctx) = CTX.get() {
                ctx.request_repaint();
            }
        }

        #[unsafe(method(handleOpen:withReply:))]
        fn handle_open(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            let list: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send![event, paramDescriptorForKeyword: four(b"----")] };
            let Some(list) = list else { return };
            let mut paths = Vec::new();
            let n = list.numberOfItems();
            let items: Vec<Retained<NSAppleEventDescriptor>> = if n > 0 { (1..=n).filter_map(|i| list.descriptorAtIndex(i)).collect() } else { vec![list] };
            for item in items {
                if let Some(path) = item.fileURLValue().and_then(|u| u.path()) {
                    paths.push(PathBuf::from(path.to_string()));
                }
            }
            QUEUE.lock().unwrap_or_else(|e| e.into_inner()).extend(paths);
            if let Some(ctx) = CTX.get() {
                ctx.request_repaint();
            }
        }
    }
);

/// The handler (made once, on the main thread, and never freed).
fn handler() -> &'static Handler {
    let mut p = HANDLER.load(Ordering::Acquire);
    if p.is_null() {
        let h: Retained<Handler> = unsafe { msg_send![Handler::alloc(), init] };
        p = Retained::into_raw(h);
        HANDLER.store(p, Ordering::Release);
    }
    // SAFETY: set above from a retained object that is never released.
    unsafe { &*p }
}

fn install_handler() {
    let handler = handler();
    let manager = NSAppleEventManager::sharedAppleEventManager();
    let _: () = unsafe {
        msg_send![&manager, setEventHandler: handler as &AnyObject, andSelector: sel!(handleOpen:withReply:), forEventClass: four(b"aevt"), andEventID: four(b"odoc")]
    };
    // The application delegate (winit's) is set by now: give it a Dock menu.
    let app: *mut AnyObject = unsafe { msg_send![class!(NSApplication), sharedApplication] };
    let delegate: *mut AnyObject = unsafe { msg_send![app, delegate] };
    // SAFETY: a live object (the delegate) or null.
    match unsafe { delegate.as_ref() } {
        Some(d) if add_dock_menu(d.class()) => {
            // NSApplication notes what its delegate answers when it's set:
            // set it again so the new method counts. (winit keeps it alive.)
            unsafe {
                let _: () = msg_send![app, setDelegate: std::ptr::null_mut::<AnyObject>()];
                let _: () = msg_send![app, setDelegate: delegate];
            }
        }
        Some(_) => log::warn!("the app delegate already has a Dock menu; not adding Strata's"),
        None => log::warn!("no app delegate: no Dock menu"),
    }
}

// ---- Dock menu -------------------------------------------------------------------

/// Give `cls` an `applicationDockMenu:` method (the menu `dock_menu` makes).
/// False if it has one already.
fn add_dock_menu(cls: &AnyClass) -> bool {
    type DockMenu = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> *mut AnyObject;
    // SAFETY: the signature matches the type encoding "@@:@" (object return;
    // self, _cmd, the NSApplication).
    let imp: Imp = unsafe { std::mem::transmute::<DockMenu, Imp>(dock_menu) };
    let cls = cls as *const AnyClass as *mut AnyClass;
    unsafe { objc2::ffi::class_addMethod(cls, sel!(applicationDockMenu:), imp, c"@@:@".as_ptr()) }.as_bool()
}

/// `-[delegate applicationDockMenu:]`: an autoreleased NSMenu.
unsafe extern "C-unwind" fn dock_menu(_this: *mut AnyObject, _cmd: Sel, _app: *mut AnyObject) -> *mut AnyObject {
    let (files, folders) = lock(&RECENTS).clone();
    let (menu, shown) = make_dock_menu(&files, &folders);
    *lock(&SHOWN) = shown;
    Retained::autorelease_return(menu)
}

/// The Dock menu: New Window, then recent files, then recent folders (each
/// titled with its name, the full path in its tooltip). Returns it and what
/// items 1, 2… open.
fn make_dock_menu(files: &[PathBuf], folders: &[PathBuf]) -> (Retained<AnyObject>, Vec<PathBuf>) {
    let menu: Retained<AnyObject> = unsafe { msg_send![class!(NSMenu), new] };
    let target = handler();
    let add = |title: &str, tag: usize, tip: Option<&str>| {
        let item: Retained<AnyObject> = unsafe { msg_send![class!(NSMenuItem), new] };
        unsafe {
            let _: () = msg_send![&item, setTitle: &*NSString::from_str(title)];
            let _: () = msg_send![&item, setTarget: target as &AnyObject];
            let _: () = msg_send![&item, setAction: sel!(dockItem:)];
            let _: () = msg_send![&item, setTag: tag as isize];
            if let Some(tip) = tip {
                let _: () = msg_send![&item, setToolTip: &*NSString::from_str(tip)];
            }
            let _: () = msg_send![&menu, addItem: &*item];
        }
    };
    let separator = || unsafe {
        let sep: Retained<AnyObject> = msg_send![class!(NSMenuItem), separatorItem];
        let _: () = msg_send![&menu, addItem: &*sep];
    };
    add("New Window", 0, None);
    let mut shown = Vec::new();
    for (list, folder) in [(files, false), (folders, true)] {
        if list.is_empty() {
            continue;
        }
        separator();
        for p in list.iter().take(DOCK_RECENTS) {
            let name = p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
            shown.push(p.clone());
            add(&if folder { format!("{name}/") } else { name }, shown.len(), Some(&p.display().to_string()));
        }
    }
    (menu, shown)
}

/// The recent files and folders the Dock menu offers (first ten of each).
pub fn set_dock_recents(files: &[PathBuf], folders: &[PathBuf]) {
    let (files, folders) = (&files[..files.len().min(DOCK_RECENTS)], &folders[..folders.len().min(DOCK_RECENTS)]);
    let mut r = lock(&RECENTS);
    if r.0 != files || r.1 != folders {
        *r = (files.to_vec(), folders.to_vec());
    }
}

/// New Window was chosen in the Dock menu since the last call.
pub fn take_new_window() -> bool {
    NEW_WINDOW.swap(false, Ordering::Relaxed)
}

/// Call on the main thread before the event loop starts.
pub fn install() {
    let block = block2::RcBlock::new(|_: NonNull<NSNotification>| install_handler());
    let name = NSString::from_str("NSApplicationWillFinishLaunchingNotification");
    let token = unsafe { NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block) };
    std::mem::forget(token);
}

/// Repaint when files arrive while the app is idle.
pub fn set_context(ctx: &egui::Context) {
    let _ = CTX.set(ctx.clone());
}

/// Paths opened since the last call.
pub fn take() -> Vec<PathBuf> {
    std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(menu: &AnyObject) -> Vec<String> {
        let n: isize = unsafe { msg_send![menu, numberOfItems] };
        (0..n)
            .map(|i| {
                let item: *mut AnyObject = unsafe { msg_send![menu, itemAtIndex: i] };
                let title: *mut NSString = unsafe { msg_send![item, title] };
                unsafe { (*title).to_string() }
            })
            .collect()
    }

    #[test]
    fn the_dock_menu_method_is_added_and_builds_the_menu() {
        // A stand-in for winit's delegate class.
        let cls = objc2::runtime::ClassBuilder::new(c"StrataTestAppDelegate", class!(NSObject)).expect("new class").register();
        let sel = sel!(applicationDockMenu:);
        assert!(!cls.responds_to(sel));
        assert!(add_dock_menu(cls));
        assert!(cls.responds_to(sel), "registered with the runtime");
        assert!(!add_dock_menu(cls), "a second time changes nothing");

        let files: Vec<PathBuf> = (0..12).map(|i| PathBuf::from(format!("/tmp/project/file{i}.rs"))).collect();
        let folders = vec![PathBuf::from("/tmp/project")];
        let (menu, shown) = make_dock_menu(&files, &folders);
        let t = titles(&menu);
        assert_eq!(t[0], "New Window");
        assert_eq!(&t[2..4], ["file0.rs", "file1.rs"]);
        assert_eq!(t.len(), 1 + 1 + 10 + 1 + 1, "ten recent files at most");
        assert_eq!(t.last().map(String::as_str), Some("project/"));
        assert_eq!(shown.len(), 11);
        assert_eq!(shown[10], folders[0]);

        // Calling it the way AppKit does gives a menu too.
        set_dock_recents(&files[..2], &[]);
        let delegate: Retained<AnyObject> = unsafe { msg_send![cls, new] };
        let menu: *mut AnyObject = unsafe { msg_send![&delegate, applicationDockMenu: &*delegate] };
        assert_eq!(titles(unsafe { &*menu }), ["New Window", "", "file0.rs", "file1.rs"]);
        set_dock_recents(&[], &[]);
    }
}
