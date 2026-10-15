//! Notifications through the User Notifications framework, and the Dock
//! icon's badge. macOS shows notifications only for an app bundle, so a
//! blitz run straight from its binary, as `cargo run` does, shows none.

use std::cell::OnceCell;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send_id, mutability};
use objc2_app_kit::NSApplication;
use objc2_foundation::{MainThreadMarker, NSArray, NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use winit::event_loop::EventLoopProxy;

use crate::app::UserEvent;
use crate::layout::PaneId;

/// What a notification leaves to keep while it is up; the notification
/// centre keeps it, and its click comes to the delegate.
pub type Toast = ();

declare_class!(
    /// Hears clicks on blitz's notifications.
    struct Delegate;

    // SAFETY: NSObject has no subclassing requirements, interior
    // mutability is a safe default, and Delegate does not implement Drop.
    unsafe impl ClassType for Delegate {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "BlitzNotificationDelegate";
    }

    impl DeclaredClass for Delegate {
        type Ivars = EventLoopProxy<UserEvent>;
    }

    // SAFETY: NSObject already conforms; Delegate adds nothing to it.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: the methods below have the protocol's selectors and types.
    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// A notification while blitz is in front, as when the user is
        /// away from it, shows as one in the background does. Without this
        /// macOS shows nothing.
        #[method(userNotificationCenter:willPresentNotification:withCompletionHandler:)]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            done: &block2::Block<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            done.call((UNNotificationPresentationOptions::UNNotificationPresentationOptionBanner
                | UNNotificationPresentationOptions::UNNotificationPresentationOptionList
                | UNNotificationPresentationOptions::UNNotificationPresentationOptionSound,));
        }

        #[method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:)]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            done: &block2::Block<dyn Fn()>,
        ) {
            // SAFETY: the response and what it holds stay alive for the
            // call.
            let id = unsafe { response.notification().request().identifier() };
            if let Some(pane) = pane_of(&id.to_string()) {
                let _ = self.ivars().send_event(UserEvent::ShowPane(pane));
            }
            done.call(());
        }
    }
);

thread_local! {
    /// The notification centre, once blitz runs from a bundle and has
    /// asked to show notifications. Only the main thread, the window's,
    /// uses it.
    static CENTER: OnceCell<Option<Retained<UNUserNotificationCenter>>> =
        const { OnceCell::new() };
}

/// The notification centre, if [`install`] got one.
fn center() -> Option<Retained<UNUserNotificationCenter>> {
    CENTER.with(|c| c.get().cloned().flatten())
}

/// Tells two blitz processes' notifications apart, as their panes may
/// have the same ids.
fn ident(id: PaneId) -> String {
    format!("blitz.{}.{}", std::process::id(), id.0)
}

/// The pane a notification of this process names.
fn pane_of(ident: &str) -> Option<PaneId> {
    let rest = ident.strip_prefix(&format!("blitz.{}.", std::process::id()))?;
    rest.parse().ok().map(PaneId)
}

/// Asks to show notifications, the first time, and has clicks on them
/// come back to `proxy`. Nothing outside a bundle, where the framework
/// would stop the process.
pub fn install(_hwnd: isize, proxy: EventLoopProxy<UserEvent>) {
    CENTER.with(|c| {
        c.get_or_init(|| {
            // SAFETY: plain reads of this process's bundle.
            let bundled = unsafe { NSBundle::mainBundle().bundleIdentifier() }.is_some();
            if !bundled {
                return None;
            }
            // SAFETY: the centre lives as long as the process. It holds its
            // delegate weakly, so the delegate is leaked to live as long.
            unsafe {
                let center = UNUserNotificationCenter::currentNotificationCenter();
                let delegate: Retained<Delegate> =
                    msg_send_id![super(Delegate::alloc().set_ivars(proxy)), init];
                center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                std::mem::forget(delegate);
                let asked = RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
                let options = UNAuthorizationOptions::UNAuthorizationOptionAlert
                    | UNAuthorizationOptions::UNAuthorizationOptionSound;
                center.requestAuthorizationWithOptions_completionHandler(options, &asked);
                Some(center)
            }
        });
    });
}

/// Shows a notification saying `lines` about pane `id`, the first line as
/// its title, in place of the one before about it. `None` when blitz
/// shows none, outside a bundle; one the user turned off goes nowhere.
pub fn show(
    id: PaneId,
    lines: &[&str],
    silent: bool,
    _proxy: &EventLoopProxy<UserEvent>,
) -> Result<Option<Toast>, String> {
    let Some(center) = center() else {
        return Ok(None);
    };
    let lines = super::clean_lines(lines);
    let Some((title, body)) = lines.split_first() else {
        return Ok(None);
    };
    // SAFETY: every object is made here and outlives the calls; the
    // centre copies the request.
    unsafe {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(&body.join("\n")));
        if !silent {
            content.setSound(Some(&UNNotificationSound::defaultSound()));
        }
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&ident(id)),
            &content,
            None,
        );
        center.addNotificationRequest_withCompletionHandler(&request, None);
    }
    Ok(Some(()))
}

/// Takes the notification about pane `id` out of the notification centre.
pub fn untoast(id: PaneId) {
    if let Some(center) = center() {
        let ids = NSArray::from_vec(vec![NSString::from_str(&ident(id))]);
        // SAFETY: the array outlives the call.
        unsafe { center.removeDeliveredNotificationsWithIdentifiers(&ids) };
    }
}

/// Takes down every notification, as nothing answers a click on one once
/// blitz exits.
pub fn untoast_all() {
    if let Some(center) = center() {
        // SAFETY: a plain call on the live centre.
        unsafe { center.removeAllDeliveredNotifications() };
    }
}

/// Puts `label` on the Dock icon, or with `None` takes it off. Only from
/// the main thread, which owns the application.
pub fn badge(label: Option<&str>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let label = label.map(NSString::from_str);
    // SAFETY: the application and its Dock tile live as long as the
    // process; the label outlives the call, which copies it.
    unsafe {
        NSApplication::sharedApplication(mtm)
            .dockTile()
            .setBadgeLabel(label.as_deref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_name_their_pane_and_process() {
        assert_eq!(pane_of(&ident(PaneId(7))), Some(PaneId(7)));
        let other = format!("blitz.{}.7", std::process::id() + 1);
        assert_eq!(pane_of(&other), None);
        assert_eq!(pane_of("other"), None);
    }
}
