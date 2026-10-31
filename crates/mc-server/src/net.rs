//! Warstwa sieciowa: listener, handshake, status, login (offline + online), configuration.
//! Po `finish_configuration` połączenie przechodzi w tryb `play`: czytnik dekoduje pakiety
//! i przekazuje je do wątku gry; osobne zadanie zapisu wysyła gotowe ramki.

use crate::auth;
use crate::config::Settings;
use crate::prepare::ConfigPackets;
use anyhow::{anyhow, bail, Result};
use mc_protocol::frame::{Encryptor, FrameDecoder, FrameEncoder};
use mc_protocol::packets::play::{decode_serverbound, Serverbound};
use mc_protocol::packets::{self, ClientInformation, GameProfile, Intention, KnownPack, LoginHello, LoginKey};
use mc_protocol::text::Text;
use mc_protocol::{ids, Reader};
use rand::RngCore;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

pub const VERSION_NAME: &str = "1.21.1";
/// Port nie weryfikuje jeszcze podpisanych wiadomości, więc nie ogłasza wymuszania bezpiecznego czatu
/// (odstępstwo od domyślnego `enforce-secure-profile=true` — patrz README).
pub const ENFORCES_SECURE_CHAT: bool = false;
const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Co wysyłamy do zadania zapisu gracza.
pub enum Out {
    Frame(Arc<Vec<u8>>),
    /// Wyślij ramkę (opcjonalną) i zamknij połączenie.
    Close(Option<Arc<Vec<u8>>>),
}

#[derive(Clone)]
pub struct PlayerSender {
    pub tx: mpsc::UnboundedSender<Out>,
    pub pending: Arc<AtomicUsize>,
}
impl PlayerSender {
    pub fn send(&self, frame: Arc<Vec<u8>>) {
        self.pending.fetch_add(frame.len(), Ordering::Relaxed);
        let _ = self.tx.send(Out::Frame(frame));
    }
    pub fn close(&self, last: Option<Arc<Vec<u8>>>) {
        let _ = self.tx.send(Out::Close(last));
    }
}

pub struct JoinRequest {
    pub session: u64,
    pub profile: GameProfile,
    pub client_info: ClientInformation,
    pub sender: PlayerSender,
    pub addr: SocketAddr,
}

pub enum GameEvent {
    Join(Box<JoinRequest>),
    Packet { session: u64, packet: Serverbound },
    Disconnected { session: u64, reason: String },
    Shutdown,
}

/// Dane statusu aktualizowane przez wątek gry.
#[derive(Default)]
pub struct StatusInfo {
    pub sample: Vec<(String, uuid::Uuid)>,
}

pub struct Shared {
    pub settings: Settings,
    pub config: ConfigPackets,
    pub rsa: Option<auth::ServerKey>,
    pub events: std::sync::mpsc::Sender<GameEvent>,
    pub online: AtomicI32,
    pub status: Mutex<StatusInfo>,
    pub favicon: Option<String>,
    pub next_session: std::sync::atomic::AtomicU64,
}

pub async fn serve(shared: Arc<Shared>) -> Result<()> {
    let ip = if shared.settings.ip.is_empty() { "0.0.0.0" } else { &shared.settings.ip };
    let listener = TcpListener::bind((ip, shared.settings.port)).await?;
    info!("Nasłuchuję na {}:{}", ip, shared.settings.port);
    loop {
        let (stream, addr) = listener.accept().await?;
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, addr, shared).await {
                debug!("{addr}: połączenie zakończone: {e}");
            }
        });
    }
}

struct Conn {
    rd: OwnedReadHalf,
    wr: OwnedWriteHalf,
    dec: FrameDecoder,
    enc: FrameEncoder,
    cipher: Option<Encryptor>,
    buf: Box<[u8; 16384]>,
}

impl Conn {
    async fn read_packet(&mut self) -> Result<(i32, Vec<u8>)> {
        loop {
            if let Some(p) = self.dec.next_packet()? {
                return Ok(p);
            }
            let n = self.rd.read(&mut self.buf[..]).await?;
            if n == 0 {
                bail!("EOF");
            }
            self.dec.feed(&self.buf[..n]);
        }
    }
    async fn read_packet_timeout(&mut self) -> Result<(i32, Vec<u8>)> {
        tokio::time::timeout(LOGIN_TIMEOUT, self.read_packet()).await.map_err(|_| anyhow!("timeout"))?
    }
    async fn send(&mut self, payload: &[u8]) -> Result<()> {
        let mut f = self.enc.encode(payload);
        if let Some(c) = &mut self.cipher {
            c.encrypt(&mut f);
        }
        self.wr.write_all(&f).await?;
        Ok(())
    }
}

async fn handle(stream: TcpStream, addr: SocketAddr, shared: Arc<Shared>) -> Result<()> {
    stream.set_nodelay(true)?;
    let (rd, wr) = stream.into_split();
    let mut c = Conn { rd, wr, dec: FrameDecoder::new(), enc: FrameEncoder::new(None), cipher: None, buf: Box::new([0; 16384]) };

    let (id, body) = c.read_packet_timeout().await?;
    if id != ids::handshake::serverbound::INTENTION {
        bail!("pierwszy pakiet nie jest handshake: {id}");
    }
    let intent = Intention::decode(&mut Reader::new(&body))?;
    match intent.next_state {
        1 => {
            if !shared.settings.enable_status {
                return Ok(());
            }
            status(&mut c, &shared).await
        }
        2 | 3 => {
            let transfer = intent.next_state == 3;
            if transfer {
                // accepts-transfers=false (domyślnie)
                let reason = Text::translatable("multiplayer.disconnect.transfers_disabled");
                c.send(&packets::login_disconnect(&reason)).await?;
                return Ok(());
            }
            if intent.protocol_version != ids::PROTOCOL_VERSION {
                // dokładnie jak ServerHandshakePacketListenerImpl.beginLogin
                let key = if intent.protocol_version < 754 { "multiplayer.disconnect.outdated_client" } else { "multiplayer.disconnect.incompatible" };
                let reason = Text::translatable(key).with_args(vec![Text::plain(VERSION_NAME)]);
                c.send(&packets::login_disconnect(&reason)).await?;
                return Ok(());
            }
            login_and_play(c, addr, shared).await
        }
        n => bail!("nieprawidłowy next_state {n}"),
    }
}

// ------------------------------------------------------------------- status

fn status_json(shared: &Shared) -> String {
    use serde_json::{json, Map, Value};
    let s = &shared.settings;
    let mut m = Map::new();
    m.insert("description".into(), Text::plain(&s.motd).to_json());
    let mut players = Map::new();
    players.insert("max".into(), json!(s.max_players));
    players.insert("online".into(), json!(shared.online.load(Ordering::Relaxed)));
    if !s.hide_online_players {
        let st = shared.status.lock().unwrap();
        if !st.sample.is_empty() {
            let sample: Vec<Value> = st.sample.iter().take(12).map(|(n, u)| json!({"id": u.to_string(), "name": n})).collect();
            players.insert("sample".into(), Value::Array(sample));
        }
    }
    m.insert("players".into(), Value::Object(players));
    m.insert("version".into(), json!({"name": VERSION_NAME, "protocol": ids::PROTOCOL_VERSION}));
    if let Some(f) = &shared.favicon {
        m.insert("favicon".into(), Value::String(f.clone()));
    }
    // `enforcesSecureChat` ma wartość domyślną false i jest pomijane przy false (kodek vanilla)
    if ENFORCES_SECURE_CHAT {
        m.insert("enforcesSecureChat".into(), Value::Bool(true));
    }
    Value::Object(m).to_string()
}

async fn status(c: &mut Conn, shared: &Shared) -> Result<()> {
    let mut requested = false;
    loop {
        let (id, body) = c.read_packet_timeout().await?;
        match id {
            ids::status::serverbound::STATUS_REQUEST => {
                if requested {
                    bail!("podwójne żądanie statusu");
                }
                requested = true;
                c.send(&packets::status_response(&status_json(shared))).await?;
            }
            ids::status::serverbound::PING_REQUEST => {
                let t = Reader::new(&body).i64()?;
                c.send(&packets::pong_response(t)).await?;
                return Ok(());
            }
            other => bail!("nieoczekiwany pakiet statusu {other}"),
        }
    }
}

// -------------------------------------------------------------------- login

fn valid_player_name(n: &str) -> bool {
    n.chars().count() <= 16 && n.chars().all(|c| (c as u32) > 32 && (c as u32) < 127)
}

async fn login_and_play(mut c: Conn, addr: SocketAddr, shared: Arc<Shared>) -> Result<()> {
    let (id, body) = c.read_packet_timeout().await?;
    if id != ids::login::serverbound::HELLO {
        bail!("oczekiwano login hello, jest {id}");
    }
    let hello = LoginHello::decode(&mut Reader::new(&body))?;
    if !valid_player_name(&hello.name) {
        bail!("nieprawidłowy nick");
    }

    let profile = if shared.settings.online_mode {
        let key = shared.rsa.as_ref().expect("online-mode wymaga klucza RSA");
        let mut challenge = [0u8; 4];
        rand::thread_rng().fill_bytes(&mut challenge);
        c.send(&packets::login_encryption_request("", &key.public_der, &challenge, true)).await?;
        let (id, body) = c.read_packet_timeout().await?;
        if id != ids::login::serverbound::KEY {
            bail!("oczekiwano login key, jest {id}");
        }
        let k = LoginKey::decode(&mut Reader::new(&body))?;
        let (secret, ok) = key.decrypt(&k.shared_secret, &k.challenge, &challenge)?;
        if !ok {
            bail!("Protocol error: zły token weryfikacyjny");
        }
        // Od tego momentu strumień jest szyfrowany AES-128-CFB8 w obu kierunkach.
        c.dec.enable_encryption(&secret);
        c.cipher = Some(Encryptor::new(&secret));
        let hash = auth::server_hash("", &secret, &key.public_der);
        let name = hello.name.clone();
        let prevent_proxy = None::<std::net::IpAddr>;
        match tokio::task::spawn_blocking(move || auth::has_joined(&name, &hash, prevent_proxy)).await? {
            Ok(Some(p)) => {
                info!("UUID gracza {} to {}", p.name, p.id);
                p
            }
            Ok(None) => {
                let reason = Text::translatable("multiplayer.disconnect.unverified_username");
                c.send(&packets::login_disconnect(&reason)).await?;
                bail!("sesja niepoprawna dla {}", hello.name);
            }
            Err(e) => {
                warn!("Nie udało się zweryfikować sesji: {e}");
                let reason = Text::translatable("multiplayer.disconnect.authservers_down");
                c.send(&packets::login_disconnect(&reason)).await?;
                bail!("serwery uwierzytelniania niedostępne");
            }
        }
    } else {
        GameProfile { id: mc_protocol::buf::offline_uuid(&hello.name), name: hello.name.clone(), properties: vec![] }
    };

    // canPlayerLogin: pełny serwer
    if shared.online.load(Ordering::Relaxed) >= shared.settings.max_players {
        c.send(&packets::login_disconnect(&Text::translatable("multiplayer.disconnect.server_full"))).await?;
        return Ok(());
    }

    // kompresja
    let threshold = shared.settings.compression_threshold;
    if threshold >= 0 {
        c.send(&packets::login_compression(threshold)).await?;
        c.enc = FrameEncoder::new(Some(threshold));
        c.dec.set_compression(threshold);
    }
    c.send(&packets::login_success(&profile, true)).await?;

    let (id, _) = c.read_packet_timeout().await?;
    if id != ids::login::serverbound::LOGIN_ACKNOWLEDGED {
        bail!("oczekiwano login_acknowledged, jest {id}");
    }

    let client_info = configuration(&mut c, &shared).await?;
    play(c, addr, shared, profile, client_info).await
}

// ------------------------------------------------------------ configuration

/// Faza configuration, kolejność jak `ServerConfigurationPacketListenerImpl.startConfiguration`:
/// brand -> enabled features -> select_known_packs -> (odpowiedź) registry_data* + update_tags -> finish.
async fn configuration(c: &mut Conn, shared: &Shared) -> Result<ClientInformation> {
    let cfg = &shared.config;
    c.send(&cfg.brand).await?;
    c.send(&cfg.features).await?;
    c.send(&cfg.select_known_packs).await?;

    let mut info = ClientInformation::default();
    let mut registries_sent = false;
    loop {
        let (id, body) = c.read_packet_timeout().await?;
        use ids::configuration::serverbound as sb;
        match id {
            sb::CLIENT_INFORMATION => info = ClientInformation::decode(&mut Reader::new(&body))?,
            sb::SELECT_KNOWN_PACKS => {
                if registries_sent {
                    bail!("podwójna odpowiedź known packs");
                }
                let theirs = packets::decode_known_packs(&mut Reader::new(&body))?;
                // Jak SynchronizeRegistriesTask.handleResponse: wpisy bez danych tylko przy dokładnie tej samej liście.
                let known: &[KnownPack] = &cfg.known_packs;
                let use_known = theirs.as_slice() == known;
                let regs = if use_known { &cfg.registry_known } else { &cfg.registry_full };
                for r in regs {
                    c.send(r).await?;
                }
                c.send(&cfg.tags).await?;
                c.send(&cfg.finish).await?;
                registries_sent = true;
            }
            sb::FINISH_CONFIGURATION => {
                if !registries_sent {
                    bail!("finish_configuration przed synchronizacją rejestrów");
                }
                return Ok(info);
            }
            sb::CUSTOM_PAYLOAD | sb::KEEP_ALIVE | sb::PONG | sb::RESOURCE_PACK | sb::COOKIE_RESPONSE => {}
            other => bail!("nieoczekiwany pakiet configuration {other}"),
        }
    }
}

// --------------------------------------------------------------------- play

async fn play(c: Conn, addr: SocketAddr, shared: Arc<Shared>, profile: GameProfile, client_info: ClientInformation) -> Result<()> {
    let session = shared.next_session.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let pending = Arc::new(AtomicUsize::new(0));
    let sender = PlayerSender { tx, pending: pending.clone() };

    let Conn { mut rd, mut wr, mut dec, cipher, mut buf, .. } = c;
    let closed = Arc::new(tokio::sync::Notify::new());
    let closed_w = closed.clone();

    // Zadanie zapisu: scala dostępne ramki w jeden zapis, szyfruje i wysyła.
    let writer = tokio::spawn(async move {
        let mut cipher = cipher;
        let mut out: Vec<u8> = Vec::with_capacity(64 * 1024);
        'outer: while let Some(msg) = rx.recv().await {
            out.clear();
            let mut close = false;
            let handle = |m: Out, out: &mut Vec<u8>, close: &mut bool| match m {
                Out::Frame(f) => {
                    pending.fetch_sub(f.len(), Ordering::Relaxed);
                    out.extend_from_slice(&f);
                }
                Out::Close(last) => {
                    if let Some(f) = last {
                        out.extend_from_slice(&f);
                    }
                    *close = true;
                }
            };
            handle(msg, &mut out, &mut close);
            while !close {
                match rx.try_recv() {
                    Ok(m) => handle(m, &mut out, &mut close),
                    Err(_) => break,
                }
            }
            if let Some(c) = &mut cipher {
                c.encrypt(&mut out);
            }
            if wr.write_all(&out).await.is_err() {
                break 'outer;
            }
            if close {
                let _ = wr.shutdown().await;
                break;
            }
        }
        closed_w.notify_one();
    });

    shared
        .events
        .send(GameEvent::Join(Box::new(JoinRequest { session, profile, client_info, sender, addr })))
        .map_err(|_| anyhow!("serwer się zamyka"))?;

    // Czytnik: dekoduje pakiety play i przekazuje do wątku gry.
    let reason = loop {
        match dec.next_packet() {
            Ok(Some((id, body))) => match decode_serverbound(id, &body) {
                Ok(packet) => {
                    if shared.events.send(GameEvent::Packet { session, packet }).is_err() {
                        break "serwer zamknięty".to_string();
                    }
                }
                Err(e) => break format!("błąd dekodowania pakietu {id:#x}: {e}"),
            },
            Ok(None) => {
                tokio::select! {
                    r = rd.read(&mut buf[..]) => match r {
                        Ok(0) => break "EOF".to_string(),
                        Ok(n) => dec.feed(&buf[..n]),
                        Err(e) => break e.to_string(),
                    },
                    _ = closed.notified() => break "połączenie zamknięte przez serwer".to_string(),
                }
            }
            Err(e) => break format!("błąd ramki: {e}"),
        }
    };
    let _ = shared.events.send(GameEvent::Disconnected { session, reason });
    writer.abort();
    Ok(())
}
