mod harness;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

const WORKER: &[u8] = br#"
import {createInterface} from 'node:readline';
const send = m => process.stdout.write(JSON.stringify(m)+'\n');
const runs = new Map();
function next(id) {
  const r=runs.get(id), op=r.ops.shift();
  if(op) send({type:'agent_state',invoke_id:id,id:crypto.randomUUID(),namespace:'files',...op});
  else {runs.delete(id);send({type:'agent_done',invoke_id:id,response:JSON.stringify(r.results)});}
}
createInterface({input:process.stdin}).on('line', line=>{
  const m=JSON.parse(line);
  if(m.type==='discover') send({type:'discover',protocol:1});
  if(m.type==='agent_invoke') {runs.set(m.invoke_id,{ops:JSON.parse(m.message),results:[]});next(m.invoke_id);}
  if(m.type==='agent_state_result') {const id=[...runs.keys()][0];runs.get(id).results.push({result:m.result,error:m.error});next(id);}
});
"#;
async fn setup() -> harness::TestRuntime {
    let rt = harness::TestRuntime::boot().await;
    let config =
        br#"{"name":"State test","memory":{"enabled":true},"supervision":{"mode":"autonomous"}}"#;
    let (status, body) = rt
        .deploy(
            "assistant",
            &harness::make_tar_gz(&[("index.ts", WORKER), ("agent.json", config)]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    rt
}
async fn invoke(
    rt: &harness::TestRuntime,
    token: &str,
    session: Uuid,
    ops: Value,
) -> (StatusCode, String) {
    let response = rt
        .client
        .post(rt.url("/api/v1/apps/assistant/agent/invoke"))
        .bearer_auth(token)
        .json(&json!({"session_id":session,"message":ops.to_string()}))
        .send()
        .await
        .unwrap();
    (response.status(), response.text().await.unwrap())
}
fn result(events: &str) -> Value {
    let done = events
        .split("\n\n")
        .find(|e| e.contains("event: done"))
        .unwrap_or_else(|| panic!("{events}"));
    let data: Value =
        serde_json::from_str(done.lines().find_map(|l| l.strip_prefix("data:")).unwrap()).unwrap();
    serde_json::from_str(data["response"].as_str().unwrap()).unwrap()
}
#[tokio::test]
async fn session_files_and_personal_memory_keep_their_boundaries() {
    let rt = setup().await;
    let session = Uuid::new_v4();
    let (status, events) = invoke(
        &rt,
        &rt.token,
        session,
        json!([
            {"op":"put","key":"price","value":13.5}, {"op":"get","key":"price"},
            {"op":"put","namespace":"other","key":"forbidden","value":true},
            {"op":"put","namespace":"memory","key":"preference","value":"French"}
        ]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events}");
    let saved = result(&events);
    assert_eq!(saved[1]["result"], json!(13.5));
    assert!(
        saved[2]["error"]
            .as_str()
            .unwrap()
            .contains("invalid harness state")
    );
    // A process restart cannot erase Core-owned state.
    rt.runtime
        .worker_manager()
        .stop_app("assistant")
        .await
        .unwrap();
    let (_, events) = invoke(&rt, &rt.token, session, json!([{"op":"get","key":"price"}])).await;
    assert_eq!(result(&events)[0]["result"], json!(13.5));
    let (_, events) = invoke(
        &rt,
        &rt.token,
        Uuid::new_v4(),
        json!([
            {"op":"get","key":"price"},
            {"op":"get","namespace":"memory","key":"preference"}
        ]),
    )
    .await;
    assert!(result(&events)[0]["result"].is_null());
    assert_eq!(result(&events)[1]["result"], json!("French"));

    let other = rt.create_user("other-state@test.local").await;
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments(user_id,role) SELECT id,'admin' FROM rootcx_system.users WHERE email='other-state@test.local' ON CONFLICT DO NOTHING")
        .execute(rt.pool()).await.unwrap();
    let (status, events) = invoke(&rt, &other, session, json!([{"op":"get","key":"price"}])).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{events}");
    let (_, events) = invoke(
        &rt,
        &other,
        Uuid::new_v4(),
        json!([
            {"op":"get","key":"price"},
            {"op":"get","namespace":"memory","key":"preference"}
        ]),
    )
    .await;
    assert!(result(&events)[0]["result"].is_null());
    assert!(result(&events)[1]["result"].is_null());
    rt.shutdown().await;
}

#[tokio::test]
async fn workspace_cannot_borrow_an_inactive_invocation() {
    let rt = harness::TestRuntime::boot().await;
    let worker=br#"
    import {createInterface} from 'node:readline';
    const send=m=>process.stdout.write(JSON.stringify(m)+'\n');let active;
    createInterface({input:process.stdin}).on('line',l=>{
      const m=JSON.parse(l);
      if(m.type==='discover')send({type:'discover',protocol:1});
      if(m.type==='agent_invoke'){active=m.invoke_id;send({type:'agent_state',invoke_id:crypto.randomUUID(),id:'probe',op:'get',namespace:'files',key:'price'});}
      if(m.type==='agent_state_result')send({type:'agent_done',invoke_id:active,response:JSON.stringify(m)});
    });"#;
    let (status, body) = rt
        .deploy("assistant", &harness::make_tar_gz(&[("index.ts", worker)]))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, events) = invoke(&rt, &rt.token, Uuid::new_v4(), json!([])).await;
    assert_eq!(
        result(&events)["error"],
        json!("agent invocation is not active")
    );
    rt.shutdown().await;
}
