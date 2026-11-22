//! Case 20: Ory stopped / unresponsive. Reception and relay never depend on Ory; protected APIs
//! fail closed without keys and recover by themselves when Ory is back.
use super::stack::*;
use super::world::*;
use std::time::Duration;

async fn me(w: &World, jwt: &str) -> u16 {
    w.server.api("GET", "/api/v1/me", jwt, None).await.0
}

async fn relays_and_serves_instance(w: &World, a: &Linked, label: &str, expected_total: usize) {
    let r = w.peer.deliver_json(&w.server, &a.profile, &w.peer.note(label)).await;
    assert_eq!(r.status(), 202, "{label}: {}", r.text().await.unwrap());
    w.receiver.wait_accepted(expected_total, Duration::from_secs(40)).await;
    let doc: serde_json::Value = super::emumet::client().get(w.server.url("/ap/instance")).send().await.unwrap().json().await.unwrap();
    assert_eq!(doc["id"], format!("{PUBLIC}/ap/instance"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case20_ory_stopped_or_unresponsive_relay_keeps_working_and_protected_apis_fail_closed() {
    let mut w = World::setup().await;
    let a = w.linked("c20", 0).await;
    assert_eq!(me(&w, &a.user.jwt).await, 200, "baseline with the real Ory");
    relays_and_serves_instance(&w, &a, "ory up", 1).await;

    // 1. Ory stopped, process already warm: reception, relay and the instance document work; a
    //    forged or unknown-key token is never accepted.
    ory_stop();
    relays_and_serves_instance(&w, &a, "ory stopped", 2).await;
    assert_eq!(me(&w, "forged.token.value").await, 401);
    let forged = format!("{}.{}.{}", "eyJhbGciOiJSUzI1NiIsImtpZCI6InVua25vd24ifQ", a.user.jwt.split('.').nth(1).unwrap(), "AAAA");
    assert_eq!(me(&w, &forged).await, 401, "an unknown kid must stay denied while Ory is down");

    // 2. Cold start while Ory refuses connections: bounded start, relay works, API fails closed
    //    (the real token is valid, but there is no key to verify it with).
    let took = w.restart().await;
    assert!(took < Duration::from_secs(60), "cold start with Ory stopped took {took:?}");
    relays_and_serves_instance(&w, &a, "cold start, ory stopped", 3).await;
    assert_eq!(me(&w, &a.user.jwt).await, 401, "no JWKS => protected APIs fail closed");

    // 3. Ory back: JWKS is fetched lazily, the very same token works, a fresh login works.
    ory_start();
    wait_ory().await;
    super::db::eventually("API recovers by itself", Duration::from_secs(60), || async { me(&w, &a.user.jwt).await == 200 }).await;
    let again = w.user("c20again").await;
    assert_eq!(me(&w, &again.jwt).await, 200);

    // 4. Ory accepts connections but never answers (paused containers): cold start is still bounded.
    ory_pause();
    let took = w.restart().await;
    assert!(took < Duration::from_secs(60), "cold start with a stalled Ory took {took:?}");
    relays_and_serves_instance(&w, &a, "cold start, ory stalled", 4).await;
    assert_eq!(me(&w, &a.user.jwt).await, 401, "stalled JWKS => fail closed, bounded by JWKS_TIMEOUT_MS");
    ory_unpause();
    wait_ory().await;
    super::db::eventually("API recovers after the stall", Duration::from_secs(60), || async { me(&w, &a.user.jwt).await == 200 }).await;
}
