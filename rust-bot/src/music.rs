use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use serenity::{
    all::{GuildId, VoiceServerUpdateEvent, VoiceState},
    prelude::Context as DiscordContext,
};
use std::{collections::{HashMap, VecDeque}, env, sync::Arc, time::Duration};
use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::{client::IntoClientRequest, http::header::HeaderValue, Message}};
use tracing::{info, warn};

#[derive(Clone)]
struct Track {
    encoded: String,
    title: String,
    artist: String,
    uri: String,
    duration_ms: u64,
}

#[derive(Default, Clone)]
struct VoiceInfo {
    channel_id: Option<u64>,
    session_id: Option<String>,
    token: Option<String>,
    endpoint: Option<String>,
}

#[derive(Default, Clone)]
struct GuildPlayer {
    queue: VecDeque<Track>,
    current: Option<Track>,
    volume: u16,
    paused: bool,
}

#[derive(Clone)]
pub struct MusicManager {
    client: Client,
    base_url: String,
    websocket_url: String,
    password: String,
    enabled: bool,
    bot_user_id: Arc<RwLock<Option<u64>>>,
    node_session: Arc<RwLock<Option<String>>>,
    voices: Arc<RwLock<HashMap<u64, VoiceInfo>>>,
    players: Arc<Mutex<HashMap<u64, GuildPlayer>>>,
}

impl MusicManager {
    pub fn from_env() -> Result<Self> {
        let base_url = env::var("LAVALINK_URI").unwrap_or_default().trim().trim_end_matches('/').to_string();
        let password = env::var("LAVALINK_PASSWORD").unwrap_or_default();
        let enabled = matches!(env::var("ENABLE_LAVALINK").unwrap_or_default().to_lowercase().as_str(), "1" | "true" | "yes" | "on");
        let websocket_url = if base_url.starts_with("https://") { base_url.replacen("https://", "wss://", 1) } else { base_url.replacen("http://", "ws://", 1) } + "/v4/websocket";
        let client = Client::builder().connect_timeout(Duration::from_secs(5)).timeout(Duration::from_secs(15)).build().context("Could not create Lavalink HTTP client")?;
        Ok(Self {
            client, base_url, websocket_url, password, enabled,
            bot_user_id: Arc::new(RwLock::new(None)),
            node_session: Arc::new(RwLock::new(None)),
            voices: Arc::new(RwLock::new(HashMap::new())),
            players: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn enabled(&self) -> bool { self.enabled && !self.base_url.is_empty() && !self.password.is_empty() }

    pub async fn set_bot_user_id(&self, id: u64) {
        *self.bot_user_id.write().await = Some(id);
    }

    pub async fn run_node(self: Arc<Self>) {
        if !self.enabled() {
            info!("Lavalink music playback is disabled or not configured.");
            return;
        }
        loop {
            let user_id = match *self.bot_user_id.read().await {
                Some(id) => id,
                None => { tokio::time::sleep(Duration::from_secs(2)).await; continue; }
            };
            let mut request = match self.websocket_url.clone().into_client_request() {
                Ok(request) => request,
                Err(error) => { warn!(%error, "Invalid Lavalink WebSocket URL"); tokio::time::sleep(Duration::from_secs(10)).await; continue; }
            };
            let headers = request.headers_mut();
            headers.insert("Authorization", HeaderValue::from_str(&self.password).unwrap_or_else(|_| HeaderValue::from_static("")));
            headers.insert("User-Id", HeaderValue::from_str(&user_id.to_string()).unwrap());
            headers.insert("Client-Name", HeaderValue::from_static("MallowBot-Rust/1.0"));
            match connect_async(request).await {
                Ok((mut socket, _)) => {
                    info!("Connected to the external Lavalink WebSocket.");
                    while let Some(message) = socket.next().await {
                        match message {
                            Ok(Message::Text(text)) => self.handle_node_event(&text).await,
                            Ok(Message::Close(_)) => break,
                            Ok(Message::Ping(payload)) => { let _ = socket.send(Message::Pong(payload)).await; }
                            Ok(_) => {}
                            Err(error) => { warn!(%error, "Lavalink WebSocket disconnected"); break; }
                        }
                    }
                }
                Err(error) => warn!(%error, "Could not connect to Lavalink WebSocket"),
            }
            *self.node_session.write().await = None;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn handle_node_event(&self, text: &str) {
        let Ok(event) = serde_json::from_str::<Value>(text) else { return };
        match event.get("op").and_then(Value::as_str) {
            Some("ready") => {
                if let Some(session) = event.get("sessionId").and_then(Value::as_str) {
                    *self.node_session.write().await = Some(session.to_string());
                    info!("Lavalink session initialized.");
                    let guilds: Vec<u64> = self.players.lock().await.keys().copied().collect();
                    for guild in guilds { self.try_start(guild).await; }
                }
            }
            Some("event") if event.get("type").and_then(Value::as_str) == Some("TrackEndEvent") => {
                if event.get("reason").and_then(Value::as_str).map(|v| v.eq_ignore_ascii_case("finished") || v.eq_ignore_ascii_case("loadfailed")).unwrap_or(false) {
                    if let Some(guild) = event.get("guildId").and_then(Value::as_str).and_then(|v| v.parse::<u64>().ok()) {
                        self.advance(guild).await;
                    }
                }
            }
            _ => {}
        }
    }

    pub async fn voice_state_update(&self, state: &VoiceState) {
        let Some(guild_id) = state.guild_id.map(GuildId::get) else { return };
        if Some(state.user_id.get()) != *self.bot_user_id.read().await { return; }
        let mut voices = self.voices.write().await;
        let voice = voices.entry(guild_id).or_default();
        voice.channel_id = state.channel_id.map(|id| id.get());
        voice.session_id = Some(state.session_id.clone());
        drop(voices);
        self.try_start(guild_id).await;
    }

    pub async fn voice_server_update(&self, event: &VoiceServerUpdateEvent) {
        let mut voices = self.voices.write().await;
        let voice = voices.entry(event.guild_id.get()).or_default();
        voice.token = event.token.clone();
        voice.endpoint = event.endpoint.clone();
        let guild = event.guild_id.get();
        drop(voices);
        self.try_start(guild).await;
    }

    pub fn request_voice_channel(ctx: &DiscordContext, guild: u64, channel: Option<u64>) {
        let payload = json!({"op":4,"d":{"guild_id":guild.to_string(),"channel_id":channel.map(|id| id.to_string()),"self_mute":false,"self_deaf":false}});
        ctx.shard.websocket_message(Message::Text(payload.to_string()));
    }

    async fn search(&self, query: &str) -> Result<Track> {
        if !self.enabled() { return Err(anyhow!("Lavalink is disabled. Set ENABLE_LAVALINK=true to enable music.")); }
        let mut search_term = query.trim().to_string();
        if query.contains("open.spotify.com/track/") || query.contains("spotify.link/") {
            let metadata = self.client.get("https://open.spotify.com/oembed").query(&[("url", query)]).send().await
                .context("Spotify metadata lookup failed")?;
            if !metadata.status().is_success() { return Err(anyhow!("Spotify did not return track metadata")); }
            let data: Value = metadata.json().await.context("Spotify metadata was invalid")?;
            let title = data.get("title").and_then(Value::as_str).unwrap_or("").trim();
            let artist = data.get("author_name").and_then(Value::as_str).unwrap_or("Unknown Artist").trim();
            if title.is_empty() { return Err(anyhow!("Spotify track title was missing")); }
            search_term = format!("{artist} - {title}");
        }
        let identifier = if search_term.starts_with("http://") || search_term.starts_with("https://") { search_term.clone() } else { format!("ytsearch:{search_term}") };
        let response = self.client.get(format!("{}/v4/loadtracks", self.base_url))
            .header("Authorization", &self.password)
            .query(&[("identifier", identifier)])
            .send().await.context("Lavalink search request failed")?;
        if !response.status().is_success() { return Err(anyhow!("Lavalink returned HTTP {}", response.status())); }
        let payload: Value = response.json().await.context("Invalid Lavalink search response")?;
        let track = payload.get("data").and_then(Value::as_array).and_then(|v| v.first()).ok_or_else(|| anyhow!("No playable tracks were found"))?;
        let encoded = track.get("encoded").and_then(Value::as_str).ok_or_else(|| anyhow!("Lavalink did not return an encoded track"))?;
        let info = track.get("info").cloned().unwrap_or_default();
        Ok(Track {
            encoded: encoded.to_string(),
            title: info.get("title").and_then(Value::as_str).unwrap_or("Unknown track").to_string(),
            artist: info.get("author").and_then(Value::as_str).unwrap_or("Unknown Artist").to_string(),
            uri: info.get("uri").and_then(Value::as_str).unwrap_or("").to_string(),
            duration_ms: info.get("length").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    pub async fn enqueue(&self, guild: u64, query: &str) -> Result<String> {
        let track = self.search(query).await?;
        let title = track.title.clone();
        let start = {
            let mut players = self.players.lock().await;
            let player = players.entry(guild).or_insert_with(|| GuildPlayer { volume: 100, ..Default::default() });
            if player.current.is_none() { player.current = Some(track); true } else { player.queue.push_back(track); false }
        };
        if start { self.try_start(guild).await; Ok(format!("Queued and starting **{title}**.")) }
        else { let count = self.players.lock().await.get(&guild).map(|p| p.queue.len()).unwrap_or(0); Ok(format!("Added **{title}** to the queue at position {count}.")) }
    }

    async fn player_url(&self, guild: u64) -> Result<String> {
        let session = self.node_session.read().await.clone().ok_or_else(|| anyhow!("Lavalink is not connected yet"))?;
        Ok(format!("{}/v4/sessions/{session}/players/{guild}", self.base_url))
    }

    async fn try_start(&self, guild: u64) {
        let track = self.players.lock().await.get(&guild).and_then(|p| p.current.clone());
        let Some(track) = track else { return };
        let voice = self.voices.read().await.get(&guild).cloned().unwrap_or_default();
        let (Some(session_id), Some(token), Some(endpoint)) = (voice.session_id, voice.token, voice.endpoint) else { return };
        let Ok(url) = self.player_url(guild).await else { return };
        let volume = self.players.lock().await.get(&guild).map(|p| p.volume).unwrap_or(100);
        let body = json!({"track":{"encoded":track.encoded},"volume":volume,"voice":{"sessionId":session_id,"token":token,"endpoint":endpoint}});
        if let Err(error) = self.client.patch(url).header("Authorization", &self.password).json(&body).send().await {
            warn!(%error, guild, "Could not start Lavalink track");
        } else {
            info!(guild, title = %track.title, "Lavalink track started");
        }
    }

    async fn advance(&self, guild: u64) {
        let next = {
            let mut players = self.players.lock().await;
            let Some(player) = players.get_mut(&guild) else { return };
            player.current = player.queue.pop_front();
            player.current.is_some()
        };
        if next { self.try_start(guild).await; }
        else if let Ok(url) = self.player_url(guild).await {
            let _ = self.client.patch(url).header("Authorization", &self.password).json(&json!({"track":{"encoded":Value::Null}})).send().await;
        }
    }

    pub async fn pause(&self, guild: u64, paused: bool) -> Result<String> {
        let url = self.player_url(guild).await?;
        self.client.patch(url).header("Authorization", &self.password).json(&json!({"paused":paused})).send().await?;
        if let Some(player) = self.players.lock().await.get_mut(&guild) { player.paused = paused; }
        Ok(if paused { "Music paused.".into() } else { "Music resumed.".into() })
    }

    pub async fn skip(&self, guild: u64) -> Result<String> {
        let has_next = self.players.lock().await.get(&guild).map(|p| !p.queue.is_empty()).unwrap_or(false);
        self.advance(guild).await;
        Ok(if has_next { "Skipped to the next track.".into() } else { "Skipped the current track; the queue is now empty.".into() })
    }

    pub async fn stop(&self, guild: u64) -> Result<String> {
        if let Some(player) = self.players.lock().await.get_mut(&guild) { player.queue.clear(); player.current = None; }
        if let Ok(url) = self.player_url(guild).await { let _ = self.client.patch(url).header("Authorization", &self.password).json(&json!({"track":{"encoded":Value::Null},"paused":false})).send().await; }
        Ok("Playback stopped and the queue was cleared.".into())
    }

    pub async fn volume(&self, guild: u64, volume: u16) -> Result<String> {
        let url = self.player_url(guild).await?;
        self.client.patch(url).header("Authorization", &self.password).json(&json!({"volume":volume})).send().await?;
        if let Some(player) = self.players.lock().await.get_mut(&guild) { player.volume = volume; }
        Ok(format!("Music volume set to {volume}%."))
    }

    pub async fn queue_text(&self, guild: u64) -> String {
        let players = self.players.lock().await;
        let Some(player) = players.get(&guild) else { return "The music queue is empty.".into() };
        let mut lines = Vec::new();
        if let Some(current) = &player.current { lines.push(format!("Now playing: **{}**", current.title)); }
        for (index, track) in player.queue.iter().take(15).enumerate() { lines.push(format!("{}. {}", index + 1, track.title)); }
        if lines.is_empty() { "The music queue is empty.".into() } else { lines.join("\n") }
    }

    pub async fn current_track(&self, guild: u64) -> Option<(String, String)> {
        self.players.lock().await.get(&guild).and_then(|p| p.current.as_ref()).map(|t| (t.title.clone(), t.uri.clone()))
    }
}
