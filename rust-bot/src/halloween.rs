use chrono::{Datelike, Timelike, Utc};
use chrono_tz::America::Los_Angeles;
use serde::{Deserialize, Serialize};
use serenity::{
    all::{ChannelId, MessageId},
    builder::{CreateEmbed, EditMessage},
    http::Http,
};
use std::{
    collections::HashMap,
    env, fs,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tracing::{info, warn};

const FACTS: &str = include_str!("../../halloween_facts.txt");

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct CountdownTarget {
    channel_id: u64,
    message_id: u64,
}

fn data_path(name: &str) -> PathBuf {
    PathBuf::from(env::var("MALLOW_DATA_DIR").unwrap_or_else(|_| ".".into())).join(name)
}

fn read_targets() -> HashMap<String, CountdownTarget> {
    fs::read_to_string(data_path("halloween_countdowns.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_targets(targets: &HashMap<String, CountdownTarget>) -> std::io::Result<()> {
    let path = data_path("halloween_countdowns.json");
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let temp = path.with_extension("tmp");
    fs::write(&temp, serde_json::to_vec_pretty(targets).unwrap_or_default())?;
    fs::rename(temp, path)
}

pub fn set_target(guild_id: u64, channel_id: u64, message_id: u64) -> std::io::Result<()> {
    let mut targets = read_targets();
    targets.insert(guild_id.to_string(), CountdownTarget { channel_id, message_id });
    write_targets(&targets)
}

fn remove_target(guild_id: &str) {
    let mut targets = read_targets();
    if targets.remove(guild_id).is_some() {
        if let Err(error) = write_targets(&targets) { warn!(%error, "Could not remove deleted Halloween countdown"); }
    }
}

fn daily_fact() -> String {
    let today = Utc::now().with_timezone(&Los_Angeles).date_naive().to_string();
    let path = data_path("halloween_fact_state.json");
    let mut state: serde_json::Value = fs::read_to_string(&path).ok()
        .and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_else(|| serde_json::json!({}));
    if state.get("date").and_then(serde_json::Value::as_str) == Some(today.as_str()) {
        if let Some(fact) = state.get("fact").and_then(serde_json::Value::as_str) { return fact.to_string(); }
    }
    let facts: Vec<&str> = FACTS.lines().map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#')).collect();
    let used: Vec<String> = state.get("used").and_then(serde_json::Value::as_array)
        .map(|values| values.iter().filter_map(serde_json::Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let fact = facts.iter().find(|fact| !used.iter().any(|item| item == **fact)).copied()
        .unwrap_or("All listed Halloween facts have been used. Add more facts to halloween_facts.txt.");
    let mut used = used;
    if facts.contains(&fact) { used.push(fact.to_string()); }
    state = serde_json::json!({"date":today,"fact":fact,"used":used});
    if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
    let temp = path.with_extension("tmp");
    if fs::write(&temp, serde_json::to_vec_pretty(&state).unwrap_or_default()).is_ok() {
        let _ = fs::rename(temp, path);
    }
    fact.to_string()
}

pub fn build_embed() -> CreateEmbed {
    let now = Utc::now().with_timezone(&Los_Angeles);
    let year = now.year();
    let is_halloween = now.month() == 10 && now.day() == 31;
    let target = if is_halloween {
        Los_Angeles.with_ymd_and_hms(year, 10, 31, 23, 59, 59).single().unwrap()
    } else {
        let next_year = if now.month() > 10 || (now.month() == 10 && now.day() > 31) { year + 1 } else { year };
        Los_Angeles.with_ymd_and_hms(next_year, 10, 31, 0, 0, 0).single().unwrap()
    };
    let remaining = (target - now).num_seconds().max(0) as u64;
    let days = if is_halloween { 0 } else { (target.date_naive() - now.date_naive()).num_days().max(0) };
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    let seconds = remaining % 60;
    CreateEmbed::new()
        .title(if is_halloween { "Happy Halloween!" } else { &format!("{days} days until Halloween") })
        .field(if is_halloween { "Halloween ends in" } else { "Countdown" }, format!("{hours:02}:{minutes:02}:{seconds:02}"), false)
        .field("Halloween Fact of the Day", daily_fact(), false)
        .colour(serenity::all::Colour::ORANGE)
        .footer(serenity::builder::CreateEmbedFooter::new("Updates automatically • Halloween countdown"))
}

pub fn spawn(http: Arc<Http>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(15));
        loop {
            ticker.tick().await;
            let targets = read_targets();
            for (guild, target) in targets {
                let result = ChannelId::new(target.channel_id).edit_message(
                    http.as_ref(),
                    MessageId::new(target.message_id),
                    EditMessage::new().embed(build_embed()),
                ).await;
                if let Err(error) = result {
                    let message = error.to_string();
                    if message.contains("404") || message.to_lowercase().contains("unknown message") {
                        remove_target(&guild);
                        info!(guild_id = %guild, "Removed deleted Halloween countdown");
                    } else {
                        warn!(%error, guild_id = %guild, "Could not refresh Halloween countdown");
                    }
                }
            }
        }
    });
}
