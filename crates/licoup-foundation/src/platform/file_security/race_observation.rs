//! Test-only observation point in the private-file read window.
//!
//! A private-file reader inspects the path first and then opens it with
//! `O_NOFOLLOW`; a path swapped for a symlink between those two operations must
//! be refused by the opened descriptor rather than followed. That window is too
//! short to reproduce reliably from another thread, so a test can install an
//! observer that runs at exactly that point and performs the swap.
//!
//! Production builds do not contain this module: it compiles for this crate's
//! unit tests and for dependents that explicitly enable `test-support`. It
//! never changes the production read sequence and must not be used to bypass
//! the validation that follows.

use std::cell::RefCell;
use std::path::Path;

type Observer = Box<dyn FnMut(&Path)>;

thread_local! {
    static OBSERVER: RefCell<Option<Observer>> = const { RefCell::new(None) };
}

/// Removes the installed observer when it goes out of scope.
///
/// The observer is thread-local, so a guard installed by one test cannot
/// observe a read performed by another test thread.
#[must_use = "the observer is uninstalled when the guard is dropped"]
pub struct BeforePrivateFileOpenGuard;

impl Drop for BeforePrivateFileOpenGuard {
    fn drop(&mut self) {
        OBSERVER.with(|slot| {
            *slot.borrow_mut() = None;
        });
    }
}

/// Runs `observer` immediately after a reader has inspected and validated the
/// path, and immediately before it opens that path without following links.
///
/// The observer receives the exact path the reader is about to open. Installing
/// a new observer on the same thread replaces the previous one.
pub fn observe_before_private_file_open(
    observer: impl FnMut(&Path) + 'static,
) -> BeforePrivateFileOpenGuard {
    OBSERVER.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(observer));
    });
    BeforePrivateFileOpenGuard
}

pub(super) fn notify(path: &Path) {
    OBSERVER.with(|slot| {
        if let Some(observer) = slot.borrow_mut().as_mut() {
            observer(path);
        }
    });
}
