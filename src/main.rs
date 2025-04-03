use sqlx::SqlitePool;
use std::{env, error::Error, sync::Arc};
use twilight_cache_inmemory::{DefaultInMemoryCache, ResourceType};
use twilight_gateway::{
    ConfigBuilder, Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _,
};
use twilight_http::Client as HttpClient;
use twilight_model::{
    gateway::{
        payload::outgoing::update_presence::UpdatePresencePayload,
        presence::{ActivityType, MinimalActivity, Status},
    },
    id::Id,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let token = env::var("TOKEN")?;

    let pool = Arc::new(SqlitePool::connect(&env::var("DATABASE_URL")?).await?);

    let shard_config = ConfigBuilder::new(
        token.clone(),
        Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT,
    )
    .presence(UpdatePresencePayload {
        activities: vec![MinimalActivity {
            kind: ActivityType::Streaming,
            name: "0 sanity doctor".to_string(),
            url: Some("https://twitch.tv/kyostinv".to_string()),
        }
        .into()],
        afk: false,
        since: None,
        status: Status::Idle,
    })
    .build();

    let mut shard = Shard::with_config(ShardId::ONE, shard_config);

    let http = Arc::new(HttpClient::new(token));

    let user = http.current_user().await?.model().await?;
    log::info!("Logged in as {}#{}", user.name, user.discriminator());

    let cache = DefaultInMemoryCache::builder()
        .resource_types(ResourceType::MESSAGE)
        .build();

    while let Some(item) = shard.next_event(EventTypeFlags::all()).await {
        let Ok(event) = item else {
            tracing::warn!(source = ?item.unwrap_err(), "error receiving event");

            continue;
        };

        cache.update(&event);

        tokio::spawn(handle_event(event, Arc::clone(&http), Arc::clone(&pool)));
    }

    Ok(())
}

async fn handle_event(
    event: Event,
    http: Arc<HttpClient>,
    pool: Arc<SqlitePool>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match event {
        Event::MessageCreate(msg)
            if msg.channel_id == Id::new(1128297878855626873)
                && msg.author.id != Id::new(1169608948081492038)
                && msg.author.id != Id::new(1083860445275881493) =>
        {
            let mut conn = pool.acquire().await?;
            let parts: Vec<&str> = msg.content.split(" ").filter(|s| !s.is_empty()).collect();
            for (a, b) in parts.iter().zip(parts.iter().skip(1)) {
                sqlx::query!("INSERT INTO markov(word1, word2) VALUES(?, ?)", a, b)
                    .execute(&mut *conn)
                    .await?;
            }
            if msg.content.starts_with("e!talk") {
                let start = if let Some((_, args)) = msg.content.split_once(" ") {
                    args.to_string()
                } else {
                    sqlx::query!("SELECT word1 FROM markov ORDER BY RANDOM() LIMIT 1")
                        .fetch_one(&mut *conn)
                        .await?
                        .word1
                        .unwrap()
                };
                let mut content = String::new();

                let mut last = start.split(" ").last().unwrap().to_string();
                for _ in 0..=20 {
                    let new = match sqlx::query!(
                        "SELECT word2 FROM markov WHERE word1 = ? ORDER BY RANDOM() LIMIT 1",
                        last
                    )
                    .fetch_one(&mut *conn)
                    .await
                    {
                        Ok(record) => match record.word2 {
                            Some(word2) if !word2.is_empty() => Ok(word2),
                            _ => break,
                        },
                        Err(sqlx::Error::RowNotFound) => break,
                        Err(err) => Err(err),
                    }?;
                    content.push_str(&new);
                    content.push(' ');
                    last = new;
                }

                if content.is_empty() {
                    http.create_message(msg.channel_id)
                        .reply(msg.id)
                        .content("I'm too dumb for this D:")
                        .await?;
                } else {
                    http.create_message(msg.channel_id)
                        .reply(msg.id)
                        .content(&format!("{} {}", start, content))
                        .await?;
                }
            }
        }
        _ => {}
    }

    Ok(())
}
