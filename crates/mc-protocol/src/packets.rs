//! Packets of the handshake / status / login / configuration phases (field layout reconstructed from vanilla 1.21.1 code).
//! The `play` state lives in `play` (server-side encoding).

use crate::buf::{Put, Reader};
use crate::error::Result;
use crate::frame::payload;
use crate::ids;
use crate::nbt::Tag;
use uuid::Uuid;

pub mod play;

// ---------------------------------------------------------------- handshake

#[derive(Debug, Clone)]
pub struct Intention {
    pub protocol_version: i32,
    pub server_address: String,
    pub server_port: u16,
    /// 1 = status, 2 = login, 3 = transfer
    pub next_state: i32,
}

impl Intention {
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            protocol_version: r.varint()?,
            server_address: r.string(255)?,
            server_port: r.u16()?,
            next_state: r.varint()?,
        })
    }
    pub fn encode(&self) -> Vec<u8> {
        payload(ids::handshake::serverbound::INTENTION, |w| {
            w.put_varint(self.protocol_version);
            w.put_string(&self.server_address);
            w.put_u16(self.server_port);
            w.put_varint(self.next_state);
        })
    }
}

// ------------------------------------------------------------------- status

pub fn status_response(json: &str) -> Vec<u8> {
    payload(ids::status::clientbound::STATUS_RESPONSE, |w| w.put_string(json))
}
pub fn pong_response(time: i64) -> Vec<u8> {
    payload(ids::status::clientbound::PONG_RESPONSE, |w| w.put_i64(time))
}

// -------------------------------------------------------------------- login

/// Serverbound `hello`: nick (16) + UUID.
#[derive(Debug, Clone)]
pub struct LoginHello {
    pub name: String,
    pub profile_id: Uuid,
}
impl LoginHello {
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self { name: r.string(16)?, profile_id: r.uuid()? })
    }
    pub fn encode(&self) -> Vec<u8> {
        payload(ids::login::serverbound::HELLO, |w| {
            w.put_string(&self.name);
            w.put_uuid(self.profile_id);
        })
    }
}

/// Login disconnect uses a JSON component (not NBT).
pub fn login_disconnect(reason: &crate::text::Text) -> Vec<u8> {
    payload(ids::login::clientbound::LOGIN_DISCONNECT, |w| w.put_string(&reason.to_json().to_string()))
}

pub fn login_encryption_request(server_id: &str, public_key_der: &[u8], challenge: &[u8], should_authenticate: bool) -> Vec<u8> {
    payload(ids::login::clientbound::HELLO, |w| {
        w.put_string(server_id);
        w.put_byte_array(public_key_der);
        w.put_byte_array(challenge);
        w.put_bool(should_authenticate);
    })
}

/// Serverbound `key`: shared secret and verify token, both RSA-encrypted.
#[derive(Debug, Clone)]
pub struct LoginKey {
    pub shared_secret: Vec<u8>,
    pub challenge: Vec<u8>,
}
impl LoginKey {
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self { shared_secret: r.byte_array(256)?.to_vec(), challenge: r.byte_array(256)?.to_vec() })
    }
    pub fn encode(&self) -> Vec<u8> {
        payload(ids::login::serverbound::KEY, |w| {
            w.put_byte_array(&self.shared_secret);
            w.put_byte_array(&self.challenge);
        })
    }
}

pub fn login_compression(threshold: i32) -> Vec<u8> {
    payload(ids::login::clientbound::LOGIN_COMPRESSION, |w| w.put_varint(threshold))
}

#[derive(Debug, Clone)]
pub struct GameProfile {
    pub id: Uuid,
    pub name: String,
    /// (name, value, signature)
    pub properties: Vec<(String, String, Option<String>)>,
}

/// Clientbound `game_profile` (Login Success). In 1.21.1 it ends with the `strictErrorHandling` flag.
pub fn login_success(p: &GameProfile, strict_error_handling: bool) -> Vec<u8> {
    payload(ids::login::clientbound::GAME_PROFILE, |w| {
        w.put_uuid(p.id);
        w.put_string(&p.name);
        w.put_varint(p.properties.len() as i32);
        for (n, v, sig) in &p.properties {
            w.put_string(n);
            w.put_string(v);
            match sig {
                Some(s) => {
                    w.put_bool(true);
                    w.put_string(s);
                }
                None => w.put_bool(false),
            }
        }
        w.put_bool(strict_error_handling);
    })
}

// ----------------------------------------------------------- configuration

/// `minecraft:brand` as a `custom_payload`: channel + (VarInt-prefixed string with the server name).
pub fn config_brand(brand: &str) -> Vec<u8> {
    payload(ids::configuration::clientbound::CUSTOM_PAYLOAD, |w| {
        w.put_string("minecraft:brand");
        w.put_string(brand);
    })
}

pub fn config_update_enabled_features(features: &[&str]) -> Vec<u8> {
    payload(ids::configuration::clientbound::UPDATE_ENABLED_FEATURES, |w| {
        w.put_varint(features.len() as i32);
        for f in features {
            w.put_string(f);
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KnownPack {
    pub namespace: String,
    pub id: String,
    pub version: String,
}

pub fn config_select_known_packs(packs: &[KnownPack]) -> Vec<u8> {
    payload(ids::configuration::clientbound::SELECT_KNOWN_PACKS, |w| write_known_packs(w, packs))
}
fn write_known_packs(w: &mut Vec<u8>, packs: &[KnownPack]) {
    w.put_varint(packs.len() as i32);
    for p in packs {
        w.put_string(&p.namespace);
        w.put_string(&p.id);
        w.put_string(&p.version);
    }
}
/// Serverbound `select_known_packs` (limit of 64 entries like vanilla).
pub fn decode_known_packs(r: &mut Reader<'_>) -> Result<Vec<KnownPack>> {
    let n = r.varint()?;
    if !(0..=64).contains(&n) {
        return Err(crate::Error::Invalid("too many known packs"));
    }
    (0..n).map(|_| Ok(KnownPack { namespace: r.string(32767)?, id: r.string(32767)?, version: r.string(32767)? })).collect()
}
pub fn encode_known_packs_serverbound(packs: &[KnownPack]) -> Vec<u8> {
    payload(ids::configuration::serverbound::SELECT_KNOWN_PACKS, |w| write_known_packs(w, packs))
}

/// One registry entry: identifier + optional NBT data (none = the client uses its own from known packs).
pub struct RegistryEntry {
    pub id: String,
    pub data: Option<Tag>,
}

pub fn config_registry_data(registry: &str, entries: &[RegistryEntry]) -> Vec<u8> {
    payload(ids::configuration::clientbound::REGISTRY_DATA, |w| {
        w.put_string(registry);
        w.put_varint(entries.len() as i32);
        for e in entries {
            w.put_string(&e.id);
            match &e.data {
                Some(t) => {
                    w.put_bool(true);
                    t.write_network(w);
                }
                None => w.put_bool(false),
            }
        }
    })
}

/// `update_tags`: registry -> (tag -> list of IDs).
pub fn config_update_tags(tags: &[(String, Vec<(String, Vec<i32>)>)]) -> Vec<u8> {
    payload(ids::configuration::clientbound::UPDATE_TAGS, |w| write_tags(w, tags))
}
pub fn write_tags(w: &mut Vec<u8>, tags: &[(String, Vec<(String, Vec<i32>)>)]) {
    w.put_varint(tags.len() as i32);
    for (reg, list) in tags {
        w.put_string(reg);
        w.put_varint(list.len() as i32);
        for (name, ids) in list {
            w.put_string(name);
            w.put_varint(ids.len() as i32);
            for id in ids {
                w.put_varint(*id);
            }
        }
    }
}

pub fn config_finish() -> Vec<u8> {
    payload(ids::configuration::clientbound::FINISH_CONFIGURATION, |_| {})
}
pub fn config_keep_alive(id: i64) -> Vec<u8> {
    payload(ids::configuration::clientbound::KEEP_ALIVE, |w| w.put_i64(id))
}
pub fn config_disconnect(reason: &crate::text::Text) -> Vec<u8> {
    payload(ids::configuration::clientbound::DISCONNECT, |w| reason.to_nbt().write_network(w))
}

/// Serverbound `client_information` (shared by configuration and play).
#[derive(Debug, Clone)]
pub struct ClientInformation {
    pub language: String,
    pub view_distance: i8,
    pub chat_mode: i32,
    pub chat_colors: bool,
    pub model_customisation: u8,
    pub main_hand: i32,
    pub text_filtering: bool,
    pub allows_listing: bool,
}
impl ClientInformation {
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            language: r.string(16)?,
            view_distance: r.i8()?,
            chat_mode: r.varint()?,
            chat_colors: r.bool()?,
            model_customisation: r.u8()?,
            main_hand: r.varint()?,
            text_filtering: r.bool()?,
            allows_listing: r.bool()?,
        })
    }
    pub fn encode(&self, id: i32) -> Vec<u8> {
        payload(id, |w| {
            w.put_string(&self.language);
            w.put_i8(self.view_distance);
            w.put_varint(self.chat_mode);
            w.put_bool(self.chat_colors);
            w.put_u8(self.model_customisation);
            w.put_varint(self.main_hand);
            w.put_bool(self.text_filtering);
            w.put_bool(self.allows_listing);
        })
    }
}

impl Default for ClientInformation {
    fn default() -> Self {
        Self {
            language: "en_us".into(),
            view_distance: 2,
            chat_mode: 0,
            chat_colors: true,
            model_customisation: 0,
            main_hand: 1,
            text_filtering: false,
            allows_listing: true,
        }
    }
}

/// Serverbound `finish_configuration` (klient potwierdza koniec fazy configuration).
pub fn config_finish_serverbound() -> Vec<u8> {
    payload(ids::configuration::serverbound::FINISH_CONFIGURATION, |_| {})
}
