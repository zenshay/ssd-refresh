//! Messages for start errors.
//! The app has no console. Without these messages, a start error stops the app
//! and the user sees nothing.

use rfd::{MessageButtons, MessageDialog, MessageLevel};

const TITLE: &str = "ssd-refresh cannot start";
#[cfg(windows)]
const WEBVIEW2_DOWNLOAD: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";

/// Returns false when WebView2 is not available. The user saw a message.
///
/// Only Windows needs this check. macOS always has a web view. On Linux, the
/// program does not load without WebKitGTK, so this code cannot run.
#[cfg(windows)]
pub fn webview_ready() -> bool {
    use rfd::MessageDialogResult;

    let Err(error) = tauri::webview_version() else {
        return true;
    };
    let open = MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title(TITLE)
        .set_description(missing_webview_text(&error.to_string()))
        .set_buttons(MessageButtons::YesNo)
        .show();
    if open == MessageDialogResult::Yes {
        // The message also gives the address, so a failure here is not fatal.
        let _ = crate::native_open::open_url_unchecked(WEBVIEW2_DOWNLOAD);
    }
    false
}

#[cfg(not(windows))]
pub fn webview_ready() -> bool {
    true
}

/// Shows an error that stopped the app before or while the window opened.
pub fn show_start_error(detail: &str) {
    MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title(TITLE)
        .set_description(start_error_text(detail))
        .set_buttons(MessageButtons::Ok)
        .show();
}

#[cfg(windows)]
fn missing_webview_text(detail: &str) -> String {
    format!(
        "ssd-refresh needs Microsoft Edge WebView2 Runtime. \
         Windows did not find it on this computer.\n\n\
         Install the Evergreen Runtime from Microsoft: {WEBVIEW2_DOWNLOAD}\n\
         Then start ssd-refresh again.\n\n\
         Open the download page now?\n\n\
         Details: {detail}"
    )
}

fn start_error_text(detail: &str) -> String {
    format!(
        "An error stopped ssd-refresh before the window opened.\n\n\
         Details: {detail}\n\n\
         To report it, open https://github.com/zenshay/ssd-refresh/issues/new"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn missing_webview_text_names_the_runtime_page_and_detail() {
        let text = missing_webview_text("synthetic detail");
        assert!(text.contains("Microsoft Edge WebView2 Runtime"));
        assert!(text.contains(WEBVIEW2_DOWNLOAD));
        assert!(text.contains("Open the download page now?"));
        assert!(text.ends_with("Details: synthetic detail"));
    }

    #[test]
    fn start_error_text_keeps_the_detail() {
        let text = start_error_text("synthetic failure");
        assert!(text.contains("Details: synthetic failure"));
        assert!(text.contains("issues/new"));
    }
}
