//! Gemeinsame Sitzungsverwaltung für SSH- und Serial-Tabs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub enum Cmd {
    Data(Vec<u8>),
    Resize(u32, u32),
    Close,
}

pub enum Entry {
    /// Verbindung wird gerade aufgebaut – kann sofort abgebrochen werden.
    Pending(tokio::task::AbortHandle),
    Ssh(Arc<crate::ssh::SshSession>),
    Serial(Arc<crate::serial::SerialSession>),
}

pub type Sessions = Arc<Mutex<HashMap<String, Entry>>>;

pub fn close_entry(entry: Entry) {
    match entry {
        Entry::Pending(h) => h.abort(),
        Entry::Ssh(s) => s.close(),
        Entry::Serial(s) => s.close(),
    }
}
