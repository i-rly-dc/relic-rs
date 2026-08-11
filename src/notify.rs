use crate::config::NotifyConfig;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

static LAST_ID: AtomicU32 = AtomicU32::new(0);

pub fn send(cfg: &NotifyConfig, summary: &str, body: &str) -> Result<()> {
    let conn = Connection::session().context("connecting to session bus")?;
    let proxy = Proxy::new(
        &conn,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
    .context("creating notifications proxy")?;
    let urgency: u8 = match cfg.urgency.as_str() {
        "low" => 0,
        "normal" => 1,
        _ => 2,
    };
    let mut hints: HashMap<&str, Value> = HashMap::new();
    hints.insert("urgency", Value::U8(urgency));
    let id: u32 = proxy
        .call(
            "Notify",
            &(
                "relic-check",
                LAST_ID.load(Ordering::Relaxed),
                "",
                summary,
                body,
                Vec::<&str>::new(),
                hints,
                cfg.timeout_ms,
            ),
        )
        .context("sending notification")?;
    LAST_ID.store(id, Ordering::Relaxed);
    Ok(())
}
