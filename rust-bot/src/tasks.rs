use serenity::gateway::{ActivityData, ShardMessenger};
use serenity::model::user::OnlineStatus;
use std::time::Duration;
use tokio::time::interval;

pub fn spawn_status_rotation(shard: ShardMessenger) {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(45));
        let mut index = 0usize;
        loop {
            ticker.tick().await;
            let statuses = std::fs::read_to_string("status's.txt").unwrap_or_else(|_| "Watching the days until Halloween".into())
                .lines().map(str::trim).filter(|line| !line.is_empty()).map(|line| line.chars().take(128).collect::<String>()).take(100).collect::<Vec<_>>();
            if statuses.is_empty() { continue; }
            shard.set_presence(Some(ActivityData::custom(statuses[index % statuses.len()].clone())), OnlineStatus::Online);
            index = index.wrapping_add(1);
        }
    });
}
