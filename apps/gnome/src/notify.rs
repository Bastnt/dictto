//! Desktop notifications for problems the user must fix by hand.
use std::collections::HashMap;
use zbus::zvariant::Value;

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
}

pub async fn show(summary: &str, body: &str) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;
    NotificationsProxy::new(&connection)
        .await?
        .notify("Dictto", 0, "dictto", summary, body, &[], HashMap::new(), -1)
        .await?;
    Ok(())
}
