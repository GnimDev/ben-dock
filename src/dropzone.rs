//! Lets things be dragged onto a dock from other programs: apps, shortcuts
//! and files from Explorer or the desktop, apps from the Start menu.
//!
//! Like the window procedures, this only records what happened (see the
//! events in `app.rs`); the dock that is dragged over does the rest.

use crate::app::{self, Event};
use crate::shell;
use std::cell::RefCell;
use windows::core::{implement, Ref, Result};
use windows::Win32::Foundation::{HWND, POINTL};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;

thread_local! {
    /// What the drag over a dock carries (or the last one carried), as launch targets.
    static CARRIED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// What the drag over a dock carries, or what was just dropped on it.
pub fn carried() -> Vec<String> {
    CARRIED.with(|c| c.borrow().clone())
}

#[implement(IDropTarget)]
struct Target {
    window: isize,
}

/// Tells the program the drag comes from what a drop here would do: make a link to what it
/// carries, or nothing if it carries nothing a dock can hold.
fn answer(effect: *mut DROPEFFECT) {
    if effect.is_null() {
        return;
    }
    let wanted = CARRIED.with(|c| !c.borrow().is_empty());
    unsafe {
        let allowed = *effect;
        *effect = if wanted && allowed.0 & DROPEFFECT_LINK.0 != 0 {
            DROPEFFECT_LINK
        } else if wanted && allowed.0 & DROPEFFECT_COPY.0 != 0 {
            DROPEFFECT_COPY
        } else {
            DROPEFFECT_NONE
        };
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let targets = data.as_ref().map(shell::targets_in).unwrap_or_default();
        let any = !targets.is_empty();
        CARRIED.with(|c| *c.borrow_mut() = targets);
        if any {
            app::post(Event::DragIn(self.window, pt.x));
        }
        answer(effect);
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        // The dock follows the cursor by itself.
        answer(effect);
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        app::post(Event::DragOut(self.window));
        Ok(())
    }

    fn Drop(&self, _data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        answer(effect);
        app::post(Event::Dropped(self.window));
        Ok(())
    }
}

/// From now on things can be dropped on `window` (a dock's).
pub fn accept(window: HWND) {
    let target: IDropTarget = Target { window: window.0 as isize }.into();
    unsafe {
        // The window may have been a dock before (the graphics were rebuilt).
        let _ = RevokeDragDrop(window);
        if let Err(e) = RegisterDragDrop(window, &target) {
            crate::log!("nothing can be dropped on the dock ({e})");
        }
    }
}

/// `window` is going away.
pub fn forget(window: HWND) {
    unsafe {
        let _ = RevokeDragDrop(window);
    }
}
