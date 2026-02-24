use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::dom::{
    BackendNodeId, GetBoxModelParams, GetContentQuadsParams, ResolveNodeParams,
    ScrollIntoViewIfNeededParams,
};
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams,
    DispatchMouseEventType, InsertTextParams, MouseButton,
};
use chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams;
use chromiumoxide::Page;

use crate::error::BrowserError;
use crate::snapshot::refs::RefRegistry;

enum Target {
    Backend(BackendNodeId),
    Selector(String),
}

fn resolve(refs: &RefRegistry, id: u32) -> Result<Target, BrowserError> {
    let e = refs.get(id).ok_or(BrowserError::ElementNotFound(id))?;
    match e.backend_node_id {
        Some(b) => Ok(Target::Backend(BackendNodeId::new(b))),
        None => match &e.selector {
            Some(s) if !s.is_empty() => Ok(Target::Selector(s.clone())),
            _ => Err(BrowserError::ElementNotFound(id)),
        }
    }
}

pub async fn click(page: &Page, refs: &RefRegistry, id: u32) -> Result<(), BrowserError> {
    let t = resolve(refs, id)?;
    scroll_into_view(page, &t).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (x, y) = click_point(page, &t).await?;
    mouse(page, DispatchMouseEventType::MouseMoved, x, y, None).await?;
    mouse(page, DispatchMouseEventType::MousePressed, x, y, Some(1)).await?;
    mouse(page, DispatchMouseEventType::MouseReleased, x, y, Some(1)).await
}

pub async fn type_keys(page: &Page, refs: &RefRegistry, id: u32, text: &str) -> Result<(), BrowserError> {
    click(page, refs, id).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Select all + delete to clear existing value
    let (mods, mod_key) = if cfg!(target_os = "macos") { (4i64, "Meta") } else { (2, "Control") };
    key(page, true, mod_key, None, None, Some(mods)).await?;
    key(page, true, "a", None, None, Some(mods)).await?;
    key(page, false, "a", None, None, None).await?;
    key(page, false, mod_key, None, None, None).await?;
    key(page, true, "Backspace", Some("Backspace"), None, None).await?;
    key(page, false, "Backspace", Some("Backspace"), None, None).await?;
    tokio::time::sleep(Duration::from_millis(30)).await;
    // Insert all text at once (triggers input events like real typing)
    page.execute(InsertTextParams::new(text)).await
        .map_err(|e| BrowserError::Action(e.to_string()))?;
    Ok(())
}

pub async fn press_key(page: &Page, k: &str) -> Result<(), BrowserError> {
    let text = match k { "Enter" => Some("\r"), "Tab" => Some("\t"), "Space" => Some(" "), _ => None };
    key(page, true, k, Some(k), text, None).await?;
    key(page, false, k, Some(k), None, None).await
}

pub async fn select_option(page: &Page, refs: &RefRegistry, id: u32, value: &str) -> Result<(), BrowserError> {
    let t = resolve(refs, id)?;
    let obj = resolve_object(page, &t).await?;
    let js = r#"function(val){const o=this.options;for(let i=0;i<o.length;i++){if(o[i].value===val||o[i].textContent.trim()===val){this.selectedIndex=i;this.dispatchEvent(new Event('change',{bubbles:true}));return true}}return false}"#;
    page.execute(
        CallFunctionOnParams::builder()
            .object_id(obj)
            .function_declaration(js)
            .argument(chromiumoxide::cdp::js_protocol::runtime::CallArgument::builder()
                .value(serde_json::Value::String(value.into())).build())
            .build().map_err(|e| BrowserError::Action(e))?,
    ).await.map_err(|e| BrowserError::Action(e.to_string()))?;
    Ok(())
}

pub async fn scroll(page: &Page, dir: &str, amount: u32) -> Result<(), BrowserError> {
    let px = amount as i32 * 300;
    let (x, y) = match dir { "up" => (0, -px), "left" => (-px, 0), "right" => (px, 0), _ => (0, px) };
    eval(page, &format!("window.scrollBy({x},{y})")).await
}

pub async fn hover(page: &Page, refs: &RefRegistry, id: u32) -> Result<(), BrowserError> {
    let t = resolve(refs, id)?;
    scroll_into_view(page, &t).await?;
    let (x, y) = click_point(page, &t).await?;
    mouse(page, DispatchMouseEventType::MouseMoved, x, y, None).await
}

async fn key(page: &Page, down: bool, k: &str, code: Option<&str>, text: Option<&str>, mods: Option<i64>) -> Result<(), BrowserError> {
    let typ = if down { DispatchKeyEventType::KeyDown } else { DispatchKeyEventType::KeyUp };
    page.execute(DispatchKeyEventParams {
        r#type: typ.clone(),
        key: Some(k.into()),
        code: code.map(Into::into),
        text: text.map(Into::into),
        modifiers: mods,
        ..DispatchKeyEventParams::new(typ)
    }).await.map_err(|e| BrowserError::Action(e.to_string()))?;
    Ok(())
}

async fn mouse(page: &Page, typ: DispatchMouseEventType, x: f64, y: f64, clicks: Option<i64>) -> Result<(), BrowserError> {
    let mut b = DispatchMouseEventParams::builder().r#type(typ.clone()).x(x).y(y);
    if matches!(typ, DispatchMouseEventType::MousePressed | DispatchMouseEventType::MouseReleased) {
        b = b.button(MouseButton::Left);
    }
    if let Some(c) = clicks { b = b.click_count(c); }
    page.execute(b.build().map_err(|e| BrowserError::Action(e))?).await
        .map_err(|e| BrowserError::Action(e.to_string()))?;
    Ok(())
}

// Errors ignored: element may already be visible or detached; click_point will catch real issues
async fn scroll_into_view(page: &Page, t: &Target) -> Result<(), BrowserError> {
    match t {
        Target::Backend(id) => { let _ = page.execute(ScrollIntoViewIfNeededParams::builder().backend_node_id(id.clone()).build()).await; }
        Target::Selector(s) => { let _ = eval(page, &format!("document.querySelector({})?.scrollIntoView({{block:'center'}})", serde_json::to_string(s).unwrap())).await; }
    }
    Ok(())
}

fn quad_center(pts: &[f64]) -> Option<(f64, f64)> {
    (pts.len() >= 8).then(|| ((pts[0]+pts[2]+pts[4]+pts[6])/4.0, (pts[1]+pts[3]+pts[5]+pts[7])/4.0))
}

async fn click_point(page: &Page, t: &Target) -> Result<(f64, f64), BrowserError> {
    match t {
        Target::Backend(id) => {
            if let Ok(r) = page.execute(GetContentQuadsParams::builder().backend_node_id(id.clone()).build()).await {
                if let Some(c) = r.result.quads.first().and_then(|q| quad_center(q.inner())) { return Ok(c); }
            }
            if let Ok(r) = page.execute(GetBoxModelParams::builder().backend_node_id(id.clone()).build()).await {
                if let Some(c) = quad_center(r.result.model.content.inner()) { return Ok(c); }
            }
            Err(BrowserError::Action("cannot determine element position".into()))
        }
        Target::Selector(s) => {
            let js = format!(r#"(()=>{{const e=document.querySelector({});if(!e)return null;const r=e.getBoundingClientRect();return[r.left+r.width/2,r.top+r.height/2]}})()"#, serde_json::to_string(s).unwrap());
            let v: Option<Vec<f64>> = page.evaluate(js).await.map_err(|e| BrowserError::Action(e.to_string()))?
                .into_value().map_err(|e| BrowserError::Action(format!("{e:?}")))?;
            match v { Some(c) if c.len() == 2 => Ok((c[0], c[1])), _ => Err(BrowserError::Action("element not found".into())) }
        }
    }
}

async fn resolve_object(page: &Page, t: &Target) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId, BrowserError> {
    match t {
        Target::Backend(id) => {
            page.execute(ResolveNodeParams::builder().backend_node_id(id.clone()).build()).await
                .map_err(|e| BrowserError::Action(e.to_string()))?
                .result.object.object_id
                .ok_or_else(|| BrowserError::Action("cannot resolve node".into()))
        }
        Target::Selector(_) => Err(BrowserError::Action("selector object resolution unsupported".into())),
    }
}

async fn eval(page: &Page, js: &str) -> Result<(), BrowserError> {
    page.evaluate(js).await.map_err(|e| BrowserError::Action(e.to_string()))?;
    Ok(())
}
