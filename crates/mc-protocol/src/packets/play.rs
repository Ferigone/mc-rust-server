//! Pakiety stanu `play` (1.21.1). Układ pól odtworzony z zdekompilowanych klas
//! `net.minecraft.network.protocol.game.*`. Wszystkie funkcje zwracają ładunek `VarInt(id) | body`.

use crate::buf::{Put, Reader};
use crate::error::Result;
use crate::frame::payload;
use crate::ids::play::{clientbound as cb, serverbound as sb};
use crate::nbt::{Compound, Tag};
use crate::text::Text;
use uuid::Uuid;

// ============================================================ clientbound

/// Parametry pakietu Login (`ClientboundLoginPacket` + `CommonPlayerSpawnInfo`).
pub struct LoginInfo<'a> {
    pub entity_id: i32,
    pub hardcore: bool,
    pub dimensions: &'a [&'a str],
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub reduced_debug_info: bool,
    pub show_death_screen: bool,
    pub do_limited_crafting: bool,
    /// ID typu wymiaru w rejestrze `minecraft:dimension_type` (= indeks wpisu).
    pub dimension_type: i32,
    pub dimension: &'a str,
    pub hashed_seed: i64,
    pub game_mode: u8,
    pub previous_game_mode: i8,
    pub is_debug: bool,
    pub is_flat: bool,
    /// (wymiar, x, y, z)
    pub last_death: Option<(&'a str, i32, i32, i32)>,
    pub portal_cooldown: i32,
    pub enforces_secure_chat: bool,
}

pub fn login(i: &LoginInfo<'_>) -> Vec<u8> {
    payload(cb::LOGIN, |w| {
        w.put_i32(i.entity_id);
        w.put_bool(i.hardcore);
        w.put_varint(i.dimensions.len() as i32);
        for d in i.dimensions {
            w.put_string(d);
        }
        w.put_varint(i.max_players);
        w.put_varint(i.view_distance);
        w.put_varint(i.simulation_distance);
        w.put_bool(i.reduced_debug_info);
        w.put_bool(i.show_death_screen);
        w.put_bool(i.do_limited_crafting);
        // CommonPlayerSpawnInfo
        w.put_varint(i.dimension_type);
        w.put_string(i.dimension);
        w.put_i64(i.hashed_seed);
        w.put_u8(i.game_mode);
        w.put_i8(i.previous_game_mode);
        w.put_bool(i.is_debug);
        w.put_bool(i.is_flat);
        match i.last_death {
            Some((dim, x, y, z)) => {
                w.put_bool(true);
                w.put_string(dim);
                w.put_block_pos(x, y, z);
            }
            None => w.put_bool(false),
        }
        w.put_varint(i.portal_cooldown);
        w.put_bool(i.enforces_secure_chat);
    })
}

pub fn change_difficulty(difficulty: u8, locked: bool) -> Vec<u8> {
    payload(cb::CHANGE_DIFFICULTY, |w| {
        w.put_u8(difficulty);
        w.put_bool(locked);
    })
}

pub struct Abilities {
    pub invulnerable: bool,
    pub flying: bool,
    pub may_fly: bool,
    pub instabuild: bool,
    pub flying_speed: f32,
    pub walking_speed: f32,
}
pub fn player_abilities(a: &Abilities) -> Vec<u8> {
    payload(cb::PLAYER_ABILITIES, |w| {
        let flags = (a.invulnerable as u8) | (a.flying as u8) << 1 | (a.may_fly as u8) << 2 | (a.instabuild as u8) << 3;
        w.put_u8(flags);
        w.put_f32(a.flying_speed);
        w.put_f32(a.walking_speed);
    })
}

pub fn set_carried_item(slot: i8) -> Vec<u8> {
    payload(cb::SET_CARRIED_ITEM, |w| w.put_i8(slot))
}

/// `ClientboundUpdateRecipesPacket` z pustą listą (pełna lista receptur: roadmap).
pub fn update_recipes_empty() -> Vec<u8> {
    payload(cb::UPDATE_RECIPES, |w| w.put_varint(0))
}

pub fn entity_event(entity_id: i32, event: i8) -> Vec<u8> {
    payload(cb::ENTITY_EVENT, |w| {
        w.put_i32(entity_id);
        w.put_i8(event);
    })
}

/// `ClientboundPlayerPositionPacket`: pozycja, obrót, maska względna, id teleportu.
pub fn player_position(x: f64, y: f64, z: f64, yaw: f32, pitch: f32, relative: u8, teleport_id: i32) -> Vec<u8> {
    payload(cb::PLAYER_POSITION, |w| {
        w.put_f64(x);
        w.put_f64(y);
        w.put_f64(z);
        w.put_f32(yaw);
        w.put_f32(pitch);
        w.put_u8(relative);
        w.put_varint(teleport_id);
    })
}

pub mod game_event {
    pub const NO_RESPAWN_BLOCK_AVAILABLE: u8 = 0;
    pub const START_RAINING: u8 = 1;
    pub const STOP_RAINING: u8 = 2;
    pub const CHANGE_GAME_MODE: u8 = 3;
    pub const WIN_GAME: u8 = 4;
    pub const DEMO_EVENT: u8 = 5;
    pub const ARROW_HIT_PLAYER: u8 = 6;
    pub const RAIN_LEVEL_CHANGE: u8 = 7;
    pub const THUNDER_LEVEL_CHANGE: u8 = 8;
    pub const PUFFER_FISH_STING: u8 = 9;
    pub const GUARDIAN_ELDER_EFFECT: u8 = 10;
    pub const IMMEDIATE_RESPAWN: u8 = 11;
    pub const LIMITED_CRAFTING: u8 = 12;
    pub const LEVEL_CHUNKS_LOAD_START: u8 = 13;
}
pub fn game_event(event: u8, param: f32) -> Vec<u8> {
    payload(cb::GAME_EVENT, |w| {
        w.put_u8(event);
        w.put_f32(param);
    })
}

pub fn set_chunk_cache_center(x: i32, z: i32) -> Vec<u8> {
    payload(cb::SET_CHUNK_CACHE_CENTER, |w| {
        w.put_varint(x);
        w.put_varint(z);
    })
}
pub fn set_chunk_cache_radius(r: i32) -> Vec<u8> {
    payload(cb::SET_CHUNK_CACHE_RADIUS, |w| w.put_varint(r))
}
pub fn chunk_batch_start() -> Vec<u8> {
    payload(cb::CHUNK_BATCH_START, |_| {})
}
pub fn chunk_batch_finished(size: i32) -> Vec<u8> {
    payload(cb::CHUNK_BATCH_FINISHED, |w| w.put_varint(size))
}

/// `ChunkPos.toLong()`: x w niskich 32 bitach, z w wysokich.
pub fn chunk_pos_long(x: i32, z: i32) -> i64 {
    (x as u32 as i64) | ((z as u32 as i64) << 32)
}
pub fn forget_level_chunk(x: i32, z: i32) -> Vec<u8> {
    payload(cb::FORGET_LEVEL_CHUNK, |w| w.put_i64(chunk_pos_long(x, z)))
}

/// Dane światła jednego chunka (`ClientboundLightUpdatePacketData`).
#[derive(Default, Clone)]
pub struct LightData {
    pub sky_mask: Vec<u64>,
    pub block_mask: Vec<u64>,
    pub empty_sky_mask: Vec<u64>,
    pub empty_block_mask: Vec<u64>,
    /// Każda tablica ma dokładnie 2048 bajtów.
    pub sky: Vec<[u8; 2048]>,
    pub block: Vec<[u8; 2048]>,
}

fn put_bitset(w: &mut Vec<u8>, bits: &[u64]) {
    // BitSet.toLongArray() nie zawiera zer na końcu; writeLongArray => VarInt len + longi
    let mut n = bits.len();
    while n > 0 && bits[n - 1] == 0 {
        n -= 1;
    }
    w.put_varint(n as i32);
    for b in &bits[..n] {
        w.put_i64(*b as i64);
    }
}

pub struct BlockEntityInfo {
    pub packed_xz: u8,
    pub y: i16,
    pub type_id: i32,
    pub tag: Option<Tag>,
}

/// `ClientboundLevelChunkWithLightPacket`. `heightmaps` = korzeń NBT, `sections` = surowe bajty sekcji.
pub fn level_chunk_with_light(x: i32, z: i32, heightmaps: &Compound, sections: &[u8], block_entities: &[BlockEntityInfo], light: &LightData) -> Vec<u8> {
    let mut w = Vec::with_capacity(sections.len() + 8192);
    w.put_varint(cb::LEVEL_CHUNK_WITH_LIGHT);
    w.put_i32(x);
    w.put_i32(z);
    Tag::Compound(heightmaps.clone()).write_network(&mut w);
    w.put_varint(sections.len() as i32);
    w.put_bytes(sections);
    w.put_varint(block_entities.len() as i32);
    for be in block_entities {
        w.put_u8(be.packed_xz);
        w.put_i16(be.y);
        w.put_varint(be.type_id);
        match &be.tag {
            Some(t) => t.write_network(&mut w),
            None => w.put_u8(0), // TAG_End
        }
    }
    put_bitset(&mut w, &light.sky_mask);
    put_bitset(&mut w, &light.block_mask);
    put_bitset(&mut w, &light.empty_sky_mask);
    put_bitset(&mut w, &light.empty_block_mask);
    w.put_varint(light.sky.len() as i32);
    for a in &light.sky {
        w.put_varint(2048);
        w.put_bytes(a);
    }
    w.put_varint(light.block.len() as i32);
    for a in &light.block {
        w.put_varint(2048);
        w.put_bytes(a);
    }
    w
}

pub fn set_time(game_time: i64, day_time: i64, daylight_cycle: bool) -> Vec<u8> {
    payload(cb::SET_TIME, |w| {
        w.put_i64(game_time);
        // konstruktor vanilla: gdy cykl dnia wyłączony, wysyłany jest czas ujemny (min. -1)
        let dt = if daylight_cycle { day_time } else if day_time == 0 { -1 } else { -day_time };
        w.put_i64(dt);
    })
}

pub fn set_default_spawn_position(x: i32, y: i32, z: i32, angle: f32) -> Vec<u8> {
    payload(cb::SET_DEFAULT_SPAWN_POSITION, |w| {
        w.put_block_pos(x, y, z);
        w.put_f32(angle);
    })
}

pub struct Border {
    pub center_x: f64,
    pub center_z: f64,
    pub old_size: f64,
    pub new_size: f64,
    pub lerp_time: i64,
    pub absolute_max_size: i32,
    pub warning_blocks: i32,
    pub warning_time: i32,
}
impl Default for Border {
    fn default() -> Self {
        // WorldBorder.MAX_SIZE = 5.999997E7, absoluteMaxSize = 29999984, ostrzeżenia 5 bloków / 15 s
        Self { center_x: 0.0, center_z: 0.0, old_size: 5.999997E7, new_size: 5.999997E7, lerp_time: 0, absolute_max_size: 29999984, warning_blocks: 5, warning_time: 15 }
    }
}
pub fn initialize_border(b: &Border) -> Vec<u8> {
    payload(cb::INITIALIZE_BORDER, |w| {
        w.put_f64(b.center_x);
        w.put_f64(b.center_z);
        w.put_f64(b.old_size);
        w.put_f64(b.new_size);
        w.put_varlong(b.lerp_time);
        w.put_varint(b.absolute_max_size);
        w.put_varint(b.warning_blocks);
        w.put_varint(b.warning_time);
    })
}

pub fn keep_alive(id: i64) -> Vec<u8> {
    payload(cb::KEEP_ALIVE, |w| w.put_i64(id))
}

pub fn disconnect(reason: &Text) -> Vec<u8> {
    payload(cb::DISCONNECT, |w| reason.to_nbt().write_network(w))
}

pub fn system_chat(content: &Text, overlay: bool) -> Vec<u8> {
    payload(cb::SYSTEM_CHAT, |w| {
        content.to_nbt().write_network(w);
        w.put_bool(overlay);
    })
}

pub fn server_data(motd: &Text, icon: Option<&[u8]>) -> Vec<u8> {
    payload(cb::SERVER_DATA, |w| {
        motd.to_nbt().write_network(w);
        match icon {
            Some(b) => {
                w.put_bool(true);
                w.put_byte_array(b);
            }
            None => w.put_bool(false),
        }
    })
}

/// Wpis `player_info_update`. Akcje wysyłane są w ustalonej kolejności enumu.
pub struct PlayerInfoEntry<'a> {
    pub uuid: Uuid,
    pub name: &'a str,
    pub properties: &'a [(String, String, Option<String>)],
    pub game_mode: i32,
    pub listed: bool,
    pub latency: i32,
}
pub mod player_info_action {
    pub const ADD_PLAYER: u8 = 1 << 0;
    pub const INITIALIZE_CHAT: u8 = 1 << 1;
    pub const UPDATE_GAME_MODE: u8 = 1 << 2;
    pub const UPDATE_LISTED: u8 = 1 << 3;
    pub const UPDATE_LATENCY: u8 = 1 << 4;
    pub const UPDATE_DISPLAY_NAME: u8 = 1 << 5;
    /// `createPlayerInitializing`: ADD_PLAYER, INITIALIZE_CHAT, UPDATE_GAME_MODE, UPDATE_LISTED, UPDATE_LATENCY, UPDATE_DISPLAY_NAME
    pub const INITIALIZING: u8 = 0b11_1111;
}
/// Domyślnie `actions` = `INITIALIZING` (jak `ClientboundPlayerInfoUpdatePacket.createPlayerInitializing`).
pub fn player_info_update(actions: u8, entries: &[PlayerInfoEntry<'_>]) -> Vec<u8> {
    use player_info_action as a;
    payload(cb::PLAYER_INFO_UPDATE, |w| {
        w.put_u8(actions); // EnumSet jako stały BitSet: ceil(6/8) = 1 bajt
        w.put_varint(entries.len() as i32);
        for e in entries {
            w.put_uuid(e.uuid);
            if actions & a::ADD_PLAYER != 0 {
                w.put_string(e.name);
                w.put_varint(e.properties.len() as i32);
                for (n, v, sig) in e.properties {
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
            }
            if actions & a::INITIALIZE_CHAT != 0 {
                w.put_bool(false); // brak sesji czatu
            }
            if actions & a::UPDATE_GAME_MODE != 0 {
                w.put_varint(e.game_mode);
            }
            if actions & a::UPDATE_LISTED != 0 {
                w.put_bool(e.listed);
            }
            if actions & a::UPDATE_LATENCY != 0 {
                w.put_varint(e.latency);
            }
            if actions & a::UPDATE_DISPLAY_NAME != 0 {
                w.put_bool(false); // brak nazwy wyświetlanej
            }
        }
    })
}

pub fn player_info_remove(uuids: &[Uuid]) -> Vec<u8> {
    payload(cb::PLAYER_INFO_REMOVE, |w| {
        w.put_varint(uuids.len() as i32);
        for u in uuids {
            w.put_uuid(*u);
        }
    })
}

/// Kąt jako bajt: `(int)(deg * 256 / 360)` (zgodnie z `ClientboundAddEntityPacket`/`TeleportEntity`).
pub fn angle_byte(deg: f32) -> i8 {
    ((deg * 256.0 / 360.0) as i32) as i8
}

#[allow(clippy::too_many_arguments)]
pub fn add_entity(id: i32, uuid: Uuid, type_id: i32, x: f64, y: f64, z: f64, pitch: f32, yaw: f32, head_yaw: f32, data: i32, vx: i16, vy: i16, vz: i16) -> Vec<u8> {
    payload(cb::ADD_ENTITY, |w| {
        w.put_varint(id);
        w.put_uuid(uuid);
        w.put_varint(type_id);
        w.put_f64(x);
        w.put_f64(y);
        w.put_f64(z);
        w.put_i8(angle_byte(pitch));
        w.put_i8(angle_byte(yaw));
        w.put_i8(angle_byte(head_yaw));
        w.put_varint(data);
        w.put_i16(vx);
        w.put_i16(vy);
        w.put_i16(vz);
    })
}

pub fn remove_entities(ids: &[i32]) -> Vec<u8> {
    payload(cb::REMOVE_ENTITIES, |w| {
        w.put_varint(ids.len() as i32);
        for i in ids {
            w.put_varint(*i);
        }
    })
}

/// `ClientboundMoveEntityPacket.entityToPacket`: `lfloor(d * 4096)`.
pub fn entity_to_packet(v: f64) -> i64 {
    (v * 4096.0).floor() as i64
}

pub fn move_entity_pos(id: i32, dx: i16, dy: i16, dz: i16, on_ground: bool) -> Vec<u8> {
    payload(cb::MOVE_ENTITY_POS, |w| {
        w.put_varint(id);
        w.put_i16(dx);
        w.put_i16(dy);
        w.put_i16(dz);
        w.put_bool(on_ground);
    })
}
pub fn move_entity_pos_rot(id: i32, dx: i16, dy: i16, dz: i16, yaw: f32, pitch: f32, on_ground: bool) -> Vec<u8> {
    payload(cb::MOVE_ENTITY_POS_ROT, |w| {
        w.put_varint(id);
        w.put_i16(dx);
        w.put_i16(dy);
        w.put_i16(dz);
        w.put_i8(angle_byte(yaw));
        w.put_i8(angle_byte(pitch));
        w.put_bool(on_ground);
    })
}
pub fn move_entity_rot(id: i32, yaw: f32, pitch: f32, on_ground: bool) -> Vec<u8> {
    payload(cb::MOVE_ENTITY_ROT, |w| {
        w.put_varint(id);
        w.put_i8(angle_byte(yaw));
        w.put_i8(angle_byte(pitch));
        w.put_bool(on_ground);
    })
}
pub fn teleport_entity(id: i32, x: f64, y: f64, z: f64, yaw: f32, pitch: f32, on_ground: bool) -> Vec<u8> {
    payload(cb::TELEPORT_ENTITY, |w| {
        w.put_varint(id);
        w.put_f64(x);
        w.put_f64(y);
        w.put_f64(z);
        w.put_i8(angle_byte(yaw));
        w.put_i8(angle_byte(pitch));
        w.put_bool(on_ground);
    })
}
pub fn rotate_head(id: i32, head_yaw: f32) -> Vec<u8> {
    payload(cb::ROTATE_HEAD, |w| {
        w.put_varint(id);
        w.put_i8(angle_byte(head_yaw));
    })
}

/// Wartość metadanych encji. Serializator: 0 = BYTE, 1 = INT, 3 = FLOAT, 8 = BOOLEAN (kolejność `EntityDataSerializers`).
pub enum EntityData {
    Byte(i8),
    Int(i32),
    Float(f32),
    Bool(bool),
}
pub fn set_entity_data(id: i32, values: &[(u8, EntityData)]) -> Vec<u8> {
    payload(cb::SET_ENTITY_DATA, |w| {
        w.put_varint(id);
        for (idx, v) in values {
            w.put_u8(*idx);
            match v {
                EntityData::Byte(b) => {
                    w.put_varint(0);
                    w.put_i8(*b);
                }
                EntityData::Int(i) => {
                    w.put_varint(1);
                    w.put_varint(*i);
                }
                EntityData::Float(f) => {
                    w.put_varint(3);
                    w.put_f32(*f);
                }
                EntityData::Bool(b) => {
                    w.put_varint(8);
                    w.put_bool(*b);
                }
            }
        }
        w.put_u8(0xFF);
    })
}

pub fn block_update(x: i32, y: i32, z: i32, state: u32) -> Vec<u8> {
    payload(cb::BLOCK_UPDATE, |w| {
        w.put_block_pos(x, y, z);
        w.put_varint(state as i32);
    })
}
pub fn block_changed_ack(sequence: i32) -> Vec<u8> {
    payload(cb::BLOCK_CHANGED_ACK, |w| w.put_varint(sequence))
}

pub fn set_health(health: f32, food: i32, saturation: f32) -> Vec<u8> {
    payload(cb::SET_HEALTH, |w| {
        w.put_f32(health);
        w.put_varint(food);
        w.put_f32(saturation);
    })
}
pub fn set_experience(progress: f32, level: i32, total: i32) -> Vec<u8> {
    payload(cb::SET_EXPERIENCE, |w| {
        w.put_f32(progress);
        w.put_varint(level);
        w.put_varint(total);
    })
}

/// `ContainerSetContent` z pustymi slotami (`ItemStack.EMPTY` = VarInt 0).
pub fn container_set_content_empty(container_id: u8, state_id: i32, slots: usize) -> Vec<u8> {
    payload(cb::CONTAINER_SET_CONTENT, |w| {
        w.put_u8(container_id);
        w.put_varint(state_id);
        w.put_varint(slots as i32);
        for _ in 0..slots {
            w.put_varint(0);
        }
        w.put_varint(0); // przedmiot na kursorze
    })
}

/// Niepodpisana wiadomość gracza (`PlayerChatMessage.unsigned`): brak sygnatury, pusty `lastSeen`,
/// maska filtra PASS_THROUGH. `chat_type_id` = indeks wpisu w `minecraft:chat_type` (wysyłany jako id+1).
pub fn player_chat_unsigned(sender: Uuid, index: i32, content: &str, timestamp_ms: i64, chat_type_id: i32, sender_name: &Text) -> Vec<u8> {
    payload(cb::PLAYER_CHAT, |w| {
        w.put_uuid(sender);
        w.put_varint(index);
        w.put_bool(false); // brak sygnatury
        w.put_string(content); // SignedMessageBody.Packed.content (<=256)
        w.put_i64(timestamp_ms);
        w.put_i64(0); // salt
        w.put_varint(0); // lastSeen: 0 wpisów
        w.put_bool(false); // unsignedContent
        w.put_varint(0); // FilterMask.PASS_THROUGH
        w.put_varint(chat_type_id + 1); // Holder<ChatType>: id+1
        sender_name.to_nbt().write_network(w);
        w.put_bool(false); // targetName
    })
}

// ============================================================ serverbound

#[derive(Debug, Clone)]
pub enum Serverbound {
    AcceptTeleportation(i32),
    ChatCommand(String),
    /// Podpisane polecenie: bierzemy tylko tekst (sygnatur argumentów nie weryfikujemy).
    ChatCommandSigned(String),
    Chat(String),
    ChatSessionUpdate,
    ChunkBatchReceived(f32),
    ClientCommand(i32),
    ClientInformation(super::ClientInformation),
    ConfigurationAcknowledged,
    KeepAlive(i64),
    MovePos { x: f64, y: f64, z: f64, on_ground: bool },
    MovePosRot { x: f64, y: f64, z: f64, yaw: f32, pitch: f32, on_ground: bool },
    MoveRot { yaw: f32, pitch: f32, on_ground: bool },
    MoveStatusOnly { on_ground: bool },
    PlayerAbilities { flying: bool },
    PlayerAction { action: i32, x: i32, y: i32, z: i32, face: u8, sequence: i32 },
    PlayerCommand { entity_id: i32, action: i32, data: i32 },
    PlayerInput,
    Pong(i32),
    SetCarriedItem(i16),
    SetCreativeModeSlot { slot: i16, item: Option<(i32, i32)> },
    Swing { hand: i32 },
    UseItemOn { hand: i32, x: i32, y: i32, z: i32, face: i32, cursor: (f32, f32, f32), inside: bool, sequence: i32 },
    UseItem { hand: i32, sequence: i32, yaw: f32, pitch: f32 },
    CustomPayload { channel: String, data: Vec<u8> },
    /// Pakiet znany, ale (jeszcze) nieobsługiwany przez port.
    Unhandled(i32),
}

pub fn decode_serverbound(id: i32, body: &[u8]) -> Result<Serverbound> {
    let mut r = Reader::new(body);
    Ok(match id {
        sb::ACCEPT_TELEPORTATION => Serverbound::AcceptTeleportation(r.varint()?),
        sb::CHAT_COMMAND => Serverbound::ChatCommand(r.string(32767)?),
        sb::CHAT_COMMAND_SIGNED => Serverbound::ChatCommandSigned(r.string(32767)?),
        sb::CHAT => Serverbound::Chat(r.string(256)?),
        sb::CHAT_SESSION_UPDATE => Serverbound::ChatSessionUpdate,
        sb::CHUNK_BATCH_RECEIVED => Serverbound::ChunkBatchReceived(r.f32()?),
        sb::CLIENT_COMMAND => Serverbound::ClientCommand(r.varint()?),
        sb::CLIENT_INFORMATION => Serverbound::ClientInformation(super::ClientInformation::decode(&mut r)?),
        sb::CONFIGURATION_ACKNOWLEDGED => Serverbound::ConfigurationAcknowledged,
        sb::KEEP_ALIVE => Serverbound::KeepAlive(r.i64()?),
        sb::MOVE_PLAYER_POS => Serverbound::MovePos { x: r.f64()?, y: r.f64()?, z: r.f64()?, on_ground: r.u8()? != 0 },
        sb::MOVE_PLAYER_POS_ROT => Serverbound::MovePosRot { x: r.f64()?, y: r.f64()?, z: r.f64()?, yaw: r.f32()?, pitch: r.f32()?, on_ground: r.u8()? != 0 },
        sb::MOVE_PLAYER_ROT => Serverbound::MoveRot { yaw: r.f32()?, pitch: r.f32()?, on_ground: r.u8()? != 0 },
        sb::MOVE_PLAYER_STATUS_ONLY => Serverbound::MoveStatusOnly { on_ground: r.u8()? != 0 },
        sb::PLAYER_ABILITIES => Serverbound::PlayerAbilities { flying: r.u8()? & 2 != 0 },
        sb::PLAYER_ACTION => {
            let action = r.varint()?;
            let (x, y, z) = r.block_pos()?;
            Serverbound::PlayerAction { action, x, y, z, face: r.u8()?, sequence: r.varint()? }
        }
        sb::PLAYER_COMMAND => Serverbound::PlayerCommand { entity_id: r.varint()?, action: r.varint()?, data: r.varint()? },
        sb::PLAYER_INPUT => Serverbound::PlayerInput,
        sb::PONG => Serverbound::Pong(r.i32()?),
        sb::SET_CARRIED_ITEM => Serverbound::SetCarriedItem(r.i16()?),
        sb::SET_CREATIVE_MODE_SLOT => {
            let slot = r.i16()?;
            let count = r.varint()?;
            let item = if count <= 0 { None } else { Some((r.varint()?, count)) };
            // Komponenty (DataComponentPatch) pomijamy: wymagają dekodera każdego typu komponentu.
            Serverbound::SetCreativeModeSlot { slot, item }
        }
        sb::SWING => Serverbound::Swing { hand: r.varint()? },
        sb::USE_ITEM_ON => {
            let hand = r.varint()?;
            let (x, y, z) = r.block_pos()?;
            let face = r.varint()?;
            let cursor = (r.f32()?, r.f32()?, r.f32()?);
            Serverbound::UseItemOn { hand, x, y, z, face, cursor, inside: r.bool()?, sequence: r.varint()? }
        }
        sb::USE_ITEM => Serverbound::UseItem { hand: r.varint()?, sequence: r.varint()?, yaw: r.f32()?, pitch: r.f32()? },
        sb::CUSTOM_PAYLOAD => Serverbound::CustomPayload { channel: r.identifier()?, data: r.rest().to_vec() },
        other => Serverbound::Unhandled(other),
    })
}
