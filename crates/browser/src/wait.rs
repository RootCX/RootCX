use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::page::{EventLifecycleEvent, NavigateParams};
use chromiumoxide::Page;
use futures::StreamExt;
use tracing::debug;

/// Listener is set up BEFORE navigate to avoid missing early events.
pub async fn navigate(page: &Page, url: &str) -> Result<(), String> {
    let t0 = Instant::now();

    let mut stream = page.event_listener::<EventLifecycleEvent>().await
        .map_err(|e| e.to_string())?;

    let res = page.execute(NavigateParams::new(url)).await
        .map_err(|e| e.to_string())?;
    if let Some(err) = res.result.error_text {
        return Err(err);
    }
    debug!("navigate: CDP ack {}ms", t0.elapsed().as_millis());

    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = stream.next().await {
            if ev.name == "networkIdle" { return; }
        }
    }).await;
    debug!("navigate: done {}ms", t0.elapsed().as_millis());

    Ok(())
}
