//! NanoBuddy通知（DistributedNotificationCenter経由）

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::CString;
    use std::os::raw::c_void;
    use std::ptr;

    type CFRef = *const c_void;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFNotificationCenterGetDistributedCenter() -> CFRef;
        fn CFNotificationCenterPostNotification(
            center: CFRef,
            name: CFRef,
            object: CFRef,
            user_info: CFRef,
            deliver_immediately: bool,
        );
        fn CFStringCreateWithCString(alloc: CFRef, c_str: *const i8, encoding: u32) -> CFRef;
        fn CFRelease(cf: CFRef);
    }

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    pub fn post_speech(text: &str) {
        let name_c = match CString::new("owayo.nanobuddy.speech") {
            Ok(c) => c,
            Err(_) => return,
        };
        let text_c = match CString::new(text) {
            Ok(c) => c,
            Err(_) => return,
        };

        unsafe {
            let center = CFNotificationCenterGetDistributedCenter();
            let cf_name =
                CFStringCreateWithCString(ptr::null(), name_c.as_ptr(), K_CF_STRING_ENCODING_UTF8);
            let cf_text =
                CFStringCreateWithCString(ptr::null(), text_c.as_ptr(), K_CF_STRING_ENCODING_UTF8);

            CFNotificationCenterPostNotification(center, cf_name, cf_text, ptr::null(), true);

            CFRelease(cf_name);
            CFRelease(cf_text);
        }
    }
}

/// コミットメッセージをNanoBuddyに通知（吹き出しに表示）
/// メッセージの1行目のみ送信される
pub fn notify_commit_message(message: &str) {
    let first_line = message.lines().next().unwrap_or(message);

    #[cfg(target_os = "macos")]
    macos::post_speech(first_line);

    #[cfg(not(target_os = "macos"))]
    let _ = first_line;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notify_does_not_panic() {
        notify_commit_message("feat: test commit");
        notify_commit_message("");
        notify_commit_message("multi\nline\nmessage");
    }

    #[test]
    fn test_first_line_extraction() {
        let msg = "feat: add feature\n\nDetailed description";
        let first_line = msg.lines().next().unwrap_or(msg);
        assert_eq!(first_line, "feat: add feature");
    }

    #[test]
    fn test_first_line_single_line() {
        let msg = "fix: simple fix";
        let first_line = msg.lines().next().unwrap_or(msg);
        assert_eq!(first_line, "fix: simple fix");
    }

    #[test]
    fn test_first_line_empty() {
        let msg = "";
        let first_line = msg.lines().next().unwrap_or(msg);
        assert_eq!(first_line, "");
    }

    #[test]
    fn test_notify_with_unicode() {
        notify_commit_message("feat: Unicode テスト 日本語");
    }

    #[test]
    fn test_notify_with_emoji() {
        notify_commit_message("feat: add feature ✨🎉🐛");
    }

    #[test]
    fn test_first_line_with_body_bullets() {
        let msg = "feat: add feature\n\n- detail 1\n- detail 2";
        let first_line = msg.lines().next().unwrap_or(msg);
        assert_eq!(first_line, "feat: add feature");
    }
}
