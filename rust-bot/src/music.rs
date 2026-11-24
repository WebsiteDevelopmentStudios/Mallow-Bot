use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use serenity::{
    all::{Guild, VoiceServerUpdateEvent, VoiceState},
    http::Http,
    gateway::ShardMessenger,
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
    is_tts: bool,
    start_position_ms: Option<u64>,
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
    lyrics_enabled: bool,
    chill_mode: bool,
    tts_queue: VecDeque<Track>,
    resume_after_tts: Option<(Track, u64)>,
    // Prevent voice-state refreshes from restarting an already-playing track.
    last_started: Option<(String, String)>,
}

#[derive(Clone)]
pub struct MusicManager {
    client: Client,
    base_url: String,
    websocket_url: String,
    password: String,
    enabled: bool,
    bot_user_id: Arc<RwLock<Option<u64>>>,
    discord_http: Arc<RwLock<Option<Arc<Http>>>>,
    node_session: Arc<RwLock<Option<String>>>,
    voices: Arc<RwLock<HashMap<u64, VoiceInfo>>>,
    member_voice_channels: Arc<RwLock<HashMap<(u64, u64), u64>>>,
    last_tts_speaker: Arc<Mutex<HashMap<u64, u64>>>,
    shard: Arc<RwLock<Option<ShardMessenger>>>,
    players: Arc<Mutex<HashMap<u64, GuildPlayer>>>,
    lyrics_tasks: Arc<Mutex<HashMap<u64, tokio::task::JoinHandle<()>>>>,
}

impl MusicManager {
    pub fn from_env() -> Result<Self> {
        let raw_base = env::var("LAVALINK_URI").unwrap_or_default().trim().trim_end_matches('/').to_string();
        let password = env::var("LAVALINK_PASSWORD").unwrap_or_default();
        let enabled = matches!(env::var("ENABLE_LAVALINK").unwrap_or_default().to_lowercase().as_str(), "1" | "true" | "yes" | "on");
        // Hosts may provide a hostname[:port] rather than a URL; secure nodes default to HTTPS/WSS.
        let base_url = if raw_base.is_empty() || raw_base.starts_with("http://") || raw_base.starts_with("https://") {
            raw_base
        } else { format!("https://{raw_base}") };
        let websocket_url = if base_url.starts_with("https://") { base_url.replacen("https://", "wss://", 1) } else { base_url.replacen("http://", "ws://", 1) } + "/v4/websocket";
        let client = Client::builder().connect_timeout(Duration::from_secs(5)).timeout(Duration::from_secs(15)).build().context("Could not create Lavalink HTTP client")?;
        Ok(Self {
            client, base_url, websocket_url, password, enabled,
            bot_user_id: Arc::new(RwLock::new(None)),
            discord_http: Arc::new(RwLock::new(None)),
            node_session: Arc::new(RwLock::new(None)),
            voices: Arc::new(RwLock::new(HashMap::new())),
            member_voice_channels: Arc::new(RwLock::new(HashMap::new())),
            last_tts_speaker: Arc::new(Mutex::new(HashMap::new())),
            shard: Arc::new(RwLock::new(None)),
            players: Arc::new(Mutex::new(HashMap::new())),
            lyrics_tasks: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn enabled(&self) -> bool { self.enabled && !self.base_url.is_empty() && !self.password.is_empty() }

    pub async fn set_shard(&self, shard: ShardMessenger) { *self.shard.write().await = Some(shard); }

    pub async fn voice_channel_for_user(&self, guild: u64, user: u64) -> Option<u64> { self.member_voice_channels.read().await.get(&(guild, user)).copied() }

    pub async fn set_discord_http(&self, http: Arc<Http>) { *self.discord_http.write().await = Some(http); }

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
        if event.get("op").and_then(Value::as_str) == Some("ready") {
            if let Some(session) = event.get("sessionId").and_then(Value::as_str) {
                *self.node_session.write().await = Some(session.to_string());
                info!("Lavalink session initialized.");
                let guilds: Vec<u64> = self.players.lock().await.keys().copied().collect();
                for guild in guilds { self.try_start(guild).await; }
            }
            return;
        }
        if event.get("op").and_then(Value::as_str) != Some("event") { return; }
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        let guild_id = event.get("guildId").and_then(Value::as_str).and_then(|value| value.parse::<u64>().ok());
        match event_type {
            "TrackEndEvent" => {
                let finished = event.get("reason").and_then(Value::as_str)
                    .map(|reason| reason.eq_ignore_ascii_case("finished") || reason.eq_ignore_ascii_case("loadfailed"))
                    .unwrap_or(false);
                if finished {
                    if let Some(guild) = guild_id { self.advance(guild).await; }
                }
            }
            "TrackExceptionEvent" | "TrackStuckEvent" => {
                let detail = event.get("exception").and_then(|value| value.get("message")).and_then(Value::as_str)
                    .or_else(|| event.get("thresholdMs").and_then(Value::as_u64).map(|_| "Track exceeded the configured stuck threshold."));
                warn!(guild_id, event_type, detail = detail.unwrap_or("Lavalink playback error"), "Lavalink track failed; advancing the queue");
                if let Some(guild) = guild_id {
                    let encoded = event.get("track").and_then(Value::as_str);
                    let should_advance = self.players.lock().await.get(&guild)
                        .and_then(|player| player.current.as_ref())
                        .map(|track| encoded.map(|value| value == track.encoded).unwrap_or(true))
                        .unwrap_or(false);
                    if should_advance { self.advance(guild).await; }
                }
            }
            "WebSocketClosedEvent" => {
                warn!(guild_id, code = ?event.get("code"), reason = ?event.get("reason"), "Lavalink player WebSocket closed");
            }
            _ => {}
        }
    }

    pub async fn sync_guild_voice_states(&self, guild: &Guild) {
        // GUILD_CREATE includes the members' current voice states. Rebuild our
        // in-memory map after a process restart instead of waiting for new joins.
        for state in guild.voice_states.values() {
            self.voice_state_update(state).await;
        }
    }

    pub async fn voice_state_update(&self, state: &VoiceState) {
        let Some(guild_id) = state.guild_id.map(|id| id.get()) else { return };
        {
            let mut members = self.member_voice_channels.write().await;
            if let Some(channel) = state.channel_id {
                members.insert((guild_id, state.user_id.get()), channel.get());
            } else {
                members.remove(&(guild_id, state.user_id.get()));
            }
        }
        if Some(state.user_id.get()) != *self.bot_user_id.read().await { return; }
        let new_channel = state.channel_id.map(|id| id.get());
        let previous_channel = {
            let mut voices = self.voices.write().await;
            let voice = voices.entry(guild_id).or_default();
            let previous = voice.channel_id;
            voice.channel_id = new_channel;
            voice.session_id = Some(state.session_id.clone());
            previous
        };
        if previous_channel != new_channel {
            let stopped_chill = {
                let mut players = self.players.lock().await;
                if let Some(player) = players.get_mut(&guild_id) {
                    let was_chill = player.chill_mode;
                    player.chill_mode = false;
                    if was_chill {
                        player.current = None;
                        player.queue.clear();
                        player.tts_queue.clear();
                        player.last_started = None;
                    }
                    was_chill
                } else { false }
            };
            if stopped_chill {
                if let Ok(url) = self.player_url(guild_id).await {
                    let _ = self.client.patch(url).header("Authorization", &self.password)
                        .json(&json!({"track":{"encoded":Value::Null},"paused":false})).send().await;
                }
            }
        }
        self.try_start(guild_id).await;
    }

    pub async fn voice_server_update(&self, event: &VoiceServerUpdateEvent) {
        let Some(guild_id) = event.guild_id.map(|id| id.get()) else { return };
        let mut voices = self.voices.write().await;
        let voice = voices.entry(guild_id).or_default();
        voice.token = Some(event.token.clone());
        voice.endpoint = event.endpoint.clone();
        drop(voices);
        self.try_start(guild_id).await;
    }

    pub async fn request_voice_channel(&self, guild: u64, channel: Option<u64>) {
        let Some(shard) = self.shard.read().await.clone() else { return };
        let payload = json!({"op":4,"d":{"guild_id":guild.to_string(),"channel_id":channel.map(|id| id.to_string()),"self_mute":false,"self_deaf":false}});
        shard.websocket_message(Message::Text(payload.to_string()));
    }

    async fn search(&self, query: &str) -> Result<Vec<Track>> {
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
        let data = payload.get("data").ok_or_else(|| anyhow!("Lavalink returned no track data."))?;
        // Lavalink v4 returns a track object for direct URLs, an array for
        // search results, and data.tracks for playlist URLs.
        let tracks_value: Vec<&Value> = if let Some(items) = data.get("tracks").and_then(Value::as_array) {
            items.iter().collect()
        } else if let Some(items) = data.as_array() {
            items.iter().collect()
        } else if data.get("encoded").is_some() {
            vec![data]
        } else {
            Vec::new()
        };
        let tracks: Vec<Track> = tracks_value.into_iter().filter_map(|track| {
            let encoded = track.get("encoded").and_then(Value::as_str)?;
            let info = track.get("info")?;
            Some(Track {
                encoded: encoded.to_string(),
                title: info.get("title").and_then(Value::as_str).unwrap_or("Unknown track").to_string(),
                artist: info.get("author").and_then(Value::as_str).unwrap_or("Unknown Artist").to_string(),
                is_tts: false,
                start_position_ms: None,
            })
        }).collect();
        if tracks.is_empty() {
            let reason = data.get("message").and_then(Value::as_str)
                .or_else(|| data.get("exception").and_then(|v| v.get("message")).and_then(Value::as_str))
                .unwrap_or("No playable tracks were found. Make sure the YouTube link is public and supported by the Lavalink node.");
            return Err(anyhow!(reason.to_string()));
        }
        Ok(tracks)
    }

    pub async fn enqueue(&self, guild: u64, query: &str) -> Result<String> {
        let mut tracks = self.search(query).await?;
        let first_title = tracks.first().map(|track| track.title.clone()).unwrap_or_else(|| "track".into());
        let total = tracks.len();
        let start = {
            let mut players = self.players.lock().await;
            let player = players.entry(guild).or_insert_with(|| GuildPlayer { volume: 100, ..Default::default() });
            let mut iter = tracks.drain(..);
            let mut should_start = false;
            if player.current.is_none() {
                if let Some(first) = iter.next() {
                    player.current = Some(first);
                    player.last_started = None;
                    should_start = true;
                }
            }
            player.queue.extend(iter);
            should_start
        };
        if start { self.try_start(guild).await; }
        if total > 1 {
            Ok(format!("Added **{total} tracks** from the YouTube playlist. First up: **{first_title}**."))
        } else if start {
            Ok(format!("Now playing **{first_title}**."))
        } else {
            let position = self.players.lock().await.get(&guild).map(|p| p.queue.len()).unwrap_or(0);
            Ok(format!("Added **{first_title}** to the queue at position {position}."))
        }
    }

    pub async fn bot_voice_channel(&self, guild: u64) -> Option<u64> {
        self.voices.read().await.get(&guild).and_then(|voice| voice.channel_id)
    }

    pub async fn speak_tts(&self, guild: u64, speaker_id: u64, speaker: &str, text: &str) -> Result<()> {
        if self.players.lock().await.get(&guild).map(|player| player.chill_mode).unwrap_or(false) {
            return Ok(());
        }
        // Announce a speaker once; repeat their name only after someone else speaks.
        let include_name = {
            let mut last = self.last_tts_speaker.lock().await;
            let previous = last.insert(guild, speaker_id);
            previous != Some(speaker_id)
        };
        let spoken = if include_name { format!("{} says: {}", speaker.trim(), text.trim()) } else { text.trim().to_string() };
        let spoken: String = spoken.chars().take(240).collect();
        if spoken.trim().is_empty() { return Ok(()); }

        // Google Translate's TTS endpoint returns audio/mpeg directly. The
        // StreamElements endpoint used previously is rejected by Lavalink's
        // HTTP source manager ("Sorry this track cannot be allowed to load").
        let mut speech_url = url::Url::parse("https://translate.google.com/translate_tts")?;
        speech_url.query_pairs_mut()
            .append_pair("ie", "UTF-8")
            .append_pair("client", "tw-ob")
            .append_pair("tl", "en")
            .append_pair("q", &spoken);
        let response = self.client.get(format!("{}/v4/loadtracks", self.base_url))
            .header("Authorization", &self.password)
            .query(&[("identifier", speech_url.as_str())])
            .send().await.context("Could not request TTS audio from Lavalink")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Lavalink TTS track lookup returned {status}: {}", body.chars().take(300).collect::<String>()));
        }
        let payload: Value = response.json().await.context("Invalid Lavalink TTS response")?;
        let track = payload.get("data").and_then(|data| {
            data.as_array().and_then(|items| items.first())
                .or_else(|| if data.is_object() { Some(data) } else { None })
        }).ok_or_else(|| anyhow!("The TTS service did not return a playable audio track"))?;
        let encoded = track.get("encoded").and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Lavalink did not return an encoded TTS track"))?;
        let title = format!("TTS: {}", speaker.trim());
        let track = Track {
            encoded: encoded.to_string(),
            title,
            artist: "Mallow TTS".into(),
            is_tts: true,
            start_position_ms: None,
        };

        // Never replace or interrupt the song currently playing. Speech is
        // queued and will play as soon as the current track naturally ends.
        let start = {
            let mut players = self.players.lock().await;
            let player = players.entry(guild).or_insert_with(|| GuildPlayer { volume: 100, ..Default::default() });
            player.tts_queue.push_back(track);
            if player.current.is_none() {
                player.current = player.tts_queue.pop_front();
                player.last_started = None;
                player.current.is_some()
            } else {
                false
            }
        };
        if start { self.try_start(guild).await; }
        Ok(())
    }

    async fn player_url(&self, guild: u64) -> Result<String> {
        let session = self.node_session.read().await.clone().ok_or_else(|| anyhow!("Lavalink is not connected yet"))?;
        Ok(format!("{}/v4/sessions/{session}/players/{guild}", self.base_url))
    }

    async fn try_start(&self, guild: u64) {
        let voice = self.voices.read().await.get(&guild).cloned().unwrap_or_default();
        let (Some(channel_id), Some(session_id), Some(token), Some(endpoint)) =
            (voice.channel_id, voice.session_id, voice.token, voice.endpoint) else { return };
        let Ok(url) = self.player_url(guild).await else { return };
        let session_signature = self.node_session.read().await.clone().unwrap_or_default();
        let (track, volume) = {
            let mut players = self.players.lock().await;
            let Some(player) = players.get_mut(&guild) else { return };
            let Some(track) = player.current.clone() else { return };
            let signature = (track.encoded.clone(), session_signature);
            if player.last_started.as_ref() == Some(&signature) {
                // Voice gateway events can repeat while the bot stays connected.
                // Re-sending an encoded track here restarts playback and can trigger Lavalink 429s.
                return;
            }
            player.last_started = Some(signature);
            (track, player.volume)
        };
        // Keep credentials out of logs while exposing voice metadata for Lavalink HTTP 400 diagnosis.
        info!(
            guild,
            voice_endpoint = %endpoint,
            voice_session_id_len = session_id.len(),
            voice_token_len = token.len(),
            encoded_track_len = track.encoded.len(),
            "Sending Lavalink player update"
        );
        let tts_speed = if track.is_tts { 1.3 } else { 1.0 };
        let mut body = json!({
            "track": {"encoded": track.encoded},
            "volume": volume,
            "filters": {"timescale": {"speed": tts_speed}},
            "voice": {
                "sessionId": session_id,
                "token": token,
                "endpoint": endpoint,
                "channelId": channel_id.to_string()
            }
        });
        if let Some(position) = track.start_position_ms {
            body["position"] = json!(position);
        }
        match self.client.patch(url).header("Authorization", &self.password).json(&body).send().await {
            Ok(response) if response.status().is_success() => {
                info!(guild, title = %track.title, status = %response.status(), "Lavalink accepted the track start request");
                let lyrics_enabled = self.players.lock().await.get(&guild).map(|p| p.lyrics_enabled).unwrap_or(false);
                if lyrics_enabled { self.spawn_lyrics(guild, track.encoded.clone()).await; }
            }
            Ok(response) => {
                let status = response.status();
                if let Some(player) = self.players.lock().await.get_mut(&guild) { player.last_started = None; }
                let response_body = response.text().await.unwrap_or_else(|error| format!("<could not read response: {error}>"));
                warn!(guild, %status, response_body = %response_body, "Lavalink rejected the track start request");
            }
            Err(error) => {
                if let Some(player) = self.players.lock().await.get_mut(&guild) { player.last_started = None; }
                warn!(%error, guild, "Could not send Lavalink track start request");
            }
        }
    }

    async fn spawn_lyrics(&self, guild: u64, encoded: String) {
        let mut tasks = self.lyrics_tasks.lock().await;
        if let Some(old) = tasks.remove(&guild) { old.abort(); }
        let manager = self.clone();
        let task = tokio::spawn(async move { manager.lyrics_loop(guild, encoded).await; });
        tasks.insert(guild, task);
    }

    async fn lyrics_loop(&self, guild: u64, encoded: String) {
        let track = self.players.lock().await.get(&guild).and_then(|p| p.current.clone());
        let Some(track) = track else { return };
        let voice = self.voices.read().await.get(&guild).cloned().unwrap_or_default();
        let Some(channel_id) = voice.channel_id else { return };
        let Some(discord_http) = self.discord_http.read().await.clone() else { return };
        let response = self.client.get("https://lrclib.net/api/get")
            .query(&[("artist_name", track.artist.as_str()), ("track_name", track.title.as_str())])
            .send().await;
        let Ok(response) = response else { return };
        let Ok(payload) = response.json::<Value>().await else { return };
        let Some(raw) = payload.get("syncedLyrics").and_then(Value::as_str) else { return };
        let mut lines: Vec<(u64, String)> = Vec::new();
        for line in raw.lines() {
            let Some(close) = line.find(']') else { continue };
            if !line.starts_with('[') { continue; }
            let stamp = &line[1..close];
            let mut parts = stamp.split(':');
            let minutes = parts.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            let seconds = parts.next().and_then(|v| v.split('.').next()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            let text = line[close + 1..].trim();
            if !text.is_empty() { lines.push((minutes * 60 + seconds, text.to_string())); }
        }
        if lines.is_empty() { return; }
        let Ok(player_url) = self.player_url(guild).await else { return };
        let mut index = 0usize;
        loop {
            let current = self.players.lock().await.get(&guild).map(|p| p.current.as_ref().map(|t| t.encoded == encoded).unwrap_or(false) && p.lyrics_enabled).unwrap_or(false);
            if !current { return; }
            let position = match self.client.get(&player_url).header("Authorization", &self.password).send().await {
                Ok(response) => response.json::<Value>().await.ok().and_then(|v| v.get("state").and_then(|s| s.get("position")).and_then(Value::as_u64)).unwrap_or(0) / 1000,
                Err(_) => 0,
            };
            while index < lines.len() && lines[index].0 <= position {
                let _ = serenity::all::ChannelId::new(channel_id).say(&discord_http.as_ref(), format!("**{}**", lines[index].1)).await;
                index += 1;
            }
            if index >= lines.len() { return; }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn advance(&self, guild: u64) {
        let next = {
            let mut players = self.players.lock().await;
            let Some(player) = players.get_mut(&guild) else { return };
            let was_tts = player.current.as_ref().map(|track| track.is_tts).unwrap_or(false);
            player.current = if player.chill_mode {
                player.current.clone()
            } else if was_tts {
                player.tts_queue.pop_front()
                    .or_else(|| player.resume_after_tts.take().map(|(mut track, position)| {
                        track.start_position_ms = Some(position);
                        track
                    }))
                    .or_else(|| player.queue.pop_front())
            } else {
                // Speech queued during a song gets priority after that song ends,
                // before the next music track.
                player.tts_queue.pop_front().or_else(|| player.queue.pop_front())
            };
            player.last_started = None;
            player.current.is_some()
        };
        if next { self.try_start(guild).await; }
        else if let Ok(url) = self.player_url(guild).await {
            let _ = self.client.patch(url).header("Authorization", &self.password).json(&json!({"track":{"encoded":Value::Null}})).send().await;
        }
    }

    pub async fn is_chill_mode(&self, guild: u64) -> bool {
        self.players.lock().await.get(&guild).map(|player| player.chill_mode).unwrap_or(false)
    }

    pub async fn toggle_chill_mode(&self, guild: u64) -> Result<String> {
        if self.bot_voice_channel(guild).await.is_none() {
            return Err(anyhow!("Join a voice channel before enabling Chill Mode."));
        }
        if self.is_chill_mode(guild).await {
            {
                let mut players = self.players.lock().await;
                if let Some(player) = players.get_mut(&guild) {
                    player.chill_mode = false;
                    player.current = None;
                    player.queue.clear();
                    player.tts_queue.clear();
                    player.last_started = None;
                }
            }
            if let Ok(url) = self.player_url(guild).await {
                self.client.patch(url).header("Authorization", &self.password)
                    .json(&json!({"track":{"encoded":Value::Null},"paused":false})).send().await?;
            }
            return Ok("Chill Mode disabled. Music commands are available again.".into());
        }

        let mut tracks = self.search("https://www.youtube.com/watch?v=l-vSSYEuO88").await?;
        let track = tracks.drain(..).next().ok_or_else(|| anyhow!("The Chill Mode video could not be loaded."))?;
        {
            let mut players = self.players.lock().await;
            let player = players.entry(guild).or_insert_with(|| GuildPlayer { volume: 100, ..Default::default() });
            player.chill_mode = true;
            player.current = Some(track.clone());
            player.queue.clear();
            player.tts_queue.clear();
            player.last_started = None;
            player.paused = false;
        }
        self.try_start(guild).await;
        Ok(format!("Chill Mode enabled. **{}** is now playing on repeat. All other music commands are locked until Chill Mode is toggled off or the bot changes voice channels.", track.title))
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
        if let Some(player) = self.players.lock().await.get_mut(&guild) { player.queue.clear(); player.current = None; player.last_started = None; }
        if let Ok(url) = self.player_url(guild).await { let _ = self.client.patch(url).header("Authorization", &self.password).json(&json!({"track":{"encoded":Value::Null},"paused":false})).send().await; }
        Ok("Playback stopped and the queue was cleared.".into())
    }

    pub async fn volume(&self, guild: u64, volume: u16) -> Result<String> {
        let url = self.player_url(guild).await?;
        self.client.patch(url).header("Authorization", &self.password).json(&json!({"volume":volume})).send().await?;
        if let Some(player) = self.players.lock().await.get_mut(&guild) { player.volume = volume; }
        Ok(format!("Music volume set to {volume}%."))
    }

    pub async fn set_lyrics(&self, guild: u64, enabled: Option<bool>) -> String {
        let (enabled, encoded) = {
            let mut players = self.players.lock().await;
            let player = players.entry(guild).or_insert_with(|| GuildPlayer { volume: 100, ..Default::default() });
            player.lyrics_enabled = enabled.unwrap_or(!player.lyrics_enabled);
            (player.lyrics_enabled, player.current.as_ref().map(|t| t.encoded.clone()))
        };
        if enabled {
            if let Some(encoded) = encoded { self.spawn_lyrics(guild, encoded).await; }
            "Warning: This Feature Is In Beta, Don't Expect A Fully Working Version Soon".into()
        } else {
            if let Some(task) = self.lyrics_tasks.lock().await.remove(&guild) { task.abort(); }
            "Lyrics disabled.".into()
        }
    }

    pub async fn queue_text(&self, guild: u64) -> String {
        let players = self.players.lock().await;
        let Some(player) = players.get(&guild) else { return "The music queue is empty.".into() };
        let mut lines = Vec::new();
        if let Some(current) = &player.current { lines.push(format!("Now playing: **{}**", current.title)); }
        for (index, track) in player.queue.iter().take(15).enumerate() { lines.push(format!("{}. {}", index + 1, track.title)); }
        if lines.is_empty() { "The music queue is empty.".into() } else { lines.join("\n") }
    }


}
