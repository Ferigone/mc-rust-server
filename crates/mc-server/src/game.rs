//! Game thread: a single 20 TPS loop (like the vanilla main thread) processing network events,
//! player state, chunk/entity tracking and packet sending.
//!
//! The join sequence is reconstructed from `PlayerList.placeNewPlayer`, chunk sending from
//! `PlayerChunkSender`, the chunk view from `ChunkTrackingView`, entity movement from `ServerEntity.sendChanges`.

use crate::net::{GameEvent, JoinRequest, PlayerSender, Shared, VERSION_NAME};
use crate::world::World;
use mc_data::GameData;
use mc_protocol::frame::FrameEncoder;
use mc_protocol::nbt::{Compound, Tag};
use std::path::PathBuf;
use mc_protocol::packets::play::{self as pk, Abilities, Border, EntityData, PlayerInfoEntry, Serverbound};
use mc_protocol::packets::{ClientInformation, GameProfile};
use mc_protocol::text::Text;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};
use uuid::Uuid;

const TICK: Duration = Duration::from_millis(50);
const KEEPALIVE_INTERVAL_MS: u128 = 15_000;
const MAX_PENDING_BYTES: usize = 32 * 1024 * 1024;

/// IDs from the official registries needed during the game.
pub struct Ids {
    pub dimension_type: i32,
    pub player_entity_type: i32,
    pub chat_type: i32,
    pub overworld: &'static str,
    pub attr_movement_speed: i32,
    pub attr_entity_range: i32,
    pub attr_block_range: i32,
}

/// `PlayerChunkSender`.
struct ChunkSender {
    pending: HashSet<(i32, i32)>,
    desired_per_tick: f32,
    batch_quota: f32,
    unacked: i32,
    max_unacked: i32,
}
impl ChunkSender {
    fn new() -> Self {
        Self { pending: HashSet::new(), desired_per_tick: 9.0, batch_quota: 0.0, unacked: 0, max_unacked: 1 }
    }
}

/// `ChunkTrackingView.isWithinDistance`.
fn within_distance(cx: i32, cz: i32, view: i32, x: i32, z: i32, include_outer: bool) -> bool {
    let dx = ((x - cx).abs() - 1).max(0) as i64;
    let dz = ((z - cz).abs() - 1).max(0) as i64;
    let a = (dx.max(dz) - if include_outer { 1 } else { 0 }).max(0);
    let b = dx.min(dz);
    b * b + a * a < (view as i64) * (view as i64)
}

#[derive(Clone, Copy, PartialEq)]
struct View {
    cx: i32,
    cz: i32,
    dist: i32,
}
impl View {
    fn contains(&self, x: i32, z: i32) -> bool {
        within_distance(self.cx, self.cz, self.dist, x, z, true)
    }
    fn bounds(&self) -> (i32, i32, i32, i32) {
        (self.cx - self.dist - 1, self.cz - self.dist - 1, self.cx + self.dist + 1, self.cz + self.dist + 1)
    }
}

struct Player {
    #[allow(dead_code)]
    session: u64,
    entity_id: i32,
    uuid: Uuid,
    name: String,
    properties: Vec<(String, String, Option<String>)>,
    sender: PlayerSender,
    info: ClientInformation,
    game_mode: u8,
    // pozycja
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
    // teleport
    teleport_counter: i32,
    awaiting: Option<(i32, f64, f64, f64, u64)>,
    // keepalive
    ka_pending: bool,
    ka_time: Instant,
    ka_challenge: i64,
    latency: i32,
    // chunki
    view: Option<View>,
    sender_state: ChunkSender,
    // visible entities (sessions of other players)
    tracked: HashSet<u64>,
    // state sent to observers (ServerEntity)
    base: (f64, f64, f64),
    last_yaw: i32,
    last_pitch: i32,
    last_head: i32,
    was_on_ground: bool,
    teleport_delay: i32,
    ticks: u64,
    // inventory (simplified): [menu slot] -> (item id, count)
    inventory: Vec<Option<(i32, i32)>>,
    selected: i8,
    first_tick_done: bool,
    /// Synchronized attribute instances (movement speed, block range, entity range): created / changed since the last sync.
    attr_created: [bool; 3],
    attr_dirty: [bool; 3],
    /// The creative-mode range modifiers are currently applied (`ServerPlayer.updatePlayerAttributes`).
    creative_mods: bool,
    /// Base value of the movement speed attribute (the abilities' walking speed).
    walk_speed: f64,
    /// Highest sequence to acknowledge at the start of the next tick (`ackBlockChangesUpTo`), -1 = none.
    ack_up_to: i32,
    /// Stan ekwipunku znany klientowi (`remoteSlots`) i licznik `stateId` menu.
    remote_inv: Vec<Option<(i32, i32)>>,
    state_id: i32,
    sneaking: bool,
    sprinting: bool,
    message_index: i32,
    /// The saved player compound (the base for saving; unknown fields round-trip).
    data: Compound,
    health: f32,
    food: i32,
    saturation: f32,
    xp_progress: f32,
    xp_level: i32,
    xp_total: i32,
    /// Stored inventory compounds per menu slot and the (id, count) they were loaded as.
    inv_raw: Vec<Option<Compound>>,
    inv_loaded: Vec<Option<(i32, i32)>>,
}

/// Game rules from `level.dat` that the join sequence and the tick loop depend on.
#[derive(Clone, Copy)]
struct Rules {
    daylight_cycle: bool,
    reduced_debug_info: bool,
    immediate_respawn: bool,
    limited_crafting: bool,
    spawn_radius: i32,
}

pub struct Game {
    shared: Arc<Shared>,
    data: Arc<GameData>,
    ids: Ids,
    world: World,
    enc: FrameEncoder,
    players: BTreeMap<u64, Player>,
    next_entity_id: i32,
    tick: u64,
    /// Blocks changed in this tick by section (chunk x, chunk z, section y) -> local positions.
    dirty: BTreeMap<(i32, i32, i32), std::collections::BTreeSet<(u8, u8, u8)>>,
    game_time: i64,
    day_time: i64,
    stopping: bool,
    rules: Rules,
    player_dir: PathBuf,
}

fn pos_enc(v: f64) -> i64 {
    pk::entity_to_packet(v)
}

impl Game {
    pub fn new(shared: Arc<Shared>, data: Arc<GameData>, ids: Ids, world: World) -> Self {
        let enc = FrameEncoder::new(if shared.settings.compression_threshold >= 0 { Some(shared.settings.compression_threshold) } else { None });
        let (game_time, day_time, rules) = match &world.level {
            Some(l) => (
                l.game_time(),
                l.day_time(),
                Rules {
                    daylight_cycle: l.rule_bool("doDaylightCycle", true),
                    reduced_debug_info: l.rule_bool("reducedDebugInfo", false),
                    immediate_respawn: l.rule_bool("doImmediateRespawn", false),
                    limited_crafting: l.rule_bool("doLimitedCrafting", false),
                    spawn_radius: l.rule_int("spawnRadius", 10),
                },
            ),
            None => (0, 0, Rules { daylight_cycle: true, reduced_debug_info: false, immediate_respawn: false, limited_crafting: false, spawn_radius: 10 }),
        };
        let player_dir = PathBuf::from(&shared.settings.level_name).join("playerdata");
        Self { shared, data, ids, world, enc, players: BTreeMap::new(), next_entity_id: 1, tick: 0, dirty: BTreeMap::new(), game_time, day_time, stopping: false, rules, player_dir }
    }

    pub fn run(mut self, rx: Receiver<GameEvent>) {
        let mut next = Instant::now() + TICK;
        info!("Game loop started (20 TPS)");
        'main: loop {
            loop {
                let timeout = next.saturating_duration_since(Instant::now());
                match rx.recv_timeout(timeout) {
                    Ok(GameEvent::Shutdown) => break 'main,
                    Ok(ev) => self.handle_event(ev),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => break 'main,
                }
            }
            self.do_tick();
            next += TICK;
            let now = Instant::now();
            if now > next + Duration::from_millis(2000) {
                warn!("Can't keep up! Skipping {} ms", (now - next).as_millis());
                next = now;
            }
            if self.stopping {
                break;
            }
        }
        // shutdown: disconnect everybody
        let sessions: Vec<u64> = self.players.keys().copied().collect();
        for s in sessions {
            self.kick(s, Text::translatable("multiplayer.disconnect.server_shutdown"));
        }
        let saved = self.world.save(self.game_time, self.day_time);
        info!("World saved ({saved} chunks)");
    }

    // ------------------------------------------------------------ sending

    fn frame(&mut self, payload: &[u8]) -> Arc<Vec<u8>> {
        Arc::new(self.enc.encode(payload))
    }
    fn send(&mut self, session: u64, payload: &[u8]) {
        let f = self.frame(payload);
        if let Some(p) = self.players.get(&session) {
            p.sender.send(f);
        }
    }
    fn broadcast(&mut self, payload: &[u8], except: Option<u64>) {
        let f = self.frame(payload);
        for (s, p) in &self.players {
            if Some(*s) != except {
                p.sender.send(f.clone());
            }
        }
    }
    fn kick(&mut self, session: u64, reason: Text) {
        let f = self.frame(&pk::disconnect(&reason));
        if let Some(p) = self.players.get(&session) {
            info!("{} was disconnected: {}", p.name, text_plain(&reason));
            p.sender.close(Some(f));
        }
        self.remove_player(session);
    }

    // ------------------------------------------------------------ events

    fn handle_event(&mut self, ev: GameEvent) {
        match ev {
            GameEvent::Join(req) => self.on_join(*req),
            GameEvent::Packets { session, packets } => {
                for packet in packets {
                    if !self.players.contains_key(&session) {
                        break; // kicked while handling an earlier packet of the same read
                    }
                    self.on_packet(session, packet);
                }
            }
            GameEvent::Disconnected { session, reason } => {
                if self.players.contains_key(&session) {
                    let name = self.players[&session].name.clone();
                    info!("{name} lost connection: {reason}");
                    self.remove_player(session);
                }
            }
            GameEvent::Shutdown => self.stopping = true,
        }
    }

    fn display_name(p: &Player) -> Text {
        // Player.getDisplayName: the name with click (/tell), hover tooltip (show_entity) and insertion.
        let u = p.uuid.as_u128();
        let id = Tag::IntArray(vec![(u >> 96) as i32, (u >> 64) as i32, (u >> 32) as i32, u as i32]);
        let mut t = Text::plain(p.name.clone());
        t.style_nbt.push(("clickEvent".into(), Tag::Compound(Compound::new().with("action", Tag::string("suggest_command")).with("value", Tag::string(format!("/tell {} ", p.name))))));
        t.style_nbt.push((
            "hoverEvent".into(),
            Tag::Compound(
                Compound::new().with("action", Tag::string("show_entity")).with(
                    "contents",
                    Tag::Compound(Compound::new().with("type", Tag::string("minecraft:player")).with("id", id).with("name", Tag::string(p.name.clone()))),
                ),
            ),
        ));
        t.style_nbt.push(("insertion".into(), Tag::string(p.name.clone())));
        t
    }

    fn info_entry<'a>(p: &'a Player) -> PlayerInfoEntry<'a> {
        PlayerInfoEntry { uuid: p.uuid, name: &p.name, properties: &p.properties, game_mode: p.game_mode as i32, listed: true, latency: p.latency }
    }

    fn sync_status(&mut self) {
        self.shared.online.store(self.players.len() as i32, Ordering::Relaxed);
        let mut st = self.shared.status.lock().unwrap();
        st.sample = self.players.values().map(|p| (p.name.clone(), p.uuid)).collect();
    }

    // ------------------------------------------------------------ joining

    fn on_join(&mut self, req: JoinRequest) {
        let JoinRequest { session, profile, client_info, sender, addr } = req;
        let GameProfile { id: uuid, name, properties } = profile;

        // "Logged in from another location": the old session is kicked.
        let dup: Vec<u64> = self.players.iter().filter(|(_, p)| p.uuid == uuid).map(|(s, _)| *s).collect();
        for s in dup {
            self.kick(s, Text::translatable("multiplayer.disconnect.duplicate_login"));
        }
        if self.players.len() as i32 >= self.shared.settings.max_players {
            let f = self.frame(&pk::disconnect(&Text::translatable("multiplayer.disconnect.server_full")));
            sender.close(Some(f));
            return;
        }

        let entity_id = self.next_entity_id;
        self.next_entity_id += 1;
        let default_gm = self.default_game_mode();
        let saved = crate::playerdata::load(&self.player_dir, uuid);
        let returning = saved.is_some();
        let force_gm = self.shared.settings.force_game_mode;
        let (x, y, z, yaw, pitch, gm, data) = match saved {
            Some(d) => {
                let pos: Vec<f64> = d.get_list("Pos").map(|l| l.iter().filter_map(Tag::as_f64).collect()).unwrap_or_default();
                let rot: Vec<f64> = d.get_list("Rotation").map(|l| l.iter().filter_map(Tag::as_f64).collect()).unwrap_or_default();
                let gm = if force_gm { default_gm } else { d.get_i32("playerGameType").unwrap_or(default_gm as i32) as u8 };
                if pos.len() == 3 {
                    (pos[0], pos[1], pos[2], rot.first().copied().unwrap_or(0.0) as f32, rot.get(1).copied().unwrap_or(0.0) as f32, gm, d)
                } else {
                    let (px, py, pz) = self.adjust_spawn_location();
                    (px as f64 + 0.5, py as f64, pz as f64 + 0.5, 0.0, 0.0, gm, d)
                }
            }
            None => {
                let (px, py, pz) = self.adjust_spawn_location();
                let (x, y, z) = (px as f64 + 0.5, py as f64, pz as f64 + 0.5);
                (x, y, z, 0.0, 0.0, default_gm, crate::playerdata::new_player(uuid, (x, y, z), (0.0, 0.0), default_gm as i32))
            }
        };

        info!("{name}[{addr}] logged in with entity id {entity_id} at ({x:.1}, {y:.1}, {z:.1})");
        let (inventory, inv_raw) = self.load_inventory(&data);
        let selected = data.get_i32("SelectedItemSlot").unwrap_or(0).clamp(0, 8) as i8;
        let p = Player {
            session,
            entity_id,
            uuid,
            name: name.clone(),
            properties,
            sender,
            info: client_info,
            game_mode: gm,
            x,
            y,
            z,
            yaw,
            pitch,
            on_ground: data.get_bool("OnGround").unwrap_or(false),
            teleport_counter: 0,
            awaiting: None,
            ka_pending: false,
            ka_time: Instant::now(),
            ka_challenge: 0,
            latency: 0,
            view: None,
            sender_state: ChunkSender::new(),
            tracked: HashSet::new(),
            base: (x, y, z),
            last_yaw: 0,
            last_pitch: 0,
            last_head: 0,
            was_on_ground: false,
            teleport_delay: 0,
            ticks: 0,
            inv_loaded: inventory.clone(),
            inventory,
            selected,
            first_tick_done: false,
            // A new player has no attribute instances yet (the first tick creates them and syncs the dirty ones); a player
            // loaded from disk already has the movement speed set by the abilities.
            attr_created: [returning, false, false],
            attr_dirty: [!returning, false, false],
            creative_mods: false,
            walk_speed: abilities_from(&data, gm).walking_speed as f64,
            ack_up_to: -1,
            remote_inv: vec![None; 46],
            state_id: 1,
            sneaking: false,
            sprinting: false,
            message_index: 0,
            health: data.get_f64("Health").unwrap_or(20.0) as f32,
            food: data.get_i32("foodLevel").unwrap_or(20),
            // FoodData.readAdditionalSaveData: when foodLevel is stored the other fields default to 0, otherwise 20 / 5.0
            saturation: if data.get_i32("foodLevel").is_some() { data.get_f64("foodSaturationLevel").unwrap_or(0.0) as f32 } else { 5.0 },
            xp_progress: data.get_f64("XpP").unwrap_or(0.0) as f32,
            xp_level: data.get_i32("XpLevel").unwrap_or(0),
            xp_total: data.get_i32("XpTotal").unwrap_or(0),
            inv_raw,
            data,
        };
        self.players.insert(session, p);

        let s = &self.shared.settings;
        let (hardcore, is_flat) = (self.world.level.as_ref().map(|l| l.hardcore()).unwrap_or(s.hardcore), self.world.is_flat());
        // 1. Login
        // The set of dimension keys like in vanilla (`server.levelKeys()`), in the order observed on the real server.
        let dims = [self.ids.overworld, "minecraft:the_end", "minecraft:the_nether"];
        let login = pk::login(&pk::LoginInfo {
            entity_id,
            hardcore,
            dimensions: &dims,
            max_players: s.max_players,
            view_distance: s.view_distance,
            simulation_distance: s.simulation_distance,
            reduced_debug_info: self.rules.reduced_debug_info,
            show_death_screen: !self.rules.immediate_respawn,
            do_limited_crafting: self.rules.limited_crafting,
            dimension_type: self.ids.dimension_type,
            dimension: self.ids.overworld,
            hashed_seed: self.world.hashed_seed(),
            game_mode: gm,
            previous_game_mode: -1,
            is_debug: false,
            is_flat,
            last_death: None,
            portal_cooldown: 0,
            enforces_secure_chat: crate::net::ENFORCES_SECURE_CHAT,
        });
        let (difficulty, locked) = self.world.level.as_ref().map(|l| (l.difficulty(), l.difficulty_locked())).unwrap_or((s.difficulty, false));
        let motd = Text::plain(s.motd.clone());
        self.send(session, &login);
        // 2-5. Difficulty, abilities, selected slot, recipes
        self.send(session, &pk::change_difficulty(difficulty, locked));
        let ab = abilities_from(&self.players[&session].data, gm);
        self.send(session, &pk::player_abilities(&ab));
        self.send(session, &pk::set_carried_item(selected));
        self.send(session, &pk::update_recipes_empty());
        // 6. Permission level (entity event 24 + level)
        self.send(session, &pk::entity_event(entity_id, 24));
        // 8. Join message to the players already on the list (the new player is not on it yet)
        let dn = Self::display_name(&self.players[&session]);
        let join_msg = Text::translatable("multiplayer.player.joined").with_args(vec![dn]).color("yellow");
        let f = pk::system_chat(&join_msg, false);
        self.broadcast(&f, Some(session));
        // 9. Teleport (start position)
        self.teleport(session, x, y, z, yaw, pitch);
        // 10. Server data
        self.send(session, &pk::server_data(&motd, None));
        // 11. Player list: existing -> new
        let others: Vec<u64> = self.players.keys().copied().filter(|k| *k != session).collect();
        // vanilla always sends this packet, even with an empty list (createPlayerInitializing(this.players))
        let entries: Vec<PlayerInfoEntry<'_>> = others.iter().map(|k| Self::info_entry(&self.players[k])).collect();
        let payload = pk::player_info_update(pk::player_info_action::INITIALIZING, &entries);
        self.send(session, &payload);
        // 12. new -> everybody (including itself)
        let payload = pk::player_info_update(pk::player_info_action::INITIALIZING, &[Self::info_entry(&self.players[&session])]);
        self.broadcast(&payload, None);
        // 13. sendLevelInfo
        let border = self.border();
        self.send(session, &pk::initialize_border(&border));
        self.send(session, &pk::set_time(self.game_time, self.day_time, self.rules.daylight_cycle));
        let (sx, sy, sz) = self.world.spawn;
        let angle = self.world.level.as_ref().map(|l| l.spawn_angle()).unwrap_or(0.0);
        self.send(session, &pk::set_default_spawn_position(sx, sy, sz, angle));
        let weather = self.world.level.as_ref().map(|l| (l.raining(), l.thundering()));
        if let Some((true, thundering)) = weather {
            // the rain level of a freshly loaded level is 1.0 (thunder 1.0 only when thundering)
            self.send(session, &pk::game_event(pk::game_event::START_RAINING, 0.0));
            self.send(session, &pk::game_event(pk::game_event::RAIN_LEVEL_CHANGE, 1.0));
            self.send(session, &pk::game_event(pk::game_event::THUNDER_LEVEL_CHANGE, if thundering { 1.0 } else { 0.0 }));
        }
        self.send(session, &pk::game_event(pk::game_event::LEVEL_CHUNKS_LOAD_START, 0.0));
        // tickRateManager().updateJoiningPlayer: tick rate and step
        self.send(session, &pk::ticking_state(20.0, false));
        self.send(session, &pk::ticking_step(0));
        // 14. addNewPlayer: chunk and entity tracking
        self.update_chunk_tracking(session);
        self.update_entity_tracking(session);
        // 15. initInventoryMenu
        let items = self.players[&session].inventory.clone();
        self.players.get_mut(&session).unwrap().remote_inv = items.clone();
        self.send(session, &pk::container_set_content(0, 1, &items, None));
        // the player's own entity data (sendPairingData for the player itself)
        let data = Self::entity_data_values(&self.players[&session]);
        self.send(session, &pk::set_entity_data(entity_id, &data));
        // the attribute instances that already exist (a player loaded from disk has the movement speed)
        let existing: Vec<usize> = (0..3).filter(|&k| self.players[&session].attr_created[k]).collect();
        if let Some(payload) = self.attributes_packet(&self.players[&session], &existing) {
            self.send(session, &payload);
        }
        self.sync_status();
    }

    fn default_game_mode(&self) -> u8 {
        self.world.level.as_ref().map(|l| l.game_type() as u8).unwrap_or(self.shared.settings.game_mode)
    }

    /// The world border from `level.dat` (`WorldBorder` serialization), defaulting to the maximum size.
    fn border(&self) -> Border {
        let mut b = Border::default();
        if let Some(l) = &self.world.level {
            let d = l.data();
            b.center_x = d.get_f64("BorderCenterX").unwrap_or(0.0);
            b.center_z = d.get_f64("BorderCenterZ").unwrap_or(0.0);
            let size = d.get_f64("BorderSize").unwrap_or(b.old_size);
            b.old_size = size;
            b.new_size = d.get_f64("BorderSizeLerpTarget").unwrap_or(size);
            b.lerp_time = d.get_i64("BorderSizeLerpTime").unwrap_or(0);
            b.warning_blocks = d.get_f64("BorderWarningBlocks").unwrap_or(5.0) as i32;
            b.warning_time = d.get_f64("BorderWarningTime").unwrap_or(15.0) as i32;
        }
        b
    }

    /// Parses the stored `Inventory` list into menu slots: (id, count) for the client and the raw compounds for saving.
    #[allow(clippy::type_complexity)]
    fn load_inventory(&self, data: &Compound) -> (Vec<Option<(i32, i32)>>, Vec<Option<Compound>>) {
        let mut items = vec![None; 46];
        let mut raw: Vec<Option<Compound>> = vec![None; 46];
        let ids = self.data.registries.get("minecraft:item");
        for it in data.get_list("Inventory").map(|l| l.as_slice()).unwrap_or(&[]) {
            let Some(c) = it.as_compound() else { continue };
            let Some(slot) = c.get_i32("Slot").and_then(crate::playerdata::menu_slot) else { continue };
            let count = c.get_i32("count").or_else(|| c.get_i32("Count")).unwrap_or(1);
            if let Some(id) = c.get_str("id").and_then(|n| ids.and_then(|r| r.ids.get(n))) {
                items[slot] = Some((*id, count));
                raw[slot] = Some(c.clone());
            }
        }
        (items, raw)
    }

    /// Writes the player's saved state (position, rotation, mode, vitals, inventory) to `playerdata/<uuid>.dat`.
    fn save_player(&mut self, session: u64) {
        let Some(p) = self.players.get(&session) else { return };
        let mut d = p.data.clone();
        d.put("DataVersion", Tag::Int(crate::playerdata::DATA_VERSION));
        d.put("Pos", Tag::List(vec![Tag::Double(p.x), Tag::Double(p.y), Tag::Double(p.z)]));
        d.put("Rotation", Tag::List(vec![Tag::Float(p.yaw), Tag::Float(p.pitch)]));
        d.put("Dimension", Tag::string(self.ids.overworld));
        d.put("playerGameType", Tag::Int(p.game_mode as i32));
        d.put("SelectedItemSlot", Tag::Int(p.selected as i32));
        d.put("OnGround", Tag::Byte(p.on_ground as i8));
        d.put("Health", Tag::Float(p.health));
        d.put("foodLevel", Tag::Int(p.food));
        d.put("foodSaturationLevel", Tag::Float(p.saturation));
        d.put("XpP", Tag::Float(p.xp_progress));
        d.put("XpLevel", Tag::Int(p.xp_level));
        d.put("XpTotal", Tag::Int(p.xp_total));
        // inventory: unchanged stacks keep their stored compound (components!), changed ones are rewritten
        let names = self.data.registries.get("minecraft:item").map(|r| &r.names);
        let mut inv: Vec<Tag> = Vec::new();
        for (menu, item) in p.inventory.iter().enumerate() {
            let Some(stored) = crate::playerdata::stored_slot(menu) else { continue };
            match item {
                Some(cur) if p.inv_loaded[menu] == Some(*cur) && p.inv_raw[menu].is_some() => inv.push(Tag::Compound(p.inv_raw[menu].clone().unwrap())),
                Some((id, count)) => {
                    if let Some(n) = names.and_then(|n| n.get(*id as usize)) {
                        inv.push(Tag::Compound(Compound::new().with("Slot", Tag::Byte(stored)).with("id", Tag::string(n.clone())).with("count", Tag::Int(*count))));
                    }
                }
                None => {}
            }
        }
        d.put("Inventory", Tag::List(inv));
        if let Err(e) = crate::playerdata::save(&self.player_dir, p.uuid, &d) {
            warn!("Could not save player data of {}: {e}", p.name);
        }
    }

    fn remove_player(&mut self, session: u64) {
        self.save_player(session);
        let Some(p) = self.players.remove(&session) else { return };
        let uuid = p.uuid;
        let eid = p.entity_id;
        let dn = Self::display_name(&p);
        // remove the entity from observers
        let rm = pk::remove_entities(&[eid]);
        let f = self.frame(&rm);
        for o in self.players.values_mut() {
            if o.tracked.remove(&session) {
                o.sender.send(f.clone());
            }
        }
        let info_rm = pk::player_info_remove(&[uuid]);
        self.broadcast(&info_rm, None);
        let left = Text::translatable("multiplayer.player.left").with_args(vec![dn]).color("yellow");
        self.broadcast(&pk::system_chat(&left, false), None);
        // close the socket (if still open)
        p.sender.close(None);
        self.sync_status();
    }

    fn teleport(&mut self, session: u64, x: f64, y: f64, z: f64, yaw: f32, pitch: f32) {
        let tick = self.tick;
        let Some(p) = self.players.get_mut(&session) else { return };
        p.teleport_counter = p.teleport_counter.wrapping_add(1) & 0x7FFF_FFFF;
        let id = p.teleport_counter;
        p.awaiting = Some((id, x, y, z, tick));
        p.x = x;
        p.y = y;
        p.z = z;
        p.yaw = yaw;
        p.pitch = pitch;
        let payload = pk::player_position(x, y, z, yaw, pitch, 0, id);
        self.send(session, &payload);
    }

    // ------------------------------------------------------------ chunks

    fn desired_view(&self, p: &Player) -> View {
        let dist = (p.info.view_distance as i32).clamp(2, self.shared.settings.view_distance);
        View { cx: (p.x.floor() as i32) >> 4, cz: (p.z.floor() as i32) >> 4, dist }
    }

    /// `ChunkMap.updateChunkTracking` + `applyChunkTrackingView`.
    fn update_chunk_tracking(&mut self, session: u64) {
        let Some(p) = self.players.get(&session) else { return };
        let new = self.desired_view(p);
        let old = p.view;
        if old == Some(new) {
            return;
        }
        let mut to_send_center = false;
        if old.map(|o| (o.cx, o.cz)) != Some((new.cx, new.cz)) {
            to_send_center = true;
        }
        // difference of the views
        let mut add: Vec<(i32, i32)> = Vec::new();
        let mut drop: Vec<(i32, i32)> = Vec::new();
        match old {
            Some(o) => {
                let (a0, b0, a1, b1) = o.bounds();
                let (c0, d0, c1, d1) = new.bounds();
                let (minx, minz, maxx, maxz) = (a0.min(c0), b0.min(d0), a1.max(c1), b1.max(d1));
                for x in minx..=maxx {
                    for z in minz..=maxz {
                        let (in_old, in_new) = (o.contains(x, z), new.contains(x, z));
                        if in_old != in_new {
                            if in_new {
                                add.push((x, z));
                            } else {
                                drop.push((x, z));
                            }
                        }
                    }
                }
            }
            None => {
                let (c0, d0, c1, d1) = new.bounds();
                for x in c0..=c1 {
                    for z in d0..=d1 {
                        if new.contains(x, z) {
                            add.push((x, z));
                        }
                    }
                }
            }
        }
        if to_send_center {
            let payload = pk::set_chunk_cache_center(new.cx, new.cz);
            self.send(session, &payload);
        }
        let mut forget = Vec::new();
        {
            let p = self.players.get_mut(&session).unwrap();
            p.view = Some(new);
            for c in add {
                p.sender_state.pending.insert(c);
            }
            for c in drop {
                if !p.sender_state.pending.remove(&c) {
                    forget.push(c);
                }
            }
        }
        for (x, z) in forget {
            self.send(session, &pk::forget_level_chunk(x, z));
        }
    }

    /// `PlayerChunkSender.sendNextChunks`.
    fn send_next_chunks(&mut self, session: u64) {
        let (center, quota_chunks) = {
            let Some(p) = self.players.get_mut(&session) else { return };
            let st = &mut p.sender_state;
            if st.unacked >= st.max_unacked {
                return;
            }
            let cap = st.desired_per_tick.max(1.0);
            st.batch_quota = (st.batch_quota + st.desired_per_tick).min(cap);
            if st.batch_quota < 1.0 || st.pending.is_empty() {
                return;
            }
            let v = p.view.unwrap();
            ((v.cx, v.cz), st.batch_quota.floor() as usize)
        };
        // nearest first (Comparators.least / sorting by distanceSquared)
        let mut list: Vec<(i32, i32)> = self.players[&session].sender_state.pending.iter().copied().collect();
        let dist = |c: &(i32, i32)| {
            let (dx, dz) = (c.0 - center.0, c.1 - center.1);
            dx * dx + dz * dz
        };
        list.sort_by_key(|c| (dist(c), c.0, c.1));
        list.truncate(quota_chunks);
        if list.is_empty() {
            return;
        }
        let start = self.frame(&pk::chunk_batch_start());
        let mut frames = Vec::with_capacity(list.len() + 2);
        frames.push(start);
        for &(x, z) in &list {
            frames.push(self.world.chunk_frame(x, z, &mut self.enc));
        }
        let fin = self.frame(&pk::chunk_batch_finished(list.len() as i32));
        frames.push(fin);
        let p = self.players.get_mut(&session).unwrap();
        for c in &list {
            p.sender_state.pending.remove(c);
        }
        p.sender_state.unacked += 1;
        p.sender_state.batch_quota -= list.len() as f32;
        for f in frames {
            p.sender.send(f);
        }
    }

    // ------------------------------------------------------------ encje

    /// Non-default metadata values of a player (`SynchedEntityData.getNonDefaultValues`), ascending by index.
    fn entity_data_values(p: &Player) -> Vec<(u8, EntityData)> {
        let mut data: Vec<(u8, EntityData)> = Vec::new();
        let mut flags = 0i8;
        if p.sneaking {
            flags |= 0x02;
        }
        if p.sprinting {
            flags |= 0x08;
        }
        if flags != 0 {
            data.push((0, EntityData::Byte(flags)));
        }
        data.push((9, EntityData::Float(p.health)));
        if p.info.model_customisation != 0 {
            data.push((17, EntityData::Byte(p.info.model_customisation as i8)));
        }
        if p.info.main_hand != 1 {
            data.push((18, EntityData::Byte(p.info.main_hand as i8)));
        }
        data
    }

    /// `ClientboundUpdateAttributesPacket` for the given attribute instances of a player
    /// (0 = movement speed, 1 = block interaction range, 2 = entity interaction range), or `None` for an empty selection.
    fn attributes_packet(&self, p: &Player, which: &[usize]) -> Option<Vec<u8>> {
        const BLOCK_MOD: [(&str, f64); 1] = [("minecraft:creative_mode_block_range", 0.5)];
        const ENTITY_MOD: [(&str, f64); 1] = [("minecraft:creative_mode_entity_range", 2.0)];
        if which.is_empty() {
            return None;
        }
        let list: Vec<pk::AttributeSnapshot<'_>> = which
            .iter()
            .map(|&i| match i {
                0 => pk::AttributeSnapshot { id: self.ids.attr_movement_speed, base: p.walk_speed, modifiers: &[] },
                1 => pk::AttributeSnapshot { id: self.ids.attr_block_range, base: 4.5, modifiers: if p.creative_mods { &BLOCK_MOD } else { &[] } },
                _ => pk::AttributeSnapshot { id: self.ids.attr_entity_range, base: 3.0, modifiers: if p.creative_mods { &ENTITY_MOD } else { &[] } },
            })
            .collect();
        Some(pk::update_attributes(p.entity_id, &list))
    }

    /// `ServerPlayer.updatePlayerAttributes` (every tick): creates the range instances and applies / removes the creative
    /// modifiers; then `LivingEntity.updateDirtyAttributes` syncs what changed to the player and everybody tracking it.
    fn update_player_attributes(&mut self, session: u64) {
        let (payload, observers) = {
            let p = self.players.get_mut(&session).unwrap();
            p.attr_created[1] = true;
            p.attr_created[2] = true;
            let creative = p.game_mode == 1;
            if creative != p.creative_mods {
                p.creative_mods = creative;
                p.attr_dirty[1] = true;
                p.attr_dirty[2] = true;
            }
            // dirty order as observed on the vanilla server: block range, movement speed, entity range
            let which: Vec<usize> = [1usize, 0, 2].into_iter().filter(|&i| p.attr_dirty[i]).collect();
            p.attr_dirty = [false; 3];
            let payload = self.attributes_packet(&self.players[&session], &which);
            let observers: Vec<u64> = self.players.iter().filter(|(s, o)| **s != session && o.tracked.contains(&session)).map(|(s, _)| *s).collect();
            (payload, observers)
        };
        if let Some(payload) = payload {
            let f = self.frame(&payload);
            for s in std::iter::once(&session).chain(observers.iter()) {
                if let Some(p) = self.players.get(s) {
                    p.sender.send(f.clone());
                }
            }
        }
    }

    /// Packets for a player appearing at an observer: one bundle (`ClientboundBundlePacket`)
    /// with add_entity, entity data and attributes, like `ServerEntity.sendPairingData`.
    fn spawn_packets(&self, p: &Player) -> Vec<Vec<u8>> {
        let mut out = vec![
            pk::bundle_delimiter(),
            pk::add_entity(p.entity_id, p.uuid, self.ids.player_entity_type, p.x, p.y, p.z, p.pitch, p.yaw, p.yaw, 0, 0, 0, 0),
            pk::set_entity_data(p.entity_id, &Self::entity_data_values(p)),
        ];
        // only the attribute instances that exist at this moment are synchronized
        let existing: Vec<usize> = (0..3).filter(|&k| p.attr_created[k]).collect();
        out.extend(self.attributes_packet(p, &existing));
        out.push(pk::bundle_delimiter());
        out
    }

    /// `ServerPlayer.adjustSpawnLocation`: a random point in a square of radius `spawnRadius` around the world spawn
    /// (iterated with a coprime step), the first one with valid ground.
    fn adjust_spawn_location(&mut self) -> (i32, i32, i32) {
        use rand::Rng;
        let (sx, sy, sz) = self.world.spawn;
        let r = self.rules.spawn_radius.max(0) as i64;
        let side = r * 2 + 1;
        let n = side * side;
        let coprime = if n <= 16 { n - 1 } else { 17 };
        let start = rand::thread_rng().gen_range(0..n);
        for i in 0..n {
            let idx = (start + coprime * i) % n;
            let (dx, dz) = ((idx % side) as i32 - r as i32, (idx / side) as i32 - r as i32);
            if let Some(y) = self.world.surface_y(sx + dx, sz + dz) {
                return (sx + dx, y, sz + dz);
            }
        }
        (sx, sy, sz)
    }

    /// Visibility per `ChunkMap.TrackedEntity.updatePlayer`: horizontal distance <= min(range, viewDistance*16)
    /// and the entity's chunk inside the observer's view. A player entity's range is 32 chunks.
    fn visible(&self, viewer: &Player, target: &Player) -> bool {
        let dx = viewer.x - target.x;
        let dz = viewer.z - target.z;
        let range = (32.0f64 * 16.0).min(self.shared.settings.view_distance.min(viewer.info.view_distance as i32).max(2) as f64 * 16.0);
        let in_range = dx * dx + dz * dz <= range * range;
        let tracked_chunk = viewer.view.is_some_and(|v| v.contains((target.x.floor() as i32) >> 4, (target.z.floor() as i32) >> 4));
        in_range && tracked_chunk
    }

    /// Updates pairing (spawn/despawn) between `session` and the other players in both directions.
    fn update_entity_tracking(&mut self, session: u64) {
        let sessions: Vec<u64> = self.players.keys().copied().filter(|s| *s != session).collect();
        for other in sessions {
            // other -> session widzi?
            let (sees_me, i_see) = {
                let me = &self.players[&session];
                let o = &self.players[&other];
                (self.visible(o, me), self.visible(me, o))
            };
            // whether the other player sees me
            let tracked_by_other = self.players[&other].tracked.contains(&session);
            if sees_me && !tracked_by_other {
                let pkts = self.spawn_packets(&self.players[&session]);
                for pl in pkts {
                    self.send(other, &pl);
                }
                self.players.get_mut(&other).unwrap().tracked.insert(session);
            } else if !sees_me && tracked_by_other {
                let eid = self.players[&session].entity_id;
                self.send(other, &pk::remove_entities(&[eid]));
                self.players.get_mut(&other).unwrap().tracked.remove(&session);
            }
            // whether I see the other player
            let tracked_by_me = self.players[&session].tracked.contains(&other);
            if i_see && !tracked_by_me {
                let pkts = self.spawn_packets(&self.players[&other]);
                for pl in pkts {
                    self.send(session, &pl);
                }
                self.players.get_mut(&session).unwrap().tracked.insert(other);
            } else if !i_see && tracked_by_me {
                let eid = self.players[&other].entity_id;
                self.send(session, &pk::remove_entities(&[eid]));
                self.players.get_mut(&session).unwrap().tracked.remove(&other);
            }
        }
    }

    /// `ServerEntity.sendChanges` for a player (updateInterval = 2): sends position/rotation changes to observers.
    fn send_entity_changes(&mut self, session: u64) {
        let (payloads, observers): (Vec<Vec<u8>>, Vec<u64>) = {
            let p = self.players.get_mut(&session).unwrap();
            let t = p.ticks; // tickCount of ServerEntity (grows after every call)
            p.ticks += 1;
            if t % 2 != 0 {
                return;
            }
            p.teleport_delay += 1;
            let yaw_b = (p.yaw * 256.0 / 360.0).floor() as i32;
            let pitch_b = (p.pitch * 256.0 / 360.0).floor() as i32;
            let (bx, by, bz) = p.base;
            let dx = pos_enc(p.x) - pos_enc(bx);
            let dy = pos_enc(p.y) - pos_enc(by);
            let dz = pos_enc(p.z) - pos_enc(bz);
            // VecDeltaCodec.delta: difference from the decoded base (position rounded to 1/4096)
            let (ddx, ddy, ddz) = (p.x - bx, p.y - by, p.z - bz);
            let moved = ddx * ddx + ddy * ddy + ddz * ddz >= 7.6293945E-6;
            let pos_changed = moved || t % 60 == 0;
            let rot_changed = (yaw_b - p.last_yaw).abs() >= 1 || (pitch_b - p.last_pitch).abs() >= 1;
            let overflow = [dx, dy, dz].iter().any(|d| *d < -32768 || *d > 32767);
            let mut out = Vec::new();
            let mut set_base = false;
            let mut set_rot = false;
            if overflow || p.teleport_delay > 400 || p.was_on_ground != p.on_ground {
                p.was_on_ground = p.on_ground;
                p.teleport_delay = 0;
                out.push(pk::teleport_entity(p.entity_id, p.x, p.y, p.z, p.yaw, p.pitch, p.on_ground));
                set_base = true;
                set_rot = true;
            } else if !pos_changed || !rot_changed {
                if pos_changed {
                    out.push(pk::move_entity_pos(p.entity_id, dx as i16, dy as i16, dz as i16, p.on_ground));
                    set_base = true;
                } else if rot_changed {
                    out.push(pk::move_entity_rot(p.entity_id, p.yaw, p.pitch, p.on_ground));
                    set_rot = true;
                }
            } else {
                out.push(pk::move_entity_pos_rot(p.entity_id, dx as i16, dy as i16, dz as i16, p.yaw, p.pitch, p.on_ground));
                set_base = true;
                set_rot = true;
            }
            if set_base {
                p.base = (p.x, p.y, p.z);
            }
            if set_rot {
                p.last_yaw = yaw_b;
                p.last_pitch = pitch_b;
            }
            let head_b = (p.yaw * 256.0 / 360.0).floor() as i32;
            if (head_b - p.last_head).abs() >= 1 {
                out.push(pk::rotate_head(p.entity_id, p.yaw));
                p.last_head = head_b;
            }
            let obs = self.players.iter().filter(|(s, o)| **s != session && o.tracked.contains(&session)).map(|(s, _)| *s).collect();
            (out, obs)
        };
        for pl in payloads {
            let f = self.frame(&pl);
            for o in &observers {
                if let Some(op) = self.players.get(o) {
                    op.sender.send(f.clone());
                }
            }
        }
    }

    // ------------------------------------------------------------ tick

    fn do_tick(&mut self) {
        self.tick += 1;
        self.game_time += 1;
        if self.rules.daylight_cycle {
            self.day_time += 1;
        }
        if self.tick % 20 == 0 {
            let f = pk::set_time(self.game_time, self.day_time, self.rules.daylight_cycle);
            self.broadcast(&f, None);
        }
        // autosave every 5 minutes (vanilla: every 6000 ticks) and unload chunks nobody watches
        if self.tick % 6000 == 0 {
            let saved = self.world.save(self.game_time, self.day_time);
            let uuids: Vec<u64> = self.players.keys().copied().collect();
            for s in uuids {
                self.save_player(s);
            }
            info!("Saved the world ({saved} chunks)");
        }
        if self.tick % 200 == 0 {
            let mut keep = std::collections::HashSet::new();
            for p in self.players.values() {
                if let Some(v) = p.view {
                    let (a, b, c, d) = v.bounds();
                    for x in a - 1..=c + 1 {
                        for z in b - 1..=d + 1 {
                            keep.insert((x, z));
                        }
                    }
                }
            }
            self.world.evict(&keep, self.game_time);
        }
        // ChunkMap.tick -> ChunkHolder.broadcastChanges: block changes go to the players tracking the chunk
        self.flush_block_changes();
        let sessions: Vec<u64> = self.players.keys().copied().collect();
        for s in sessions {
            if !self.players.contains_key(&s) {
                continue;
            }
            self.tick_player(s);
        }
    }

    /// Sets a block and records the change for broadcast at the start of the next tick.
    fn set_block_tracked(&mut self, x: i32, y: i32, z: i32, state: u16) -> Option<u16> {
        let old = self.world.set_block(x, y, z, state)?;
        if old != state {
            let sy = (y - self.world.dim.min_y) >> 4;
            self.dirty.entry((x >> 4, z >> 4, sy)).or_default().insert(((x & 15) as u8, (z & 15) as u8, (y & 15) as u8));
        }
        Some(old)
    }

    /// `ChunkHolder.broadcastChanges`: 1 change in a section -> `block_update`, more -> `section_blocks_update`.
    /// Recipients: players that have the chunk in view and have already received it (`ChunkMap.isChunkTracked`).
    fn flush_block_changes(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        let dirty = std::mem::take(&mut self.dirty);
        let min_y = self.world.dim.min_y;
        for ((cx, cz, sy), set) in dirty {
            let payload = if set.len() == 1 {
                let &(lx, lz, ly) = set.iter().next().unwrap();
                let (x, y, z) = ((cx << 4) + lx as i32, min_y + (sy << 4) + ly as i32, (cz << 4) + lz as i32);
                pk::block_update(x, y, z, self.world.get_block(x, y, z) as u32)
            } else {
                let changes: Vec<(u16, u32)> = set
                    .iter()
                    .map(|&(lx, lz, ly)| {
                        let (x, y, z) = ((cx << 4) + lx as i32, min_y + (sy << 4) + ly as i32, (cz << 4) + lz as i32);
                        (((lx as u16) << 8) | ((lz as u16) << 4) | ly as u16, self.world.get_block(x, y, z) as u32)
                    })
                    .collect();
                pk::section_blocks_update(cx, sy + (min_y >> 4), cz, &changes)
            };
            let f = self.frame(&payload);
            for p in self.players.values() {
                if p.view.is_some_and(|v| v.contains(cx, cz)) && !p.sender_state.pending.contains(&(cx, cz)) {
                    p.sender.send(f.clone());
                }
            }
        }
    }

    /// `AbstractContainerMenu.broadcastChanges`: sends `container_set_slot` for slots that differ from the state the client knows.
    fn broadcast_inventory_changes(&mut self, session: u64) {
        let mut out = Vec::new();
        {
            let p = self.players.get_mut(&session).unwrap();
            for slot in 0..p.inventory.len() {
                if p.inventory[slot] != p.remote_inv[slot] {
                    p.remote_inv[slot] = p.inventory[slot];
                    p.state_id = (p.state_id + 1) & 0x7FFF_FFFF;
                    out.push(pk::container_set_slot(0, p.state_id, slot as i16, p.inventory[slot]));
                }
            }
        }
        for o in out {
            self.send(session, &o);
        }
    }

    /// `Player.canInteractWithBlock(pos, extra)`: distance from the eye to the block AABB < (range + extra).
    fn can_interact_with_block(p: &Player, x: i32, y: i32, z: i32, extra: f64) -> bool {
        let eye = (p.x, p.y + if p.sneaking { 1.27 } else { 1.62 }, p.z);
        let d = |v: f64, lo: i32| (lo as f64 - v).max(v - (lo as f64 + 1.0)).max(0.0);
        let (dx, dy, dz) = (d(eye.0, x), d(eye.1, y), d(eye.2, z));
        let r = if p.creative_mods { 5.0 } else { 4.5 } + extra; // block_interaction_range (+0.5 creative modifier)
        dx * dx + dy * dy + dz * dz < r * r
    }

    /// Whether a block (a full cube) overlaps a player's hitbox (`isUnobstructed`; simplification: full cubes).
    fn block_intersects_player(&self, x: i32, y: i32, z: i32) -> bool {
        self.players.values().any(|p| {
            if p.game_mode == 3 {
                return false;
            }
            let h = if p.sneaking { 1.5 } else { 1.8 };
            let (minx, maxx, miny, maxy, minz, maxz) = (p.x - 0.3, p.x + 0.3, p.y, p.y + h, p.z - 0.3, p.z + 0.3);
            (x as f64) < maxx && (x as f64 + 1.0) > minx && (y as f64) < maxy && (y as f64 + 1.0) > miny && (z as f64) < maxz && (z as f64 + 1.0) > minz
        })
    }

    fn max_build_height(&self) -> i32 {
        self.world.dim.min_y + self.world.dim.height
    }

    /// `ServerLevel.mayInteract`: spawn protection only applies when the server has operators (none yet) + the world border.
    fn may_interact(&self, x: i32, z: i32) -> bool {
        const LIMIT: i32 = 29_999_984;
        x.abs() <= LIMIT && z.abs() <= LIMIT
    }

    fn tick_player(&mut self, s: u64) {
        // ServerGamePacketListenerImpl.tick: the block sequence acknowledgement is sent first
        let ack = std::mem::replace(&mut self.players.get_mut(&s).unwrap().ack_up_to, -1);
        if ack > -1 {
            self.send(s, &pk::block_changed_ack(ack));
        }
        // overloaded socket
        if self.players[&s].sender.pending.load(Ordering::Relaxed) > MAX_PENDING_BYTES {
            return self.kick(s, Text::translatable("disconnect.timeout"));
        }
        // resend the teleport after 20 ticks without confirmation
        let tick = self.tick;
        if let Some((_, x, y, z, t)) = self.players[&s].awaiting {
            if tick - t > 20 {
                let (yaw, pitch) = (self.players[&s].yaw, self.players[&s].pitch);
                self.teleport(s, x, y, z, yaw, pitch);
            }
        }
        // keepalive
        {
            let p = self.players.get_mut(&s).unwrap();
            if p.ka_time.elapsed().as_millis() >= KEEPALIVE_INTERVAL_MS {
                if p.ka_pending {
                    return self.kick(s, Text::translatable("disconnect.timeout"));
                }
                p.ka_pending = true;
                p.ka_time = Instant::now();
                // Util.getMillis()
                p.ka_challenge = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
                let id = p.ka_challenge;
                self.send(s, &pk::keep_alive(id));
            }
        }
        // first sending of health/experience (ServerPlayer.doTick: lastSent* = invalid)
        let first = !self.players[&s].first_tick_done;
        if first {
            self.players.get_mut(&s).unwrap().first_tick_done = true;
            let (h, f, sat, xp) = {
                let p = &self.players[&s];
                (p.health, p.food, p.saturation, (p.xp_progress, p.xp_level, p.xp_total))
            };
            self.send(s, &pk::set_health(h, f, sat));
            self.send(s, &pk::set_experience(xp.0, xp.1, xp.2));
        }
        self.broadcast_inventory_changes(s);
        self.send_next_chunks(s);
        // vanilla syncs the player's attributes after the chunk batch (ServerPlayer.tick -> LivingEntity.updateDirtyAttributes)
        self.update_player_attributes(s);
        self.send_entity_changes(s);
    }

    // ------------------------------------------------------------ packets

    fn on_packet(&mut self, session: u64, pkt: Serverbound) {
        if !self.players.contains_key(&session) {
            return;
        }
        match pkt {
            Serverbound::KeepAlive(id) => {
                let p = self.players.get_mut(&session).unwrap();
                if p.ka_pending && id == p.ka_challenge {
                    let elapsed = p.ka_time.elapsed().as_millis() as i32;
                    p.latency = (p.latency * 3 + elapsed) / 4;
                    p.ka_pending = false;
                } else {
                    self.kick(session, Text::translatable("disconnect.timeout"));
                }
            }
            Serverbound::AcceptTeleportation(id) => {
                let p = self.players.get_mut(&session).unwrap();
                if let Some((aid, x, y, z, _)) = p.awaiting {
                    if aid == id {
                        p.x = x;
                        p.y = y;
                        p.z = z;
                        p.awaiting = None;
                    }
                }
            }
            Serverbound::ClientInformation(ci) => {
                self.players.get_mut(&session).unwrap().info = ci;
                self.update_chunk_tracking(session);
            }
            Serverbound::ChunkBatchReceived(n) => {
                let st = &mut self.players.get_mut(&session).unwrap().sender_state;
                st.unacked -= 1;
                st.desired_per_tick = if n.is_nan() { 0.01 } else { n.clamp(0.01, 64.0) };
                if st.unacked == 0 {
                    st.batch_quota = 1.0;
                }
                st.max_unacked = 10;
            }
            Serverbound::MovePos { x, y, z, on_ground } => self.on_move(session, Some((x, y, z)), None, on_ground),
            Serverbound::MovePosRot { x, y, z, yaw, pitch, on_ground } => self.on_move(session, Some((x, y, z)), Some((yaw, pitch)), on_ground),
            Serverbound::MoveRot { yaw, pitch, on_ground } => self.on_move(session, None, Some((yaw, pitch)), on_ground),
            Serverbound::MoveStatusOnly { on_ground } => self.on_move(session, None, None, on_ground),
            Serverbound::Chat(msg) => self.on_chat(session, msg),
            Serverbound::ChatCommand(c) | Serverbound::ChatCommandSigned(c) => self.on_command(session, c),
            Serverbound::PlayerCommand { action, .. } => {
                let p = self.players.get_mut(&session).unwrap();
                match action {
                    0 => p.sneaking = true,
                    1 => p.sneaking = false,
                    3 => p.sprinting = true,
                    4 => p.sprinting = false,
                    _ => return,
                }
                let mut flags = 0i8;
                if p.sneaking {
                    flags |= 0x02;
                }
                if p.sprinting {
                    flags |= 0x08;
                }
                let payload = pk::set_entity_data(p.entity_id, &[(0, EntityData::Byte(flags))]);
                self.broadcast_to_observers(session, &payload);
            }
            Serverbound::Swing { hand } => {
                let eid = self.players[&session].entity_id;
                let payload = mc_protocol::frame::payload(mc_protocol::ids::play::clientbound::ANIMATE, |w| {
                    use mc_protocol::Put;
                    w.put_varint(eid);
                    w.put_u8(if hand == 0 { 0 } else { 3 });
                });
                self.broadcast_to_observers(session, &payload);
            }
            Serverbound::SetCarriedItem(slot) => {
                if (0..9).contains(&slot) {
                    self.players.get_mut(&session).unwrap().selected = slot as i8;
                } else {
                    self.kick(session, Text::plain("Invalid hotbar selection"));
                }
            }
            Serverbound::SetCreativeModeSlot { slot, item } => {
                let p = self.players.get_mut(&session).unwrap();
                if p.game_mode == 1 && (0..46).contains(&slot) {
                    p.inventory[slot as usize] = item;
                    // handleSetCreativeModeSlot: broadcastChanges right away (a confirming echo for the client)
                    self.broadcast_inventory_changes(session);
                }
            }
            Serverbound::PlayerAction { action, x, y, z, sequence, .. } => self.on_player_action(session, action, x, y, z, sequence),
            Serverbound::UseItemOn { x, y, z, face, sequence, hand, cursor, .. } => self.on_use_item_on(session, hand, x, y, z, face, cursor, sequence),
            Serverbound::PlayerAbilities { flying } => {
                let _ = flying;
            }
            Serverbound::ClientCommand(_)
            | Serverbound::ChatSessionUpdate
            | Serverbound::ConfigurationAcknowledged
            | Serverbound::PlayerInput
            | Serverbound::Pong(_)
            | Serverbound::UseItem { .. }
            | Serverbound::CustomPayload { .. }
            | Serverbound::Unhandled(_) => {}
        }
    }

    fn broadcast_to_observers(&mut self, session: u64, payload: &[u8]) {
        let f = self.frame(payload);
        for (s, o) in &self.players {
            if *s != session && o.tracked.contains(&session) {
                o.sender.send(f.clone());
            }
        }
    }

    fn on_move(&mut self, session: u64, pos: Option<(f64, f64, f64)>, rot: Option<(f32, f32)>, on_ground: bool) {
        let tick = self.tick;
        let (awaiting, resend) = {
            let p = &self.players[&session];
            (p.awaiting, p.awaiting.map(|a| tick - a.4 > 20).unwrap_or(false))
        };
        if let Some((_, x, y, z, _)) = awaiting {
            // Until the client confirms the teleport, movement is ignored (ServerGamePacketListenerImpl.handleMovePlayer)
            if resend {
                let (yaw, pitch) = (self.players[&session].yaw, self.players[&session].pitch);
                self.teleport(session, x, y, z, yaw, pitch);
            }
            return;
        }
        if let Some((x, y, z)) = pos {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) || x.abs() > 3.0E7 || z.abs() > 3.0E7 || y.abs() > 2.0E7 {
                return self.kick(session, Text::translatable("multiplayer.disconnect.invalid_player_movement"));
            }
        }
        if let Some((yaw, pitch)) = rot {
            if !(yaw.is_finite() && pitch.is_finite()) {
                return self.kick(session, Text::translatable("multiplayer.disconnect.invalid_player_movement"));
            }
        }
        let p = self.players.get_mut(&session).unwrap();
        if let Some((x, y, z)) = pos {
            p.x = x;
            p.y = y;
            p.z = z;
        }
        if let Some((yaw, pitch)) = rot {
            p.yaw = yaw % 360.0;
            p.pitch = pitch.clamp(-90.0, 90.0);
        }
        p.on_ground = on_ground;
        if pos.is_some() {
            self.update_chunk_tracking(session);
            // movement may change the visibility of other players (both ways)
            self.update_entity_tracking(session);
        }
    }

    fn on_chat(&mut self, session: u64, msg: String) {
        if msg.chars().any(|c| c == '§' || (c as u32) < 32 || c as u32 == 127) {
            return self.kick(session, Text::translatable("multiplayer.disconnect.illegal_characters"));
        }
        let (uuid, name) = {
            let p = &self.players[&session];
            (p.uuid, Self::display_name(p))
        };
        info!("<{}> {}", self.players[&session].name, msg);
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        // unsigned message: every recipient gets its own `index` counter
        let sessions: Vec<u64> = self.players.keys().copied().collect();
        for s in sessions {
            let idx = {
                let p = self.players.get_mut(&s).unwrap();
                let i = p.message_index;
                p.message_index += 1;
                i
            };
            let payload = pk::player_chat_unsigned(uuid, idx, &msg, ts, self.ids.chat_type, &name);
            self.send(s, &payload);
        }
    }

    /// `CommandSourceStack.sendFailure`: `Component.empty().append(msg).withStyle(RED)`.
    fn send_failure(&mut self, session: u64, msg: Text) {
        let mut t = Text::empty();
        t.color = Some("red".into());
        t.extra = vec![msg];
        self.send(session, &pk::system_chat(&t, false));
    }

    fn on_command(&mut self, session: u64, cmd: String) {
        info!("{} issued server command: /{}", self.players[&session].name, cmd);
        let name = cmd.split_whitespace().next().unwrap_or("");
        match name {
            "list" => {
                // ListPlayersCommand: a translatable with (player count, max, ComponentUtils.formatList(names))
                let names: Vec<Text> = self.players.values().map(Self::display_name).collect();
                let list = match names.len() {
                    0 => Text::empty(),
                    1 => names.into_iter().next().unwrap(),
                    _ => {
                        let mut sep_list = Text::empty();
                        let mut first = true;
                        for n in names {
                            if !first {
                                let mut sep = Text::plain(", ");
                                sep.color = Some("gray".into());
                                sep_list.extra.push(sep);
                            }
                            sep_list.extra.push(n);
                            first = false;
                        }
                        sep_list
                    }
                };
                let msg = Text::translatable("commands.list.players").with_args(vec![
                    Text::int(self.players.len() as i32),
                    Text::int(self.shared.settings.max_players),
                    list,
                ]);
                self.send(session, &pk::system_chat(&msg, false));
            }
            _ => {
                // Brigadier: dispatcherUnknownCommand z kursorem 0 -> komunikat + kontekst "<--[HERE]" (Commands.performCommand)
                self.send_failure(session, Text::translatable("command.unknown.command"));
                let cursor = 0usize;
                let mut ctx = Text::empty();
                ctx.color = Some("gray".into());
                ctx.style_nbt.push(("clickEvent".into(), Tag::Compound(Compound::new().with("action", Tag::string("suggest_command")).with("value", Tag::string(format!("/{cmd}"))))));
                let chars: Vec<char> = cmd.chars().collect();
                let prefix: String = chars[cursor.saturating_sub(10)..cursor].iter().collect();
                if cursor > 10 {
                    ctx.extra.push(Text::plain("..."));
                }
                if !prefix.is_empty() {
                    ctx.extra.push(Text::plain(prefix));
                }
                if cursor < chars.len() {
                    let mut rest = Text::plain(chars[cursor..].iter().collect::<String>());
                    rest.color = Some("red".into());
                    rest.style_nbt.push(("underlined".into(), Tag::Byte(1)));
                    ctx.extra.push(rest);
                }
                let mut here = Text::translatable("command.context.here").color("red");
                here.italic = Some(true);
                ctx.extra.push(here);
                self.send_failure(session, ctx);
            }
        }
    }

    fn ack_block_changes_up_to(&mut self, session: u64, sequence: i32) {
        let p = self.players.get_mut(&session).unwrap();
        p.ack_up_to = p.ack_up_to.max(sequence);
    }

    fn resend_block(&mut self, session: u64, x: i32, y: i32, z: i32) {
        let st = self.world.get_block(x, y, z);
        self.send(session, &pk::block_update(x, y, z, st as u32));
    }

    /// `ServerGamePacketListenerImpl.handlePlayerAction` + `ServerPlayerGameMode.handleBlockBreakAction`.
    /// Survival mode: mining times need block hardness (M3), so `STOP_DESTROY_BLOCK` is accepted without verification.
    fn on_player_action(&mut self, session: u64, action: i32, x: i32, y: i32, z: i32, sequence: i32) {
        match action {
            0..=2 => {
                self.handle_block_break_action(session, action, x, y, z);
                self.ack_block_changes_up_to(session, sequence);
            }
            _ => {} // drop / release / swap: inventory and items (M8)
        }
    }

    fn handle_block_break_action(&mut self, session: u64, action: i32, x: i32, y: i32, z: i32) {
        let (gm, near) = {
            let p = &self.players[&session];
            (p.game_mode, Self::can_interact_with_block(p, x, y, z, 1.0))
        };
        if !near {
            return; // "too far"
        }
        if y >= self.max_build_height() {
            return self.resend_block(session, x, y, z);
        }
        if gm == 3 {
            return;
        }
        let air = self.world.palette.air;
        match action {
            0 => {
                if !self.may_interact(x, z) {
                    return self.resend_block(session, x, y, z);
                }
                if gm == 1 {
                    self.destroy_and_ack(session, x, y, z, air);
                }
            }
            2 => {
                // survival: trust the client (see the comment above); adventure mode does not destroy blocks
                if gm == 0 {
                    self.destroy_and_ack(session, x, y, z, air);
                }
            }
            _ => {}
        }
    }

    /// `ServerPlayerGameMode.destroyAndAck`: after a successful destroy the change goes through the chunk change queue; on failure the state is sent back.
    fn destroy_and_ack(&mut self, session: u64, x: i32, y: i32, z: i32, air: u16) {
        if self.set_block_tracked(x, y, z, air).is_none() {
            self.resend_block(session, x, y, z);
        }
    }

    /// `ServerGamePacketListenerImpl.handleUseItemOn` + `BlockItem.place` (simplified: the block's default state).
    #[allow(clippy::too_many_arguments)]
    fn on_use_item_on(&mut self, session: u64, hand: i32, x: i32, y: i32, z: i32, face: i32, cursor: (f32, f32, f32), sequence: i32) {
        self.ack_block_changes_up_to(session, sequence);
        if !Self::can_interact_with_block(&self.players[&session], x, y, z, 1.0) {
            return;
        }
        if (cursor.0 - 0.5).abs() >= 1.0000001 || (cursor.1 - 0.5).abs() >= 1.0000001 || (cursor.2 - 0.5).abs() >= 1.0000001 {
            return; // "Location too far away from hit block"
        }
        // kierunki: 0 DOWN,1 UP,2 NORTH(-z),3 SOUTH(+z),4 WEST(-x),5 EAST(+x)
        let (dx, dy, dz) = match face {
            0 => (0, -1, 0),
            1 => (0, 1, 0),
            2 => (0, 0, -1),
            3 => (0, 0, 1),
            4 => (-1, 0, 0),
            5 => (1, 0, 0),
            _ => (0, 0, 0),
        };
        let (px, py, pz) = (x + dx, y + dy, z + dz);
        let max_h = self.max_build_height();
        if y < max_h {
            let (gm, awaiting) = {
                let p = &self.players[&session];
                (p.game_mode, p.awaiting.is_some())
            };
            if !awaiting && self.may_interact(x, z) && gm != 3 && gm != 2 {
                let slot = if hand == 0 { 36 + self.players[&session].selected as usize } else { 45 };
                let held = self.players[&session].inventory.get(slot).copied().flatten();
                let target_block = self.world.get_block(px, py, pz);
                let target_air = self.world.palette.is_air(target_block);
                if let (Some((item_id, _)), true) = (held, target_air && py < max_h && py >= self.world.dim.min_y) {
                    // item -> block with the same name; default state (getStateForPlacement: M7)
                    let name = self.data.registries.get("minecraft:item").and_then(|r| r.names.get(item_id as usize)).cloned();
                    if let Some(state) = name.and_then(|n| self.data.blocks.default_state(&n)) {
                        if !self.block_intersects_player(px, py, pz) && self.set_block_tracked(px, py, pz, state as u16).is_some() && gm == 0 {
                            // survival: consume one item from the hand
                            let p = self.players.get_mut(&session).unwrap();
                            if let Some((id, c)) = p.inventory[slot] {
                                p.inventory[slot] = if c > 1 { Some((id, c - 1)) } else { None };
                            }
                        }
                    }
                }
            }
        } else {
            let msg = Text::translatable("build.tooHigh").with_args(vec![Text::int(max_h - 1)]).color("red");
            self.send(session, &pk::system_chat(&msg, true));
        }
        // vanilla always sends back the state of the clicked and the adjacent block so the client reverts its prediction
        self.resend_block(session, x, y, z);
        self.resend_block(session, px, py, pz);
    }
}

fn abilities_from(data: &Compound, gm: u8) -> Abilities {
    let base = abilities_for(gm);
    match data.get_compound("abilities") {
        Some(a) => Abilities {
            invulnerable: a.get_bool("invulnerable").unwrap_or(base.invulnerable),
            flying: a.get_bool("flying").unwrap_or(base.flying),
            may_fly: a.get_bool("mayfly").unwrap_or(base.may_fly),
            instabuild: a.get_bool("instabuild").unwrap_or(base.instabuild),
            flying_speed: a.get_f64("flySpeed").unwrap_or(base.flying_speed as f64) as f32,
            walking_speed: a.get_f64("walkSpeed").unwrap_or(base.walking_speed as f64) as f32,
        },
        None => base,
    }
}

fn abilities_for(gm: u8) -> Abilities {
    // Abilities.setUpdate...: creative: invulnerable, mayfly, instabuild; spectator: + flying
    Abilities {
        invulnerable: gm == 1 || gm == 3,
        flying: gm == 3,
        may_fly: gm == 1 || gm == 3,
        instabuild: gm == 1,
        flying_speed: 0.05,
        walking_speed: 0.1,
    }
}

fn text_plain(t: &Text) -> String {
    t.text.clone().or_else(|| t.translate.clone()).unwrap_or_default()
}

#[allow(dead_code)]
fn version() -> &'static str {
    VERSION_NAME
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values computed by calling the real `ChunkTrackingView.isWithinDistance` from the 1.21.1 server
    /// (decompiled jar with Mojang mappings), number of chunks in the square [-view-3, view+3]^2.
    #[test]
    fn view_matches_mojang_reference_counts() {
        let reference = [(2, 45), (3, 77), (8, 313), (10, 453), (12, 609), (32, 3653)];
        for (dist, expected) in reference {
            let v = View { cx: 0, cz: 0, dist };
            let mut n = 0;
            for x in -dist - 3..=dist + 3 {
                for z in -dist - 3..=dist + 3 {
                    if v.contains(x, z) {
                        n += 1;
                    }
                }
            }
            assert_eq!(n, expected, "view distance {dist}");
        }
        let v = View { cx: 0, cz: 0, dist: 10 };
        assert!(v.contains(11, 0) && !v.contains(12, 0) && !v.contains(11, 11));
        // isInViewDistance (without the outer ring) per the reference: 385
        let mut n = 0;
        for x in -13..=13 {
            for z in -13..=13 {
                if within_distance(0, 0, 10, x, z, false) {
                    n += 1;
                }
            }
        }
        assert_eq!(n, 385);
    }
}
