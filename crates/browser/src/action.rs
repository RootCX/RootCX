use std::time::Duration;

use chromiumoxide::Page;

use crate::error::BrowserError;
use crate::snapshot::refs::RefRegistry;

const TIMEOUT: Duration = Duration::from_secs(8);
const POLL: Duration = Duration::from_millis(200);

fn selector(refs: &RefRegistry, ref_id: u32) -> Result<String, BrowserError> {
    refs.get(ref_id)
        .map(|e| serde_json::to_string(&e.selector).unwrap())
        .ok_or(BrowserError::ElementNotFound(ref_id))
}

pub async fn click(page: &Page, refs: &RefRegistry, ref_id: u32) -> Result<(), BrowserError> {
    let sel = selector(refs, ref_id)?;
    wait_actionable(page, &sel).await?;
    eval(page, &format!(
        "((s)=>{{const e=document.querySelector(s);e.scrollIntoView({{block:'center'}});e.click()}})({sel})"
    )).await
}

pub async fn type_keys(page: &Page, refs: &RefRegistry, ref_id: u32, text: &str) -> Result<(), BrowserError> {
    let sel = selector(refs, ref_id)?;
    let txt = serde_json::to_string(text).unwrap();
    wait_actionable(page, &sel).await?;
    eval(page, &format!(
        r#"((s,t)=>{{const e=document.querySelector(s);e.focus();e.value='';for(const c of t){{e.dispatchEvent(new KeyboardEvent('keydown',{{key:c,bubbles:true}}));e.value+=c;e.dispatchEvent(new InputEvent('input',{{data:c,inputType:'insertText',bubbles:true}}));e.dispatchEvent(new KeyboardEvent('keyup',{{key:c,bubbles:true}}))}}e.dispatchEvent(new Event('change',{{bubbles:true}}))}})({sel},{txt})"#
    )).await
}

pub async fn scroll(page: &Page, direction: &str, amount: u32) -> Result<(), BrowserError> {
    let px = amount as i32 * 300;
    let (x, y) = match direction {
        "up" => (0, -px), "left" => (-px, 0), "right" => (px, 0), _ => (0, px),
    };
    eval(page, &format!("window.scrollBy({x},{y})")).await
}

async fn wait_actionable(page: &Page, sel: &str) -> Result<(), BrowserError> {
    let js = format!(
        r#"(s=>{{const e=document.querySelector(s);if(!e)return'not_found';const r=e.getBoundingClientRect();if(!r.width||!r.height)return'hidden';if(e.disabled)return'disabled';const t=document.elementFromPoint(r.left+r.width/2,r.top+r.height/2);if(t&&!e.contains(t)&&t!==e)return'obscured';return'ok'}})({sel})"#
    );
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let r: Result<String, _> = page.evaluate(js.as_str()).await
            .and_then(|v| v.into_value().map_err(Into::into));
        match r {
            Ok(s) if s == "ok" => return Ok(()),
            Ok(s) if tokio::time::Instant::now() >= deadline =>
                return Err(BrowserError::Action(format!("not actionable: {s}"))),
            _ => tokio::time::sleep(POLL).await,
        }
    }
}

async fn eval(page: &Page, js: &str) -> Result<(), BrowserError> {
    page.evaluate(js).await.map_err(|e| BrowserError::Action(e.to_string()))?;
    Ok(())
}
