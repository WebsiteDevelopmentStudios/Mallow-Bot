use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuildConfig {
    pub prefix: String,
    pub greeting: String,
    pub greetings: bool,
    pub mention_only: bool,
    pub disabled_commands: Vec<String>,
    pub features: Value,
    pub economy_config: Value,
    pub xp_config: Value,
    pub messages_config: Value,
    pub tickets_config: Value,
    pub swear_words: Vec<String>,
    pub swear_count_adjustments: Value,
}

impl Default for GuildConfig {
    fn default() -> Self {
        Self {
            prefix: "-mb".into(),
            greeting: "hey! i'm Mallow Bot ☁️".into(),
            greetings: true,
            mention_only: true,
            disabled_commands: vec![],
            features: json!({
                "economy": true, "xp": true, "moderation": true, "music": true,
                "fun": true, "utility": true, "social": true,
                "swear_counter": true
            }),
            economy_config: json!({"starting_money":100,"daily_reward":100,"work_min":20,"work_max":80}),
            xp_config: json!({"min_xp":10,"max_xp":20,"cooldown_seconds":60,"announce_level_up":true}),
            messages_config: json!({}),
            tickets_config: json!({}),
            swear_words: vec![],
            swear_count_adjustments: json!({}),
        }
    }
}
