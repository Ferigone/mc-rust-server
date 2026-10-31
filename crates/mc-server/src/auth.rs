//! Tryb online: klucz RSA serwera, szyfrowanie AES (wspólny sekret), weryfikacja sesji u Mojang.

use anyhow::{anyhow, bail, Result};
use mc_protocol::packets::GameProfile;
use rsa::pkcs8::EncodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use sha1::{Digest, Sha1};
use uuid::Uuid;

pub struct ServerKey {
    private: RsaPrivateKey,
    /// Klucz publiczny w DER (X.509 SubjectPublicKeyInfo) — dokładnie to, co wysyła vanilla.
    pub public_der: Vec<u8>,
}

impl ServerKey {
    /// Vanilla: `Crypt.generateKeyPair()` -> RSA 1024.
    pub fn generate() -> Result<Self> {
        let private = RsaPrivateKey::new(&mut rand::thread_rng(), 1024)?;
        let public = RsaPublicKey::from(&private);
        let public_der = public.to_public_key_der()?.as_bytes().to_vec();
        Ok(Self { private, public_der })
    }

    /// Odszyfrowuje sekret i token. Zwraca (sekret AES, czy token się zgadza).
    pub fn decrypt(&self, secret_enc: &[u8], token_enc: &[u8], expected_token: &[u8]) -> Result<([u8; 16], bool)> {
        let token = self.private.decrypt(Pkcs1v15Encrypt, token_enc).map_err(|e| anyhow!("token: {e}"))?;
        let secret = self.private.decrypt(Pkcs1v15Encrypt, secret_enc).map_err(|e| anyhow!("sekret: {e}"))?;
        let secret: [u8; 16] = secret.try_into().map_err(|_| anyhow!("sekret ma zły rozmiar"))?;
        Ok((secret, token == expected_token))
    }
}

/// `new BigInteger(sha1_bytes).toString(16)` — hex ze znakiem w kodzie uzupełnień do dwóch.
pub fn minecraft_hex_digest(parts: &[&[u8]]) -> String {
    let mut h = Sha1::new();
    for p in parts {
        h.update(p);
    }
    let mut d: [u8; 20] = h.finalize().into();
    let negative = d[0] & 0x80 != 0;
    if negative {
        // uzupełnienie do dwóch
        let mut carry = true;
        for b in d.iter_mut().rev() {
            *b = !*b;
            if carry {
                let (v, c) = b.overflowing_add(1);
                *b = v;
                carry = c;
            }
        }
    }
    let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
    let trimmed = hex.trim_start_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
    if negative {
        format!("-{trimmed}")
    } else {
        trimmed.to_owned()
    }
}

/// `Crypt.digestData(serverId, publicKey, secretKey)`.
pub fn server_hash(server_id: &str, secret: &[u8; 16], public_der: &[u8]) -> String {
    minecraft_hex_digest(&[server_id.as_bytes(), secret, public_der])
}

const DEFAULT_SESSION_URL: &str = "https://sessionserver.mojang.com/session/minecraft/hasJoined";

/// `hasJoinedServer`. `Ok(None)` = gracz nie dołączył (204); `Err` = serwery niedostępne.
/// URL można nadpisać zmienną `MC_SESSION_URL` (testy z atrapą serwera sesji).
pub fn has_joined(name: &str, hash: &str, ip: Option<std::net::IpAddr>) -> Result<Option<GameProfile>> {
    let base = std::env::var("MC_SESSION_URL").unwrap_or_else(|_| DEFAULT_SESSION_URL.to_owned());
    let mut req = ureq::get(&base).query("username", name).query("serverId", hash);
    let ip_s;
    if let Some(ip) = ip {
        ip_s = ip.to_string();
        req = req.query("ip", &ip_s);
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(204, _)) | Err(ureq::Error::Status(403, _)) => return Ok(None),
        Err(e) => bail!("{e}"),
    };
    if resp.status() == 204 {
        return Ok(None);
    }
    let v: serde_json::Value = serde_json::from_reader(resp.into_reader())?;
    let id = Uuid::parse_str(v["id"].as_str().ok_or_else(|| anyhow!("brak id"))?)?;
    let name = v["name"].as_str().ok_or_else(|| anyhow!("brak name"))?.to_owned();
    let properties = v["properties"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    Some((p["name"].as_str()?.to_owned(), p["value"].as_str()?.to_owned(), p["signature"].as_str().map(str::to_owned)))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(GameProfile { id, name, properties }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Wektory z dokumentacji protokołu (hash nazwy: sha1(name) jako BigInteger hex).
    #[test]
    fn hex_digest_vectors() {
        assert_eq!(minecraft_hex_digest(&[b"Notch"]), "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48");
        assert_eq!(minecraft_hex_digest(&[b"jeb_"]), "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1");
        assert_eq!(minecraft_hex_digest(&[b"simon"]), "88e16a1019277b15d58faf0541e11910eb756f6");
    }

    #[test]
    fn rsa_roundtrip() {
        let k = ServerKey::generate().unwrap();
        let public = rsa::RsaPublicKey::from(&k.private);
        let secret = [9u8; 16];
        let token = [1u8, 2, 3, 4];
        let se = public.encrypt(&mut rand::thread_rng(), Pkcs1v15Encrypt, &secret).unwrap();
        let te = public.encrypt(&mut rand::thread_rng(), Pkcs1v15Encrypt, &token).unwrap();
        let (s, ok) = k.decrypt(&se, &te, &token).unwrap();
        assert_eq!(s, secret);
        assert!(ok);
    }
}
