//! NebulaSSH Kern – unabhängig von der Oberfläche.
//!
//! Verbindungen liefern ihre Ausgabe über einen [`Sink`] (Callback), Eingaben gehen
//! über [`Cmd`]s an die jeweilige Sitzung.

pub mod backup;
pub mod keys;
pub mod serial;
pub mod ssh;
pub mod storage;
pub mod update;

use std::sync::Arc;

/// Befehle an eine laufende Verbindung.
pub enum Cmd {
    Data(Vec<u8>),
    Resize(u32, u32),
    Close,
}

/// Ereignisse einer Verbindung an die Oberfläche.
pub enum Event {
    Data(Vec<u8>),
    Closed(String),
}

/// Empfänger für Verbindungsereignisse. Wird aus Hintergrund-Threads aufgerufen.
pub type Sink = Arc<dyn Fn(Event) + Send + Sync + 'static>;

/// Gemeinsame Schnittstelle für SSH- und Serial-Sitzungen.
pub trait Connection: Send + Sync {
    fn send(&self, cmd: Cmd);
    fn close(&self) {
        self.send(Cmd::Close);
    }
}
