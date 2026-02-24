use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::page::EventLifecycleEvent;
use chromiumoxide::Page;
use futures::StreamExt;

use crate::snapshot;

pub async fn until_ready(page: &Page) {
    network_idle(page).await;
    dom_stable(page).await;
    content_check(page).await;
}

async fn network_idle(page: &Page) {
    let Ok(mut stream) = page.event_listener::<EventLifecycleEvent>().await else { return };
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = stream.next().await {
            if ev.name == "networkIdle" { return; }
        }
    }).await;
}

async fn dom_stable(page: &Page) {
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        let _: Result<bool, _> = page.evaluate(
            r#"new Promise(r=>{let t;const d=()=>{o.disconnect();r(true)};const o=new MutationObserver(()=>{clearTimeout(t);t=setTimeout(d,1000)});o.observe(document.body||document.documentElement,{childList:true,subtree:true,attributes:true});t=setTimeout(d,1000)})"#
        ).await.and_then(|v| v.into_value().map_err(Into::into));
    }).await;
}

async fn content_check(page: &Page) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(snap) = snapshot::take(page).await {
            if !snap.refs.is_empty() { return; }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
