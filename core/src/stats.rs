//! Server-Kennzahlen (CPU, RAM, Festplatte, Laufzeit) für Linux-Server.

use crate::ssh::{exec, SshSession};
use std::time::Duration;

/// Ein einzelner Befehl, der alle Werte auf einmal liefert (zwei CPU-Messungen im Abstand von 0,5 s).
pub const COMMAND: &str = "LC_ALL=C; export LC_ALL; \
echo '#cpu'; head -n1 /proc/stat; sleep 0.5; head -n1 /proc/stat; \
echo '#mem'; grep -E '^(MemTotal|MemAvailable):' /proc/meminfo; \
echo '#disk'; df -Pk / | tail -n1; \
echo '#up'; cat /proc/uptime; \
echo '#load'; cat /proc/loadavg; \
echo '#cores'; nproc 2>/dev/null || grep -c ^processor /proc/cpuinfo";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    /// CPU-Auslastung in Prozent
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub uptime_secs: u64,
    pub load1: f32,
    pub cores: u32,
}

impl Stats {
    pub fn mem_pct(&self) -> f32 {
        pct(self.mem_used, self.mem_total)
    }
    pub fn disk_pct(&self) -> f32 {
        pct(self.disk_used, self.disk_total)
    }
}

fn pct(a: u64, b: u64) -> f32 {
    if b == 0 {
        0.0
    } else {
        a as f32 * 100.0 / b as f32
    }
}

fn cpu_times(line: &str) -> Option<(u64, u64)> {
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    Some((v.iter().sum(), idle))
}

/// Wertet die Ausgabe von [`COMMAND`] aus. `None`, wenn es kein Linux ist.
pub fn parse(out: &str) -> Option<Stats> {
    let mut s = Stats::default();
    let mut sec = "";
    let mut cpu = Vec::new();
    let mut ok = 0;
    for line in out.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('#') {
            sec = match name {
                "cpu" => "cpu",
                "mem" => "mem",
                "disk" => "disk",
                "up" => "up",
                "load" => "load",
                "cores" => "cores",
                _ => "",
            };
            continue;
        }
        match sec {
            "cpu" if line.starts_with("cpu") => cpu.extend(cpu_times(line)),
            "mem" => {
                let kb: u64 = line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                if line.starts_with("MemTotal") {
                    s.mem_total = kb * 1024;
                    ok += 1;
                } else if line.starts_with("MemAvailable") {
                    s.mem_used = s.mem_total.saturating_sub(kb * 1024);
                }
            }
            "disk" => {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() >= 4 {
                    s.disk_total = f[1].parse::<u64>().unwrap_or(0) * 1024;
                    s.disk_used = f[2].parse::<u64>().unwrap_or(0) * 1024;
                    ok += 1;
                }
            }
            "up" => {
                s.uptime_secs = line.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) as u64;
            }
            "load" => s.load1 = line.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0.0),
            "cores" => s.cores = line.parse().unwrap_or(0),
            _ => {}
        }
    }
    if cpu.len() == 2 {
        let (t0, i0) = cpu[0];
        let (t1, i1) = cpu[1];
        let dt = t1.saturating_sub(t0);
        if dt > 0 {
            s.cpu = 100.0 * (dt - i1.saturating_sub(i0)) as f32 / dt as f32;
        }
        ok += 1;
    }
    (ok >= 2).then_some(s)
}

pub async fn fetch(sess: &SshSession) -> Result<Option<Stats>, String> {
    let out = exec(sess, COMMAND, Duration::from_secs(10)).await?;
    Ok(parse(&out))
}

/// "3 T 4 Std" / "3 d 4 h"
pub fn uptime_text(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs / 3600) % 24, (secs / 60) % 60);
    if d > 0 {
        crate::tf!("{d} T {h} Std", "{d} d {h} h")
    } else if h > 0 {
        crate::tf!("{h} Std {m} Min", "{h} h {m} min")
    } else {
        crate::tf!("{m} Min", "{m} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_linux_output() {
        let out = "#cpu\ncpu  100 0 100 800 0 0 0 0 0 0\ncpu  150 0 150 850 50 0 0 0 0 0\n#mem\nMemTotal:       8000000 kB\nMemAvailable:   6000000 kB\n#disk\n/dev/sda1 100000 40000 60000 40% /\n#up\n93784.12 12345.00\n#load\n0.52 0.40 0.30 1/200 999\n#cores\n4\n";
        let s = parse(out).unwrap();
        assert!((s.cpu - 50.0).abs() < 0.1, "{}", s.cpu);
        assert_eq!(s.mem_total, 8_000_000 * 1024);
        assert!((s.mem_pct() - 25.0).abs() < 0.1);
        assert!((s.disk_pct() - 40.0).abs() < 0.1);
        assert_eq!(s.uptime_secs, 93784);
        assert_eq!(s.cores, 4);
        assert!((s.load1 - 0.52).abs() < 0.01);
    }

    #[test]
    fn rejects_non_linux() {
        assert!(parse("% Invalid input detected at '^' marker.").is_none());
    }
}
