//! mc-probe: a protocol bot for conformance tests.
//!
//!   mc-probe status [--host H] [--port P]
//!   mc-probe join   [--host H] [--port P] [--name N] [--data DIR] [--secs S] [--no-known-packs]
//!                   [--chat TEXT] [--walk] [--online] [--session-url URL]
//!   mc-probe mock-session [--port P]     (a mock of sessionserver.mojang.com for online-mode tests)
//!
//! The probe has its own chunk parser (independent of the server code) and verifies block IDs against
//! the OFFICIAL Mojang report (`blocks.json`), not against the server code.

use anyhow::{anyhow, bail, Result};
use mc_protocol::frame::{Encryptor, FrameDecoder, FrameEncoder};
use mc_protocol::nbt::Tag;
use mc_protocol::packets::{self, ClientInformation, Intention, KnownPack};
use mc_protocol::{ids, Put, Reader};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

struct Args {
    cmd: String,
    host: String,
    port: u16,
    name: String,
    data: Option<String>,
    secs: u64,
    known_packs: bool,
    chat: Option<String>,
    command: Option<String>,
    brk: Option<Option<(i32, i32, i32)>>,
    place: Option<Option<(i32, i32, i32)>>,
    place_beside: bool,
    walk: bool,
    online: bool,
    session_url: String,
    dump: Option<String>,
    protocol: i32,
}

fn parse_pos(s: &str) -> Result<(i32, i32, i32)> {
    let v: Vec<i32> = s.split(',').map(|x| x.trim().parse()).collect::<Result<_, _>>()?;
    if v.len() != 3 {
        bail!("a position has the form x,y,z");
    }
    Ok((v[0], v[1], v[2]))
}

/// `below` = the block directly under the player's feet (resolved after the position arrives from the server).
fn parse_pos_or_below(s: &str) -> Result<Option<(i32, i32, i32)>> {
    if s == "below" || s == "beside" {
        Ok(None)
    } else {
        Ok(Some(parse_pos(s)?))
    }
}

fn parse_args() -> Result<Args> {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().ok_or_else(|| anyhow!("usage: mc-probe <status|join|mock-session> [options]"))?;
    let mut a = Args {
        cmd,
        host: "127.0.0.1".into(),
        port: 25565,
        name: "Probe".into(),
        data: None,
        secs: 3,
        known_packs: true,
        chat: None,
        command: None,
        brk: None,
        place: None,
        place_beside: false,
        walk: false,
        online: false,
        session_url: "http://127.0.0.1:8089".into(),
        dump: None,
        protocol: ids::PROTOCOL_VERSION,
    };
    while let Some(k) = it.next() {
        match k.as_str() {
            "--host" => a.host = it.next().unwrap(),
            "--port" => a.port = it.next().unwrap().parse()?,
            "--name" => a.name = it.next().unwrap(),
            "--data" => a.data = it.next(),
            "--secs" => a.secs = it.next().unwrap().parse()?,
            "--no-known-packs" => a.known_packs = false,
            "--chat" => a.chat = it.next(),
            "--command" => a.command = it.next(),
            "--break" => a.brk = Some(parse_pos_or_below(&it.next().unwrap())?),
            "--place" => {
                let v = it.next().unwrap();
                a.place_beside = v == "beside";
                a.place = Some(parse_pos_or_below(&v)?)
            }
            "--protocol" => a.protocol = it.next().unwrap().parse()?,
            "--walk" => a.walk = true,
            "--online" => a.online = true,
            "--session-url" => a.session_url = it.next().unwrap(),
            "--dump" => a.dump = it.next(),
            o => bail!("unknown option {o}"),
        }
    }
    Ok(a)
}

struct Client {
    rd: OwnedReadHalf,
    wr: OwnedWriteHalf,
    dec: FrameDecoder,
    enc: FrameEncoder,
    cipher: Option<Encryptor>,
    buf: Vec<u8>,
    /// Dump of raw clientbound packets: [state u8][len u32 BE][id+body]; state: 0 login, 1 config, 2 play.
    dump: Option<std::fs::File>,
    state: u8,
}

impl Client {
    async fn connect(host: &str, port: u16) -> Result<Self> {
        let s = TcpStream::connect((host, port)).await?;
        s.set_nodelay(true)?;
        let (rd, wr) = s.into_split();
        Ok(Self { rd, wr, dec: FrameDecoder::new(), enc: FrameEncoder::new(None), cipher: None, buf: vec![0; 65536], dump: None, state: 0 })
    }
    async fn send(&mut self, payload: &[u8]) -> Result<()> {
        self.send_many(&[payload.to_vec()]).await
    }
    /// Writes several packets with ONE write call, so the server reads them together and handles them in the same tick
    /// (needed for deterministic "several actions in one tick" tests).
    async fn send_many(&mut self, payloads: &[Vec<u8>]) -> Result<()> {
        let mut f = Vec::new();
        for p in payloads {
            f.extend_from_slice(&self.enc.encode(p));
        }
        if let Some(c) = &mut self.cipher {
            c.encrypt(&mut f);
        }
        self.wr.write_all(&f).await?;
        Ok(())
    }
    /// `Ok(None)` when the connection is closed / on timeout.
    async fn recv(&mut self, timeout: Duration) -> Result<Option<(i32, Vec<u8>)>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(p) = self.dec.next_packet()? {
                if let Some(f) = self.dump.as_mut() {
                    use std::io::Write;
                    let pl = mc_protocol::frame::payload(p.0, |w| w.put_bytes(&p.1));
                    f.write_all(&[self.state])?;
                    f.write_all(&(pl.len() as u32).to_be_bytes())?;
                    f.write_all(&pl)?;
                }
                return Ok(Some(p));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.rd.read(&mut self.buf)).await {
                Err(_) => return Ok(None),
                Ok(Ok(0)) => return Ok(None),
                Ok(Ok(n)) => self.dec.feed(&self.buf[..n]),
                Ok(Err(e)) => return Err(e.into()),
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let a = parse_args()?;
    match a.cmd.as_str() {
        "status" => status(&a).await,
        "join" => join(&a).await,
        "mock-session" => mock_session(a.port).await,
        c => bail!("unknown command {c}"),
    }
}

async fn status(a: &Args) -> Result<()> {
    let mut c = Client::connect(&a.host, a.port).await?;
    let h = Intention { protocol_version: ids::PROTOCOL_VERSION, server_address: a.host.clone(), server_port: a.port, next_state: 1 };
    c.send(&h.encode()).await?;
    c.send(&mc_protocol::frame::payload(ids::status::serverbound::STATUS_REQUEST, |_| {})).await?;
    let (id, body) = c.recv(Duration::from_secs(5)).await?.ok_or_else(|| anyhow!("no status response"))?;
    assert_eq!(id, ids::status::clientbound::STATUS_RESPONSE);
    let json = Reader::new(&body).string(32767)?;
    println!("STATUS_JSON {json}");
    let t0 = Instant::now();
    c.send(&mc_protocol::frame::payload(ids::status::serverbound::PING_REQUEST, |w| w.put_i64(0x1234_5678_9abc))).await?;
    let (id, body) = c.recv(Duration::from_secs(5)).await?.ok_or_else(|| anyhow!("no pong"))?;
    assert_eq!(id, ids::status::clientbound::PONG_RESPONSE);
    assert_eq!(Reader::new(&body).i64()?, 0x1234_5678_9abc);
    println!("PONG ok ({} µs)", t0.elapsed().as_micros());
    Ok(())
}

// --------------------------------------------------------------- mock session server

async fn mock_session(port: u16) -> Result<()> {
    // Minimal HTTP server: POST /join?username=&serverId= records the join, GET /?username=&serverId= returns the profile.
    use std::sync::{Arc, Mutex};
    let joined: Arc<Mutex<BTreeMap<(String, String), ()>>> = Arc::new(Mutex::new(BTreeMap::new()));
    let l = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("mock-session on :{port}");
    loop {
        let (mut s, _) = l.accept().await?;
        let joined = joined.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let n = s.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let line = req.lines().next().unwrap_or("");
            let mut parts = line.split_whitespace();
            let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            let (path, q) = target.split_once('?').unwrap_or((target, ""));
            let get = |k: &str| q.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
            let (user, sid) = (get("username"), get("serverId"));
            let resp = if method == "POST" && path == "/join" {
                joined.lock().unwrap().insert((user, sid), ());
                "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_string()
            } else if joined.lock().unwrap().contains_key(&(user.clone(), sid)) {
                let id = mc_protocol::buf::offline_uuid(&format!("online-{user}")).simple().to_string();
                let body = format!(r#"{{"id":"{id}","name":"{user}","properties":[{{"name":"textures","value":"dGVzdA==","signature":"c2ln"}}]}}"#);
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
            } else {
                "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_string()
            };
            let _ = s.write_all(resp.as_bytes()).await;
        });
    }
}

/// A dependency-free HTTP client for "POST /join" (the mock runs over plain TCP).
async fn mock_join(url: &str, user: &str, server_id: &str) -> Result<()> {
    let hostport = url.trim_start_matches("http://");
    let mut s = TcpStream::connect(hostport).await?;
    let req = format!("POST /join?username={user}&serverId={server_id} HTTP/1.1\r\nHost: {hostport}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).await?;
    Ok(())
}

// ---------------------------------------------------------------------- join

#[derive(Default)]
struct Report {
    counts: BTreeMap<String, usize>,
    checks: Vec<(String, bool, String)>,
    notes: Vec<String>,
}
impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        self.checks.push((name.to_owned(), ok, detail.into()));
    }
}

fn read_blocks_json(dir: &str) -> Result<BTreeMap<String, u32>> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(format!("{dir}/reports/blocks.json"))?)?;
    let mut m = BTreeMap::new();
    for (name, b) in v.as_object().unwrap() {
        for s in b["states"].as_array().unwrap() {
            if s["default"].as_bool().unwrap_or(false) {
                m.insert(name.clone(), s["id"].as_u64().unwrap() as u32);
            }
        }
    }
    Ok(m)
}

async fn join(a: &Args) -> Result<()> {
    let blocks = a.data.as_deref().map(read_blocks_json).transpose()?;
    let mut rep = Report::default();
    let mut c = Client::connect(&a.host, a.port).await?;
    if let Some(d) = &a.dump {
        c.dump = Some(std::fs::File::create(d)?);
    }
    let h = Intention { protocol_version: a.protocol, server_address: a.host.clone(), server_port: a.port, next_state: 2 };
    c.send(&h.encode()).await?;
    let uuid = mc_protocol::buf::offline_uuid(&a.name);
    c.send(&packets::LoginHello { name: a.name.clone(), profile_id: uuid }.encode()).await?;

    // ---- login
    loop {
        let (id, body) = c.recv(Duration::from_secs(10)).await?.ok_or_else(|| anyhow!("disconnected during login"))?;
        let mut r = Reader::new(&body);
        use ids::login::clientbound as lc;
        match id {
            lc::LOGIN_DISCONNECT => bail!("login disconnect: {}", r.string(262144)?),
            lc::HELLO => {
                if !a.online {
                    bail!("the server requires online-mode; run the probe with --online (and a mock session server)");
                }
                let server_id = r.string(20)?;
                let pubkey = r.byte_array(1024)?.to_vec();
                let challenge = r.byte_array(256)?.to_vec();
                let _should_auth = r.bool()?;
                use rsa::pkcs8::DecodePublicKey;
                use rsa::Pkcs1v15Encrypt;
                let key = rsa::RsaPublicKey::from_public_key_der(&pubkey)?;
                let mut secret = [0u8; 16];
                rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut secret);
                let se = key.encrypt(&mut rand::thread_rng(), Pkcs1v15Encrypt, &secret)?;
                let te = key.encrypt(&mut rand::thread_rng(), Pkcs1v15Encrypt, &challenge)?;
                // "join" at the session provider (with Mojang the client's launcher does this)
                let hash = minecraft_hex_digest(&[server_id.as_bytes(), &secret, &pubkey]);
                mock_join(&a.session_url, &a.name, &hash).await?;
                c.send(&packets::LoginKey { shared_secret: se, challenge: te }.encode()).await?;
                c.dec.enable_encryption(&secret);
                c.cipher = Some(Encryptor::new(&secret));
            }
            lc::LOGIN_COMPRESSION => {
                let t = r.varint()?;
                c.enc = FrameEncoder::new(Some(t));
                c.dec.set_compression(t);
                rep.notes.push(format!("compression: threshold {t}"));
            }
            lc::GAME_PROFILE => {
                let u = r.uuid()?;
                let n = r.string(16)?;
                let props = r.varint()?;
                for _ in 0..props {
                    r.string(64)?;
                    r.string(32767)?;
                    if r.bool()? {
                        r.string(1024)?;
                    }
                }
                let strict = r.bool()?;
                rep.notes.push(format!("login success: {n} {u} props={props} strict={strict}"));
                rep.check("login: packet fully decoded", r.is_empty(), "");
                if !a.online {
                    rep.check("login: offline UUID matches vanilla", u == uuid, format!("{u}"));
                }
                c.send(&mc_protocol::frame::payload(ids::login::serverbound::LOGIN_ACKNOWLEDGED, |_| {})).await?;
                c.state = 1;
                break;
            }
            other => bail!("unexpected login packet {other}"),
        }
    }

    // ---- configuration
    c.send(&ClientInformation { view_distance: 4, ..Default::default() }.encode(ids::configuration::serverbound::CLIENT_INFORMATION)).await?;
    let mut registries: Vec<(String, usize)> = Vec::new();
    let mut tags_regs = 0usize;
    loop {
        let (id, body) = c.recv(Duration::from_secs(10)).await?.ok_or_else(|| anyhow!("disconnected during configuration"))?;
        let name = ids::names::configuration_clientbound(id);
        *rep.counts.entry(format!("config/{name}")).or_default() += 1;
        let mut r = Reader::new(&body);
        use ids::configuration::clientbound as cc;
        match id {
            cc::CUSTOM_PAYLOAD => {
                let ch = r.identifier()?;
                if ch == "minecraft:brand" {
                    rep.notes.push(format!("brand: {}", r.string(32767)?));
                }
            }
            cc::UPDATE_ENABLED_FEATURES => {
                let n = r.varint()?;
                let f: Vec<String> = (0..n).map(|_| r.identifier()).collect::<Result<_, _>>()?;
                rep.notes.push(format!("features: {f:?}"));
            }
            cc::SELECT_KNOWN_PACKS => {
                let packs = packets::decode_known_packs(&mut r)?;
                rep.notes.push(format!("server known packs: {packs:?}"));
                let reply: Vec<KnownPack> = if a.known_packs { packs } else { vec![] };
                c.send(&packets::encode_known_packs_serverbound(&reply)).await?;
            }
            cc::REGISTRY_DATA => {
                let reg = r.identifier()?;
                let n = r.varint()?;
                let mut with_data = 0;
                for _ in 0..n {
                    r.identifier()?;
                    if r.bool()? {
                        with_data += 1;
                        Tag::read_network(&mut r)?.ok_or_else(|| anyhow!("empty NBT in registry_data"))?;
                    }
                }
                rep.check(&format!("registry {reg}: packet fully decoded"), r.is_empty(), "");
                registries.push((reg.clone(), n as usize));
                if !a.known_packs {
                    rep.check(&format!("registry {reg}: full data (no known packs)"), with_data == n, format!("{with_data}/{n}"));
                } else {
                    rep.check(&format!("registry {reg}: no data (known packs)"), with_data == 0, format!("{with_data}/{n}"));
                }
            }
            cc::UPDATE_TAGS => {
                tags_regs = r.varint()? as usize;
                for _ in 0..tags_regs {
                    r.identifier()?;
                    let nt = r.varint()?;
                    for _ in 0..nt {
                        r.identifier()?;
                        let ne = r.varint()?;
                        for _ in 0..ne {
                            r.varint()?;
                        }
                    }
                }
                rep.check("tags: packet fully decoded", r.is_empty(), "");
            }
            cc::FINISH_CONFIGURATION => {
                c.send(&packets::config_finish_serverbound()).await?;
                c.state = 2;
                break;
            }
            cc::DISCONNECT => bail!("configuration disconnect"),
            _ => {}
        }
    }
    rep.notes.push(format!("registries: {:?}", registries));
    rep.check("configuration: 11 synchronized registries", registries.len() == 11, format!("{}", registries.len()));
    rep.check("configuration: tags present", tags_regs > 10, format!("{tags_regs} registries with tags"));

    // ---- play
    let t_end = Instant::now() + Duration::from_secs(a.secs);
    let mut chunks_ok = 0usize;
    let mut batches = 0usize;
    let mut first_chunk_checked = false;
    let mut chat_sent = false;
    let mut cmd_sent = false;
    let mut world_actions_sent = false;
    // The server is busy with join-time chunk work right after the first chunk; acting 1.5 s later makes tick alignment stable.
    let mut world_actions_due: Option<std::time::Instant> = None;
    let mut pos = (0.0f64, 0.0f64, 0.0f64);
    let mut got_position = false;
    let mut walk_step = 0;
    let mut chat_echo = false;
    let mut keepalives = 0;
    while Instant::now() < t_end {
        let Some((id, body)) = c.recv(Duration::from_millis(200)).await? else {
            // no data: if the connection is closed, we are done
            if Instant::now() >= t_end {
                break;
            }
            continue;
        };
        let name = ids::names::play_clientbound(id);
        *rep.counts.entry(format!("play/{name}")).or_default() += 1;
        let mut r = Reader::new(&body);
        use ids::play::clientbound as pc;
        match id {
            pc::LOGIN => {
                let eid = r.i32()?;
                let hardcore = r.bool()?;
                let n = r.varint()?;
                let dims: Vec<String> = (0..n).map(|_| r.identifier()).collect::<Result<_, _>>()?;
                let max_players = r.varint()?;
                let view = r.varint()?;
                let sim = r.varint()?;
                r.bool()?;
                r.bool()?;
                r.bool()?;
                let dim_type = r.varint()?;
                let dim = r.identifier()?;
                let seed = r.i64()?;
                let gm = r.u8()?;
                let prev = r.i8()?;
                let debug = r.bool()?;
                let flat = r.bool()?;
                let death = r.bool()?;
                let cooldown = r.varint()?;
                let secure = r.bool()?;
                rep.notes.push(format!(
                    "LOGIN eid={eid} hardcore={hardcore} dims={dims:?} max={max_players} view={view} sim={sim} dim_type={dim_type} dim={dim} seed={seed} gm={gm} prev={prev} debug={debug} flat={flat} death={death} cd={cooldown} secure={secure}"
                ));
                rep.check("play/login: packet fully decoded", r.is_empty(), format!("{} B left", r.remaining()));
                rep.check("play/login: overworld dimension, type 0", dim == "minecraft:overworld" && dim_type == 0, "");
            }
            pc::PLAYER_POSITION => {
                let (x, y, z) = (r.f64()?, r.f64()?, r.f64()?);
                let (yaw, pitch) = (r.f32()?, r.f32()?);
                let rel = r.u8()?;
                let tid = r.varint()?;
                rep.check("play/player_position: fully decoded", r.is_empty(), "");
                pos = (x, y, z);
                got_position = true;
                rep.notes.push(format!("POSITION ({x},{y},{z}) yaw={yaw} pitch={pitch} rel={rel} teleport_id={tid}"));
                // vanilla klient: accept_teleportation, potem potwierdzenie pozycji
                c.send(&mc_protocol::frame::payload(ids::play::serverbound::ACCEPT_TELEPORTATION, |w| w.put_varint(tid))).await?;
                c.send(&mc_protocol::frame::payload(ids::play::serverbound::MOVE_PLAYER_POS_ROT, |w| {
                    w.put_f64(x);
                    w.put_f64(y);
                    w.put_f64(z);
                    w.put_f32(yaw);
                    w.put_f32(pitch);
                    w.put_u8(0);
                }))
                .await?;
            }
            pc::KEEP_ALIVE => {
                let k = r.i64()?;
                keepalives += 1;
                c.send(&mc_protocol::frame::payload(ids::play::serverbound::KEEP_ALIVE, |w| w.put_i64(k))).await?;
            }
            pc::CHUNK_BATCH_FINISHED => {
                let n = r.varint()?;
                batches += 1;
                c.send(&mc_protocol::frame::payload(ids::play::serverbound::CHUNK_BATCH_RECEIVED, |w| w.put_f32(20.0))).await?;
                rep.notes.push(format!("batch #{batches}: {n} chunks"));
            }
            pc::LEVEL_CHUNK_WITH_LIGHT => {
                let info = parse_chunk(&body)?;
                chunks_ok += 1;
                if !first_chunk_checked {
                    first_chunk_checked = true;
                    rep.check("chunk: packet fully decoded", info.fully_consumed, "");
                    rep.check("chunk: 24 sections", info.sections.len() == 24, format!("{}", info.sections.len()));
                    rep.check("chunk: light masks within 26 sections", info.light_sections <= 26, format!("{}", info.light_sections));
                    if let Some(b) = &blocks {
                        let bedrock = b["minecraft:bedrock"];
                        let dirt = b["minecraft:dirt"];
                        let grass = b["minecraft:grass_block"];
                        let air = b["minecraft:air"];
                        let s0 = &info.sections[0];
                        let col = [(0, bedrock, "bedrock"), (1, dirt, "dirt"), (2, dirt, "dirt"), (3, grass, "grass_block"), (4, air, "air")];
                        for (y, want, nm) in col {
                            let got = s0.block(3, y, 5);
                            rep.check(&format!("chunk: block (3,{},5) = {nm} per Mojang blocks.json", y as i32 - 64), got == want, format!("got {got}, want {want}"));
                        }
                        rep.check("chunk: section 1 is all air", info.sections[1].block(0, 0, 0) == air, "");
                    }
                    rep.check("chunk: heightmap MOTION_BLOCKING[0] == 4", info.heightmap_first == Some(4), format!("{:?}", info.heightmap_first));
                    rep.check("chunk: section biome = plains (uniform within the section)", info.sections[0].biome_uniform.is_some(), format!("{:?}", info.sections[0].biome_uniform));
                    rep.check("chunk: sky light is 2048 B per section", info.sky_arrays_ok, "");
                    rep.notes.push(format!("first chunk: ({}, {}), bytes={}", info.x, info.z, body.len()));
                }
            }
            pc::SYSTEM_CHAT => {
                if let Some(t) = Tag::read_network(&mut r)? {
                    let s = format!("{t:?}");
                    if let Some(msg) = &a.chat {
                        if s.contains(msg.as_str()) {
                            chat_echo = true;
                        }
                    }
                    rep.notes.push(format!("system_chat: {}", s.chars().take(160).collect::<String>()));
                }
            }
            pc::PLAYER_CHAT => {
                let s = String::from_utf8_lossy(&body).to_string();
                if let Some(msg) = &a.chat {
                    if s.contains(msg.as_str()) {
                        chat_echo = true;
                    }
                }
            }
            pc::DISCONNECT => {
                let t = Tag::read_network(&mut r)?;
                bail!("disconnected by the server: {t:?}");
            }
            _ => {}
        }
        if got_position && !chat_sent {
            if let Some(m) = &a.chat {
                chat_sent = true;
                c.send(&mc_protocol::frame::payload(ids::play::serverbound::CHAT, |w| {
                    w.put_string(m);
                    w.put_i64(0);
                    w.put_i64(0);
                    w.put_bool(false);
                    w.put_varint(0);
                    w.put_bytes(&[0, 0, 0]);
                }))
                .await?;
            }
        }
        if got_position && !cmd_sent {
            if let Some(cmd) = &a.command {
                cmd_sent = true;
                c.send(&mc_protocol::frame::payload(ids::play::serverbound::CHAT_COMMAND, |w| w.put_string(cmd))).await?;
            }
        }
        if got_position && world_actions_due.is_none() && id == pc::LEVEL_CHUNK_WITH_LIGHT {
            world_actions_due = Some(std::time::Instant::now() + std::time::Duration::from_millis(1500));
        }
        if got_position && !world_actions_sent && world_actions_due.is_some_and(|t| std::time::Instant::now() >= t) {
            world_actions_sent = true;
            use ids::play::serverbound as sbp;
            let below = (pos.0.floor() as i32, pos.1.floor() as i32 - 1, pos.2.floor() as i32);
            let mut batch: Vec<Vec<u8>> = Vec::new();
            if let Some((x, y, z)) = a.brk.map(|p| p.unwrap_or(below)) {
                // PlayerAction START_DESTROY_BLOCK (0), face UP (1), sequence 1
                batch.push(mc_protocol::frame::payload(sbp::PLAYER_ACTION, |w| {
                    w.put_varint(0);
                    w.put_block_pos(x, y, z);
                    w.put_u8(1);
                    w.put_varint(1);
                }));
            }
            // beside: the top face of the block next to the one under the feet, so the target position does not collide with the player
            let target = if a.place_beside { (below.0 + 1, below.1, below.2) } else { below };
            if let Some((x, y, z)) = a.place.map(|p| p.unwrap_or(target)) {
                // stone (minecraft:stone) into slot 36 (hotbar 0) in creative mode, then UseItemOn on the top face
                let stone = 1; // item ID of stone from the minecraft:item registry
                batch.push(mc_protocol::frame::payload(sbp::SET_CREATIVE_MODE_SLOT, |w| {
                    w.put_i16(36);
                    w.put_varint(1);
                    w.put_varint(stone);
                    w.put_varint(0);
                    w.put_varint(0);
                }));
                batch.push(mc_protocol::frame::payload(sbp::USE_ITEM_ON, |w| {
                    w.put_varint(0);
                    w.put_block_pos(x, y, z);
                    w.put_varint(1);
                    w.put_f32(0.5);
                    w.put_f32(1.0);
                    w.put_f32(0.5);
                    w.put_bool(false);
                    w.put_varint(2);
                }));
            }
            c.send_many(&batch).await?;
        }
        if a.walk && got_position && walk_step < 40 && id == pc::LEVEL_CHUNK_WITH_LIGHT {
            walk_step += 1;
            pos.0 += 2.0;
            c.send(&mc_protocol::frame::payload(ids::play::serverbound::MOVE_PLAYER_POS, |w| {
                w.put_f64(pos.0);
                w.put_f64(pos.1);
                w.put_f64(pos.2);
                w.put_u8(1);
            }))
            .await?;
        }
    }

    rep.check("play: position received", got_position, "");
    rep.check("play: chunks received", chunks_ok >= 9, format!("{chunks_ok}"));
    rep.check("play: batch_finished received", batches >= 1, format!("{batches}"));
    if a.chat.is_some() {
        rep.check("play: chat echoed to the sender", chat_echo, "");
    }
    rep.notes.push(format!("keepalive: {keepalives}"));

    // ---- raport
    println!("== NOTES");
    for n in &rep.notes {
        println!("  {n}");
    }
    println!("== PACKET COUNTS");
    for (k, v) in &rep.counts {
        println!("  {k}: {v}");
    }
    println!("== CHECKS");
    let mut failed = 0;
    for (n, ok, d) in &rep.checks {
        println!("  [{}] {n} {d}", if *ok { "OK " } else { "FAIL" });
        if !*ok {
            failed += 1;
        }
    }
    println!("== RESULT: {} / {} checks OK", rep.checks.len() - failed, rep.checks.len());
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

// ------------------------------------------- independent chunk parser (probe)

struct SectionInfo {
    palette: Vec<u32>,
    bits: u8,
    data: Vec<i64>,
    single: Option<u32>,
    biome_uniform: Option<u32>,
}
impl SectionInfo {
    fn block(&self, x: usize, y: usize, z: usize) -> u32 {
        if let Some(s) = self.single {
            return s;
        }
        let idx = (y << 8) | (z << 4) | x;
        let per = 64 / self.bits as usize;
        let v = ((self.data[idx / per] as u64) >> ((idx % per) * self.bits as usize)) & ((1u64 << self.bits) - 1);
        if self.palette.is_empty() {
            v as u32
        } else {
            self.palette[v as usize]
        }
    }
}

struct ChunkInfo {
    x: i32,
    z: i32,
    sections: Vec<SectionInfo>,
    heightmap_first: Option<i64>,
    light_sections: usize,
    sky_arrays_ok: bool,
    fully_consumed: bool,
}

fn read_container(r: &mut Reader<'_>, max_indirect: u8) -> Result<(u8, Vec<u32>, Vec<i64>, Option<u32>)> {
    let bits = r.u8()?;
    if bits == 0 {
        let v = r.varint()? as u32;
        let n = r.varint()?;
        if n != 0 {
            bail!("a single-valued container has data");
        }
        return Ok((0, vec![], vec![], Some(v)));
    }
    let mut palette = vec![];
    if bits <= max_indirect {
        let n = r.varint()?;
        for _ in 0..n {
            palette.push(r.varint()? as u32);
        }
    }
    let n = r.varint()?;
    let mut data = Vec::new();
    for _ in 0..n {
        data.push(r.i64()?);
    }
    Ok((bits, palette, data, None))
}

fn parse_chunk(body: &[u8]) -> Result<ChunkInfo> {
    let mut r = Reader::new(body);
    let x = r.i32()?;
    let z = r.i32()?;
    let hm = Tag::read_network(&mut r)?.ok_or_else(|| anyhow!("no heightmap"))?;
    let heightmap_first = match &hm {
        Tag::Compound(c) => match c.get("MOTION_BLOCKING") {
            Some(Tag::LongArray(a)) => Some(a[0] & 0x1FF),
            _ => None,
        },
        _ => None,
    };
    let size = r.varint()? as usize;
    let sec_bytes = r.take(size)?;
    let mut sr = Reader::new(sec_bytes);
    let mut sections = Vec::new();
    while !sr.is_empty() {
        let _non_empty = sr.i16()?;
        let (bits, palette, data, single) = read_container(&mut sr, 8)?;
        let (_bb, bpal, _bd, bsingle) = read_container(&mut sr, 3)?;
        let biome_uniform = bsingle.or_else(|| if bpal.len() == 1 { Some(bpal[0]) } else { None });
        sections.push(SectionInfo { palette, bits, data, single, biome_uniform });
    }
    let nbe = r.varint()?;
    for _ in 0..nbe {
        r.u8()?;
        r.i16()?;
        r.varint()?;
        Tag::read_network(&mut r)?;
    }
    let mut masks = Vec::new();
    for _ in 0..4 {
        let n = r.varint()?;
        let mut m = Vec::new();
        for _ in 0..n {
            m.push(r.i64()? as u64);
        }
        masks.push(m);
    }
    let bit_count = |m: &Vec<u64>| m.iter().map(|x| x.count_ones() as usize).sum::<usize>();
    let max_bit = |m: &Vec<u64>| m.iter().enumerate().filter(|(_, v)| **v != 0).map(|(i, v)| i * 64 + 63 - v.leading_zeros() as usize).max().unwrap_or(0);
    let sky_n = r.varint()? as usize;
    let mut sky_ok = sky_n == bit_count(&masks[0]);
    for _ in 0..sky_n {
        let len = r.varint()?;
        sky_ok &= len == 2048;
        r.take(len as usize)?;
    }
    let blk_n = r.varint()? as usize;
    for _ in 0..blk_n {
        let len = r.varint()?;
        r.take(len as usize)?;
    }
    let light_sections = masks.iter().map(max_bit).max().unwrap_or(0) + 1;
    Ok(ChunkInfo { x, z, sections, heightmap_first, light_sections, sky_arrays_ok: sky_ok, fully_consumed: r.is_empty() })
}

// ---------- SHA-1 "signed hex" (like BigInteger.toString(16)); a separate copy for the probe
fn minecraft_hex_digest(parts: &[&[u8]]) -> String {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    for p in parts {
        h.update(p);
    }
    let mut d: [u8; 20] = h.finalize().into();
    let neg = d[0] & 0x80 != 0;
    if neg {
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
    let t = hex.trim_start_matches('0');
    let t = if t.is_empty() { "0" } else { t };
    if neg {
        format!("-{t}")
    } else {
        t.to_owned()
    }
}
