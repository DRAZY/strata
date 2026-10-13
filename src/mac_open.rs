//! macOS: files opened from Finder, the Dock or `open -a Strata file` arrive
//! as an "open documents" Apple Event, which winit doesn't surface. We
//! install our own handler for it and queue the paths for the next frame.
//!
//! AppKit installs its default handlers while the app finishes launching, so
//! ours goes in from the `NSApplicationWillFinishLaunching` notification,
//! which is still before the cold-start event is delivered.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, define_class, msg_send, sel};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSString};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::{Mutex, OnceLock};

static QUEUE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CTX: OnceLock<egui::Context> = OnceLock::new();

const fn four(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class has no Drop.
    #[unsafe(super(NSObject))]
    #[name = "StrataOpenDocumentsHandler"]
    struct Handler;

    impl Handler {
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

fn install_handler() {
    let handler: Retained<Handler> = unsafe { msg_send![Handler::alloc(), init] };
    let manager = NSAppleEventManager::sharedAppleEventManager();
    let _: () = unsafe {
        msg_send![&manager, setEventHandler: &*handler as &AnyObject, andSelector: sel!(handleOpen:withReply:), forEventClass: four(b"aevt"), andEventID: four(b"odoc")]
    };
    // The event manager doesn't retain its handlers.
    std::mem::forget(handler);
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
