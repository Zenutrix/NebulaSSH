//! SSH-Schlüssel erzeugen, prüfen und exportieren.

use crate::{t, tf};
use ssh_key::{
    getrandom::SysRng, rand_core::UnwrapErr, Algorithm, EcdsaCurve, HashAlg, LineEnding, PrivateKey,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyType {
    Ed25519,
    Rsa4096,
    EcdsaP256,
}

impl KeyType {
    pub fn label(self) -> &'static str {
        match self {
            KeyType::Ed25519 => t!("Ed25519 (empfohlen)", "Ed25519 (recommended)"),
            KeyType::Rsa4096 => t!("RSA 4096 (für alte Geräte)", "RSA 4096 (for old devices)"),
            KeyType::EcdsaP256 => "ECDSA P-256",
        }
    }
}

pub struct GeneratedKey {
    /// Private Key im OpenSSH-Format (ggf. mit Passphrase verschlüsselt)
    pub private_openssh: String,
    /// Zeile für ~/.ssh/authorized_keys
    pub public_line: String,
    pub fingerprint: String,
}

pub fn generate(kind: KeyType, comment: &str, passphrase: &str) -> Result<GeneratedKey, String> {
    let mut rng = UnwrapErr(SysRng);
    let mut key = match kind {
        KeyType::Ed25519 => PrivateKey::random(&mut rng, Algorithm::Ed25519),
        KeyType::EcdsaP256 => PrivateKey::random(
            &mut rng,
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP256,
            },
        ),
        KeyType::Rsa4096 => {
            let kp = ssh_key::private::RsaKeypair::random(&mut rng, 4096).map_err(|e| e.to_string())?;
            PrivateKey::new(ssh_key::private::KeypairData::Rsa(kp), comment)
        }
    }
    .map_err(|e| tf!("Schlüssel konnte nicht erzeugt werden: {e}", "Could not generate the key: {e}"))?;
    key.set_comment(comment);
    let public_line = key.public_key().to_openssh().map_err(|e| e.to_string())?;
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    let key = if passphrase.is_empty() {
        key
    } else {
        key.encrypt(&mut rng, passphrase).map_err(|e| e.to_string())?
    };
    let private_openssh = key
        .to_openssh(LineEnding::LF)
        .map_err(|e| e.to_string())?
        .to_string();
    Ok(GeneratedKey {
        private_openssh,
        public_line,
        fingerprint,
    })
}

/// Leitet aus einem gespeicherten Private Key den Public Key ab (für Export / Anzeige).
/// Bei OpenSSH-Keys liegt der öffentliche Teil auch bei Verschlüsselung im Klartext vor.
pub fn public_from_private(pem: &str) -> Result<(String, String), String> {
    if let Ok(k) = PrivateKey::from_openssh(pem.trim()) {
        let line = k.public_key().to_openssh().map_err(|e| e.to_string())?;
        return Ok((line, k.public_key().fingerprint(HashAlg::Sha256).to_string()));
    }
    // PEM/PKCS#8 (z.B. "BEGIN RSA PRIVATE KEY") über russh einlesen
    let k = russh::keys::decode_secret_key(pem, None).map_err(|e| tf!("Key nicht lesbar: {e}", "Cannot read key: {e}"))?;
    let pk = k.public_key();
    let line = pk.to_openssh().map_err(|e| e.to_string())?;
    Ok((
        line,
        pk.fingerprint(russh::keys::ssh_key::HashAlg::Sha256).to_string(),
    ))
}

pub fn is_encrypted(pem: &str) -> bool {
    PrivateKey::from_openssh(pem.trim())
        .map(|k| k.is_encrypted())
        .unwrap_or(false)
        || pem.contains("ENCRYPTED")
}

/// Anleitung zum Hinterlegen des Public Keys auf einem Linux-Server.
pub fn install_instructions(public_line: &str) -> String {
    tf!(
        "# 1) Auf dem Linux-Server als gewünschter Benutzer anmelden\n\
         # 2) Diese Befehle ausführen:\n\
         mkdir -p ~/.ssh && chmod 700 ~/.ssh\n\
         echo '{public_line}' >> ~/.ssh/authorized_keys\n\
         chmod 600 ~/.ssh/authorized_keys\n\
         \n\
         # Alternativ von einem Linux/macOS-Rechner aus:\n\
         # ssh-copy-id -i schluessel.pub benutzer@server\n",
        "# 1) Log in to the Linux server as the desired user\n\
         # 2) Run these commands:\n\
         mkdir -p ~/.ssh && chmod 700 ~/.ssh\n\
         echo '{public_line}' >> ~/.ssh/authorized_keys\n\
         chmod 600 ~/.ssh/authorized_keys\n\
         \n\
         # Alternatively from a Linux/macOS machine:\n\
         # ssh-copy-id -i key.pub user@server\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generate_and_reparse() {
        let k = generate(KeyType::Ed25519, "test@nebula", "").unwrap();
        assert!(k.public_line.starts_with("ssh-ed25519 "));
        assert!(k.public_line.ends_with("test@nebula"));
        let (pub2, fp2) = public_from_private(&k.private_openssh).unwrap();
        assert_eq!(pub2, k.public_line);
        assert_eq!(fp2, k.fingerprint);
        // Passphrase-geschützt: russh muss ihn mit Passphrase laden können
        let e = generate(KeyType::Ed25519, "x", "geheim123").unwrap();
        assert!(is_encrypted(&e.private_openssh));
        assert!(russh::keys::decode_secret_key(&e.private_openssh, Some("geheim123")).is_ok());
        assert!(russh::keys::decode_secret_key(&e.private_openssh, Some("falsch")).is_err());
        let p = generate(KeyType::EcdsaP256, "x", "").unwrap();
        assert!(p.public_line.starts_with("ecdsa-sha2-nistp256 "));
    }
}
