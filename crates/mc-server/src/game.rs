//! Wątek gry: jedna pętla 20 TPS (jak główny wątek vanilla) przetwarzająca zdarzenia sieciowe,
//! stan graczy, śledzenie chunków/encji i wysyłkę pakietów.
//!
//! Sekwencja dołączania odtworzona z `PlayerList.placeNewPlayer`, wysyłka chunków z
//! `PlayerChunkSender`, widok chunków z `ChunkTrackingView`, ruch encji z `ServerEntity.sendChanges`.

use crate::net::{GameEvent, JoinRequest, PlayerSender, Shared, VERSION_NAME};
use crate::world::World;
use mc_data::GameData;
use mc_protocol::frame::FrameEncoder;
use mc_protocol::nbt::{Compound, Tag};
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

/// ID z oficjalnych rejestrów potrzebne w czasie gry.
pub struct Ids {
    pub dimension_type: i32,
    pub player_entity_type: i32,
    pub chat_type: i32,
    pub overworld: &'static str,
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
    // widoczne encje (sesje innych graczy)
    tracked: HashSet<u64>,
    // stan wysłany do obserwatorów (ServerEntity)
    base: (f64, f64, f64),
    last_yaw: i32,
    last_pitch: i32,
    last_head: i32,
    was_on_ground: bool,
    teleport_delay: i32,
    ticks: u64,
    // inventory (uproszczone): [slot menu] -> (id przedmiotu, ilość)
    inventory: Vec<Option<(i32, i32)>>,
    selected: i8,
    first_tick_done: bool,
    sneaking: bool,
    sprinting: bool,
    message_index: i32,
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
    game_time: i64,
    day_time: i64,
    stopping: bool,
}

fn pos_enc(v: f64) -> i64 {
    pk::entity_to_packet(v)
}

impl Game {
    pub fn new(shared: Arc<Shared>, data: Arc<GameData>, ids: Ids, world: World) -> Self {
        let enc = FrameEncoder::new(if shared.settings.compression_threshold >= 0 { Some(shared.settings.compression_threshold) } else { None });
        Self { shared, data, ids, world, enc, players: BTreeMap::new(), next_entity_id: 1, tick: 0, game_time: 0, day_time: 0, stopping: false }
    }

    pub fn run(mut self, rx: Receiver<GameEvent>) {
        let mut next = Instant::now() + TICK;
        info!("Pętla gry uruchomiona (20 TPS)");
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
                warn!("Nie nadążam! Pominięto {} ms", (now - next).as_millis());
                next = now;
            }
            if self.stopping {
                break;
            }
        }
        // zamknięcie: rozłącz wszystkich
        let sessions: Vec<u64> = self.players.keys().copied().collect();
        for s in sessions {
            self.kick(s, Text::translatable("multiplayer.disconnect.server_shutdown"));
        }
    }

    // ------------------------------------------------------------ wysyłka

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
            info!("{} został rozłączony: {}", p.name, text_plain(&reason));
            p.sender.close(Some(f));
        }
        self.remove_player(session);
    }

    // ------------------------------------------------------------ zdarzenia

    fn handle_event(&mut self, ev: GameEvent) {
        match ev {
            GameEvent::Join(req) => self.on_join(*req),
            GameEvent::Packet { session, packet } => self.on_packet(session, packet),
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
        // Player.getDisplayName: nazwa z klikaniem (/tell), podpowiedzią (show_entity) i wstawką.
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

    // ------------------------------------------------------------ dołączanie

    fn on_join(&mut self, req: JoinRequest) {
        let JoinRequest { session, profile, client_info, sender, addr } = req;
        let GameProfile { id: uuid, name, properties } = profile;

        // „Zalogowano z innej lokalizacji": stara sesja wylatuje.
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
        let gm = self.shared.settings.game_mode;
        let (sx, sy, sz) = self.world.spawn;
        let (x, y, z) = (sx as f64 + 0.5, sy as f64, sz as f64 + 0.5);

        info!("{name}[{addr}] logged in with entity id {entity_id} at ({x:.1}, {y:.1}, {z:.1})");
        let mut inventory = vec![None; 46];
        inventory[0] = None;
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
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
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
            inventory,
            selected: 0,
            first_tick_done: false,
            sneaking: false,
            sprinting: false,
            message_index: 0,
        };
        self.players.insert(session, p);

        let s = &self.shared.settings;
        // 1. Login
        let dims = [self.ids.overworld];
        let login = pk::login(&pk::LoginInfo {
            entity_id,
            hardcore: s.hardcore,
            dimensions: &dims,
            max_players: s.max_players,
            view_distance: s.view_distance,
            simulation_distance: s.simulation_distance,
            reduced_debug_info: false,
            show_death_screen: true,
            do_limited_crafting: false,
            dimension_type: self.ids.dimension_type,
            dimension: self.ids.overworld,
            hashed_seed: 0,
            game_mode: gm,
            previous_game_mode: -1,
            is_debug: false,
            is_flat: true,
            last_death: None,
            portal_cooldown: 0,
            enforces_secure_chat: crate::net::ENFORCES_SECURE_CHAT,
        });
        let difficulty = s.difficulty;
        let motd = Text::plain(s.motd.clone());
        self.send(session, &login);
        // 2-5. Trudność, uprawnienia, zaznaczony slot, receptury
        self.send(session, &pk::change_difficulty(difficulty, false));
        let ab = abilities_for(gm);
        self.send(session, &pk::player_abilities(&ab));
        self.send(session, &pk::set_carried_item(0));
        self.send(session, &pk::update_recipes_empty());
        // 6. Poziom uprawnień (entity event 24 + poziom)
        self.send(session, &pk::entity_event(entity_id, 24));
        // 8. Komunikat o dołączeniu — do graczy już na liście (nowy jeszcze się na niej nie znajduje)
        let dn = Self::display_name(&self.players[&session]);
        let join_msg = Text::translatable("multiplayer.player.joined").with_args(vec![dn]).color("yellow");
        let f = pk::system_chat(&join_msg, false);
        self.broadcast(&f, Some(session));
        // 9. Teleport (pozycja startowa)
        self.teleport(session, x, y, z, 0.0, 0.0);
        // 10. Dane serwera
        self.send(session, &pk::server_data(&motd, None));
        // 11. Lista graczy: istniejący -> nowy
        let others: Vec<u64> = self.players.keys().copied().filter(|k| *k != session).collect();
        if !others.is_empty() {
            let entries: Vec<PlayerInfoEntry<'_>> = others.iter().map(|k| Self::info_entry(&self.players[k])).collect();
            let payload = pk::player_info_update(pk::player_info_action::INITIALIZING, &entries);
            self.send(session, &payload);
        }
        // 12. nowy -> wszyscy (łącznie z nim)
        let payload = pk::player_info_update(pk::player_info_action::INITIALIZING, &[Self::info_entry(&self.players[&session])]);
        self.broadcast(&payload, None);
        // 13. sendLevelInfo
        self.send(session, &pk::initialize_border(&Border::default()));
        self.send(session, &pk::set_time(self.game_time, self.day_time, true));
        let (sx, sy, sz) = self.world.spawn;
        self.send(session, &pk::set_default_spawn_position(sx, sy, sz, 0.0));
        self.send(session, &pk::game_event(pk::game_event::LEVEL_CHUNKS_LOAD_START, 0.0));
        // 14. addNewPlayer: śledzenie chunków i encji
        self.update_chunk_tracking(session);
        self.update_entity_tracking(session);
        // 15. initInventoryMenu
        self.send(session, &pk::container_set_content_empty(0, 1, 46));
        self.sync_status();
    }

    fn remove_player(&mut self, session: u64) {
        let Some(p) = self.players.remove(&session) else { return };
        let uuid = p.uuid;
        let eid = p.entity_id;
        let dn = Self::display_name(&p);
        // zdejmij encję u obserwatorów
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
        // zamknij gniazdo (jeśli jeszcze otwarte)
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

    // ------------------------------------------------------------ chunki

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
        // różnica widoków
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
        // najbliższe najpierw (Comparators.least / sortowanie po distanceSquared)
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

    fn spawn_packets(&self, p: &Player) -> Vec<Vec<u8>> {
        let mut v = vec![pk::add_entity(
            p.entity_id,
            p.uuid,
            self.ids.player_entity_type,
            p.x,
            p.y,
            p.z,
            p.pitch,
            p.yaw,
            p.yaw,
            0,
            0,
            0,
            0,
        )];
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
        data.push((9, EntityData::Float(20.0)));
        if p.info.model_customisation != 0 {
            data.push((17, EntityData::Byte(p.info.model_customisation as i8)));
        }
        v.push(pk::set_entity_data(p.entity_id, &data));
        v
    }

    /// Widoczność wg `ChunkMap.TrackedEntity.updatePlayer`: odległość pozioma <= min(zasięg, viewDistance*16)
    /// i chunk encji w widoku obserwatora. Zasięg encji gracza = 32 chunki.
    fn visible(&self, viewer: &Player, target: &Player) -> bool {
        let dx = viewer.x - target.x;
        let dz = viewer.z - target.z;
        let range = (32.0f64 * 16.0).min(self.shared.settings.view_distance.min(viewer.info.view_distance as i32).max(2) as f64 * 16.0);
        let in_range = dx * dx + dz * dz <= range * range;
        let tracked_chunk = viewer.view.is_some_and(|v| v.contains((target.x.floor() as i32) >> 4, (target.z.floor() as i32) >> 4));
        in_range && tracked_chunk
    }

    /// Aktualizuje parowanie (spawn/despawn) między `session` a pozostałymi graczami w obu kierunkach.
    fn update_entity_tracking(&mut self, session: u64) {
        let sessions: Vec<u64> = self.players.keys().copied().filter(|s| *s != session).collect();
        for other in sessions {
            // other -> session widzi?
            let (sees_me, i_see) = {
                let me = &self.players[&session];
                let o = &self.players[&other];
                (self.visible(o, me), self.visible(me, o))
            };
            // czy other widzi mnie
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
            // czy ja widzę other
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

    /// `ServerEntity.sendChanges` dla gracza (updateInterval = 2): rozsyła zmiany pozycji/obrotu do obserwatorów.
    fn send_entity_changes(&mut self, session: u64) {
        let (payloads, observers): (Vec<Vec<u8>>, Vec<u64>) = {
            let p = self.players.get_mut(&session).unwrap();
            let t = p.ticks; // tickCount z ServerEntity (rośnie po każdym wywołaniu)
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
            // VecDeltaCodec.delta: różnica względem zdekodowanej bazy (pozycja zaokrąglona do 1/4096)
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
        self.day_time += 1;
        if self.tick % 20 == 0 {
            let f = pk::set_time(self.game_time, self.day_time, true);
            self.broadcast(&f, None);
        }
        let sessions: Vec<u64> = self.players.keys().copied().collect();
        for s in sessions {
            if !self.players.contains_key(&s) {
                continue;
            }
            self.tick_player(s);
        }
    }

    fn tick_player(&mut self, s: u64) {
        // przeciążone gniazdo
        if self.players[&s].sender.pending.load(Ordering::Relaxed) > MAX_PENDING_BYTES {
            return self.kick(s, Text::translatable("disconnect.timeout"));
        }
        // ponowne wysłanie teleportu po 20 tickach bez potwierdzenia
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
        // pierwsze wysłanie zdrowia/doświadczenia (ServerPlayer.doTick: lastSent* = nieprawidłowe)
        if !self.players[&s].first_tick_done {
            self.players.get_mut(&s).unwrap().first_tick_done = true;
            self.send(s, &pk::set_health(20.0, 20, 5.0));
            self.send(s, &pk::set_experience(0.0, 0, 0));
        }
        self.send_next_chunks(s);
        self.send_entity_changes(s);
    }

    // ------------------------------------------------------------ pakiety

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
                }
            }
            Serverbound::PlayerAction { action, x, y, z, sequence, .. } => self.on_player_action(session, action, x, y, z, sequence),
            Serverbound::UseItemOn { x, y, z, face, sequence, hand, .. } => self.on_use_item_on(session, hand, x, y, z, face, sequence),
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
            // Dopóki klient nie potwierdzi teleportu, ruch jest ignorowany (ServerGamePacketListenerImpl.handleMovePlayer)
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
            // ruch może zmienić widoczność innych graczy (w obie strony)
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
        // wiadomość niepodpisana: każdy odbiorca dostaje własny licznik `index`
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

    fn on_command(&mut self, session: u64, cmd: String) {
        info!("{} issued server command: /{}", self.players[&session].name, cmd);
        let name = cmd.split_whitespace().next().unwrap_or("");
        match name {
            "list" => {
                let names: Vec<String> = self.players.values().map(|p| p.name.clone()).collect();
                let msg = Text::translatable("commands.list.players").with_args(vec![
                    Text::plain(self.players.len().to_string()),
                    Text::plain(self.shared.settings.max_players.to_string()),
                    Text::plain(names.join(", ")),
                ]);
                self.send(session, &pk::system_chat(&msg, false));
            }
            _ => {
                // ComponentUtils.fromMessage(unknown command) + kontekst "<--[HERE]"
                let err = Text::translatable("command.unknown.command").color("red");
                self.send(session, &pk::system_chat(&err, false));
                let mut ctx = Text::plain("");
                ctx.color = Some("gray".into());
                ctx.style_nbt.push(("clickEvent".into(), Tag::Compound(Compound::new().with("action", Tag::string("suggest_command")).with("value", Tag::string(format!("/{cmd}"))))));
                let mut input = Text::plain(cmd.clone());
                input.color = Some("red".into());
                input.style_nbt.push(("underlined".into(), Tag::Byte(1)));
                let mut here = Text::translatable("command.context.here").color("red");
                here.italic = Some(true);
                ctx.extra = vec![input, here];
                self.send(session, &pk::system_chat(&ctx, false));
            }
        }
    }

    fn on_player_action(&mut self, session: u64, action: i32, x: i32, y: i32, z: i32, sequence: i32) {
        // 0 = START_DESTROY_BLOCK, 2 = STOP_DESTROY_BLOCK. W trybie przetrwania serwer vanilla weryfikuje czas
        // kopania (wymaga twardości bloków: roadmap); tu ufamy klientowi po STOP, w kreatywnym łamiemy od razu.
        let gm = self.players[&session].game_mode;
        let breaks = (action == 0 && gm == 1) || action == 2;
        if gm == 3 {
            return;
        }
        if breaks {
            let air = self.world.palette.air;
            if let Some(old) = self.world.set_block(x, y, z, air) {
                if old != air {
                    self.broadcast(&pk::block_update(x, y, z, air as u32), None);
                }
            }
        } else if action == 0 || action == 1 {
            // anulowanie/rozpoczęcie bez złamania: odeślij faktyczny stan
            let st = self.world.get_block(x, y, z);
            self.send(session, &pk::block_update(x, y, z, st as u32));
        }
        self.send(session, &pk::block_changed_ack(sequence));
    }

    fn on_use_item_on(&mut self, session: u64, hand: i32, x: i32, y: i32, z: i32, face: i32, sequence: i32) {
        let gm = self.players[&session].game_mode;
        let slot = if hand == 0 { 36 + self.players[&session].selected as usize } else { 45 };
        let held = self.players[&session].inventory.get(slot).copied().flatten();
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
        let mut changed = false;
        if let (Some((item_id, _count)), true) = (held, gm != 3 && gm != 2) {
            // przedmiot -> blok o tej samej nazwie (uproszczenie: bez getStateForPlacement)
            let name = self.data.registries.get("minecraft:item").and_then(|r| r.names.get(item_id as usize)).cloned();
            if let Some(name) = name {
                if let Some(state) = self.data.blocks.default_state(&name) {
                    let cur = self.world.get_block(px, py, pz);
                    if self.world.palette.is_air(cur) {
                        self.world.set_block(px, py, pz, state as u16);
                        self.broadcast(&pk::block_update(px, py, pz, state), None);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            // odeślij stan, by klient wycofał przewidywaną zmianę
            let st = self.world.get_block(px, py, pz);
            self.send(session, &pk::block_update(px, py, pz, st as u32));
        }
        self.send(session, &pk::block_changed_ack(sequence));
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

    /// Wartości wyliczone wywołaniem prawdziwego `ChunkTrackingView.isWithinDistance` z serwera 1.21.1
    /// (zdekompilowany jar z mapowaniami Mojang), liczba chunków w kwadracie [-view-3, view+3]^2.
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
        // isInViewDistance (bez zewnętrznego pierścienia) wg referencji: 385
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
