//! `turmite serve --demo`: simulated chains and a ready project, for a local trial.

use crate::{app::App, crypto::random_hex, error::Result};

pub async fn setup(app: &App) -> Result<()> {
    let projects = app.store.list_projects().await?;
    let project_id = match projects.first() {
        Some(p) => p.project_id,
        None => app.store.create_project("Demo shop", &app.sealer.seal_str("ipn_secret", &random_hex(24))).await?,
    };
    for n in app.store.list_networks().await? {
        if app.store.project_wallet(project_id, n.network_id).await?.is_none() {
            app.store.upsert_project_wallet_address(project_id, n.network_id, &format!("demo-{}-wallet", n.chain), None).await?;
        }
    }
    crate::exchange::demo_setup(app, project_id).await
}
