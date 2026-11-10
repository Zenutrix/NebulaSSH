//! Netzwerk-Werkzeuge: Wake-on-LAN, Ping (ICMP) und Port-Prüfung.

use crate::{t, tf};
use std::net::{IpAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// "aa:bb:cc:dd:ee:ff", "AA-BB-…", "aabb.ccdd.eeff" oder "aabbccddeeff"
pub fn parse_mac(mac: &str) -> Result<[u8; 6], String> {
    let hex: String = mac.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let sep_ok = mac.chars().all(|c| c.is_ascii_hexdigit() || ":-. ".contains(c));
    if hex.len() != 12 || !sep_ok {
        return Err(tf!("Ungültige MAC-Adresse: „{mac}“", "Invalid MAC address: \"{mac}\""));
    }
    let mut out = [0u8; 6];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    Ok(out)
}

pub fn magic_packet(mac: [u8; 6]) -> Vec<u8> {
    let mut p = vec![0xffu8; 6];
    for _ in 0..16 {
        p.extend_from_slice(&mac);
    }
    p
}

/// Sendet ein Wake-on-LAN-Paket (Broadcast an Port 9 und 7).
/// `broadcast` z.B. "192.168.1.255" für ein bestimmtes Netz, sonst 255.255.255.255.
pub fn wake_on_lan(mac: &str, broadcast: Option<&str>) -> Result<(), String> {
    let packet = magic_packet(parse_mac(mac)?);
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    sock.set_broadcast(true).map_err(|e| e.to_string())?;
    let addr = broadcast.filter(|b| !b.trim().is_empty()).unwrap_or("255.255.255.255");
    let mut sent = false;
    for port in [9, 7] {
        if sock.send_to(&packet, (addr.trim(), port)).is_ok() {
            sent = true;
        }
    }
    if sent {
        Ok(())
    } else {
        Err(t!("Wake-on-LAN-Paket konnte nicht gesendet werden.", "Could not send the Wake-on-LAN packet.").into())
    }
}

fn resolve(host: &str) -> Result<IpAddr, String> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ip);
    }
    let mut addrs: Vec<_> = (host, 0)
        .to_socket_addrs()
        .map_err(|e| tf!("{host} nicht gefunden: {e}", "{host} not found: {e}"))?
        .collect();
    // IPv4 bevorzugen (ICMP über IcmpSendEcho)
    addrs.sort_by_key(|a| !a.is_ipv4());
    addrs.first().map(|a| a.ip()).ok_or_else(|| tf!("{host} nicht gefunden", "{host} not found"))
}

/// Prüft, ob ein TCP-Port erreichbar ist. Liefert die Verbindungszeit.
pub fn tcp_check(host: &str, port: u16, timeout: Duration) -> Result<Duration, String> {
    let ip = resolve(host)?;
    let start = Instant::now();
    std::net::TcpStream::connect_timeout(&(ip, port).into(), timeout)
        .map(|_| start.elapsed())
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                tf!("Port {port}: keine Antwort", "Port {port}: no answer")
            } else {
                tf!("Port {port}: {e}", "Port {port}: {e}")
            }
        })
}

/// ICMP-Echo (wie `ping`). Liefert die Antwortzeit.
pub fn ping(host: &str, timeout: Duration) -> Result<Duration, String> {
    let ip = resolve(host)?;
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::NetworkManagement::IpHelper::{IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, ICMP_ECHO_REPLY};
        let IpAddr::V4(v4) = ip else {
            return Err(t!("Ping ist nur für IPv4 verfügbar.", "Ping is only available for IPv4.").into());
        };
        unsafe {
            let h = IcmpCreateFile();
            if h == INVALID_HANDLE_VALUE {
                return Err("IcmpCreateFile".into());
            }
            let data = *b"NebulaSSH-ping!!";
            let mut reply = vec![0u8; std::mem::size_of::<ICMP_ECHO_REPLY>() + data.len() + 8];
            let n = IcmpSendEcho(
                h,
                u32::from_ne_bytes(v4.octets()),
                data.as_ptr().cast(),
                data.len() as u16,
                std::ptr::null(),
                reply.as_mut_ptr().cast(),
                reply.len() as u32,
                timeout.as_millis() as u32,
            );
            IcmpCloseHandle(h);
            if n == 0 {
                return Err(t!("Keine Antwort auf Ping", "No reply to ping").into());
            }
            let r = &*(reply.as_ptr() as *const ICMP_ECHO_REPLY);
            if r.Status != 0 {
                return Err(tf!("Ping fehlgeschlagen (Status {})", "Ping failed (status {})", r.Status));
            }
            Ok(Duration::from_millis(r.RoundTripTime as u64))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (ip, timeout);
        Err("Ping nur unter Windows".into())
    }
}

/// Ergebnis einer Erreichbarkeitsprüfung.
#[derive(Clone, Debug)]
pub struct Reach {
    pub ping: Result<Duration, String>,
    pub port: Option<(u16, Result<Duration, String>)>,
}

impl Reach {
    pub fn online(&self) -> bool {
        self.ping.is_ok() || self.port.as_ref().is_some_and(|p| p.1.is_ok())
    }
    pub fn summary(&self) -> String {
        let ms = |d: &Duration| format!("{} ms", d.as_millis());
        let mut s = match &self.ping {
            Ok(d) => format!("Ping {}", ms(d)),
            Err(e) => format!("Ping: {e}"),
        };
        if let Some((p, r)) = &self.port {
            s.push_str(" · ");
            s.push_str(&match r {
                Ok(d) => tf!("Port {p} offen ({})", "port {p} open ({})", ms(d)),
                Err(e) => e.clone(),
            });
        }
        s
    }
}

pub fn check(host: &str, port: Option<u16>) -> Reach {
    let timeout = Duration::from_millis(1500);
    Reach { ping: ping(host, timeout), port: port.map(|p| (p, tcp_check(host, p, timeout))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_parsing() {
        let m = [0xaa, 0xbb, 0xcc, 0x01, 0x02, 0x03];
        assert_eq!(parse_mac("aa:bb:cc:01:02:03").unwrap(), m);
        assert_eq!(parse_mac("AA-BB-CC-01-02-03").unwrap(), m);
        assert_eq!(parse_mac("aabb.cc01.0203").unwrap(), m);
        assert!(parse_mac("aa:bb:cc").is_err());
        assert!(parse_mac("zz:bb:cc:01:02:03").is_err());
        let p = magic_packet(m);
        assert_eq!(p.len(), 102);
        assert_eq!(&p[..6], &[0xff; 6]);
        assert_eq!(&p[96..], &m);
    }

    #[test]
    fn local_checks() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(tcp_check("127.0.0.1", port, Duration::from_secs(1)).is_ok());
        assert!(ping("127.0.0.1", Duration::from_secs(1)).is_ok());
        drop(l);
        assert!(tcp_check("127.0.0.1", port, Duration::from_secs(1)).is_err());
    }
}
