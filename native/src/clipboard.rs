//! Zugriff auf die Windows-Zwischenablage (lesen, zeitgesteuert leeren).

pub fn get() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

/// Kopiert sensiblen Text und leert die Zwischenablage nach `secs` Sekunden wieder,
/// sofern sie dann noch denselben Inhalt hat.
pub fn copy_secret(text: String, secs: u64) {
    let Ok(mut cb) = arboard::Clipboard::new() else { return };
    if cb.set_text(text.clone()).is_err() || secs == 0 {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(secs));
        if let Ok(mut cb) = arboard::Clipboard::new() {
            if cb.get_text().ok().as_deref() == Some(text.as_str()) {
                let _ = cb.clear();
            }
        }
    });
}
