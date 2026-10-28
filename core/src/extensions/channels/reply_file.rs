use crate::tools::{ToolContext, str_arg, check_permission};
use crate::governance::execution::ExecutionOrigin;
use crate::extensions::storage::backend::{PostgresBackend, StorageBackend};
use base64::Engine;
use serde_json::{json, Value};
use uuid::Uuid;

pub async fn reply(ctx: &ToolContext) -> Result<Value, String> {
    let authority = ctx.delegated_authority().ok_or("Channel reply requires delegated agent authority")?;
    let key = authority.owner().origin.ok_or("No current channel conversation")?;
    let (channel, chat, _): (Uuid, String, Uuid) = serde_json::from_str(&key).map_err(|_| "No current channel conversation")?;
    let origin = super::origin::ChannelSession::load(&ctx.pool, &channel.to_string(), &chat, authority.human(), authority.app()).await?;
    if origin.key() != key { return Err("Channel delegation changed".into()); }
    let perms = authority.revalidate(&ctx.pool).await?;
    let app = str_arg(&ctx.args, "app_id")?;
    check_permission(&perms, &format!("app:{app}:storage.read"))?;
    let file = str_arg(&ctx.args, "file_id")?.parse::<Uuid>().map_err(|_| "Invalid file ID")?;
    let supported: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.files WHERE id=$1 AND app_id=$2 AND content_type='application/pdf' AND size BETWEEN 1 AND 5242880)")
        .bind(file).bind(app).fetch_one(&ctx.pool).await.map_err(|e| e.to_string())?;
    if !supported { return Err("A stored PDF up to 5 MiB is required".into()); }
    let (provider, config): (String, Value) = sqlx::query_as("SELECT provider,config FROM rootcx_system.channels WHERE id=$1 AND status='active'")
        .bind(channel).fetch_one(&ctx.pool).await.map_err(|e| e.to_string())?;
    if provider != "whatsapp" || config["managed"] != true { return Err("File replies are unavailable on this channel".into()); }
    let object = PostgresBackend.get(&ctx.pool, file, app).await.map_err(|e| e.to_string())?;
    origin.validate(&ctx.pool, authority.human(), authority.app()).await?;
    super::managed::relay(&provider, "send", json!({"channelId":channel,"to":chat,"type":"document","document":{
        "filename":object.name,"content_type":object.content_type,"data":base64::engine::general_purpose::STANDARD.encode(&object.content)
    }})).await.map_err(|e| e.to_string())?;
    Ok(json!({"delivered":true,"app_id":app,"file_id":file,"destination":"current_conversation"}))
}
