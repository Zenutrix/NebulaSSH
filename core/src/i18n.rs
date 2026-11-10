//! Zweisprachigkeit (Deutsch / Englisch).
//!
//! Texte stehen direkt im Code in beiden Sprachen:
//! `t!("Abbrechen", "Cancel")` liefert `&'static str`,
//! `tf!("Verbindung zu {host}", "Connecting to {host}")` liefert einen `String`.

use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

pub fn set_english(on: bool) {
    ENGLISH.store(on, Ordering::Relaxed);
}

/// "auto" richtet sich nach der Windows-Anzeigesprache.
pub fn apply(setting: &str) {
    set_english(match setting {
        "en" => true,
        "de" => false,
        _ => !system_is_german(),
    });
}

pub fn system_is_german() -> bool {
    #[cfg(windows)]
    {
        // LANG_GERMAN = 0x07 (untere 10 Bit der LANGID)
        let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        id & 0x3ff == 0x07
    }
    #[cfg(not(windows))]
    {
        std::env::var("LANG").map(|l| l.starts_with("de")).unwrap_or(false)
    }
}

#[macro_export]
macro_rules! t {
    ($de:literal, $en:literal) => {
        if $crate::i18n::english() {
            $en
        } else {
            $de
        }
    };
}

#[macro_export]
macro_rules! tf {
    ($de:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::english() {
            format!($en $(, $arg)*)
        } else {
            format!($de $(, $arg)*)
        }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn switches_language() {
        let n = 3;
        super::set_english(true);
        assert_eq!(crate::t!("Ja", "Yes"), "Yes");
        assert_eq!(crate::tf!("{n} Dateien", "{n} files"), "3 files");
        super::set_english(false);
        assert_eq!(crate::tf!("{} Dateien", "{} files", n), "3 Dateien");
    }
}
