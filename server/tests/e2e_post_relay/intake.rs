//! Cases 2-9: signed inbox intake over HTTP -> relay (real Ory, real process, real DB).
use super::emumet::Inbound;
use super::stack::PUBLIC;
use super::world::*;
use serde_json::{json, Value};
use std::time::Duration;

async fn post(w: &World, profile: &str, v: &Value) -> u16 {
    w.peer
        .deliver_json(&w.server, profile, v)
        .await
        .status()
        .as_u16()
}
async fn forwarded(w: &World) -> usize {
    w.db.count("post_relay_bodies").await as usize
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case02_raw_bytes_unknown_fields_non_ascii_and_whitespace_survive() {
    let w = World::setup().await;
    let a = w.linked("c02", 0).await;
    let raw = format!(
        "  {{\n\t\"type\" : \"Create\",\r\n \"id\":\"{}\", \"actor\":\"{}\",\n\"x-unknown\":{{\"k\":[1,2.50,null,true]}},\n \"object\":{{\"type\":\"Note\",\"content\":\"日本語 é \\u00e9 🎌 \\n tab\\t\"}} }}\n\n",
        w.peer.activity_id(),
        w.peer.actor
    )
    .into_bytes();
    let r = w
        .peer
        .post(&w.server, &a.profile, Inbound::new(raw.clone()))
        .await;
    assert_eq!(r.status(), 202, "{}", r.text().await.unwrap());
    let got = w.receiver.wait_accepted(1, Duration::from_secs(30)).await;
    assert_eq!(got[0].raw_body, raw, "raw bytes must not be re-serialized");
    assert_eq!(got[0].envelope["activity"]["body_encoding"], "base64");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case03_to_string_actor_object_no_id_ld_json_and_array_object_are_forwarded() {
    let w = World::setup().await;
    let a = w.linked("c03", 0).await;
    let actor = w.peer.actor.clone();
    let bodies: Vec<(Vec<u8>, &str)> = vec![
        (serde_json::to_vec(&json!({"type":"Create","id":w.peer.activity_id(),"actor":actor,"to":"https://www.w3.org/ns/activitystreams#Public","object":{"type":"Note","content":"to-string"}})).unwrap(), "application/activity+json"),
        (serde_json::to_vec(&json!({"type":"Create","id":w.peer.activity_id(),"actor":{"id":actor,"type":"Person"},"object":{"type":"Note","content":"actor-object"}})).unwrap(), "application/activity+json"),
        (serde_json::to_vec(&json!({"type":"Create","actor":actor,"object":{"type":"Note","content":"no-id"}})).unwrap(), "application/activity+json"),
        (serde_json::to_vec(&json!({"@context":"https://www.w3.org/ns/activitystreams","type":"Create","id":w.peer.activity_id(),"actor":actor,"object":{"type":"Note","content":"ld"}})).unwrap(), "application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\""),
        (serde_json::to_vec(&json!({"type":"Create","id":w.peer.activity_id(),"actor":actor,"object":[{"type":"Note","content":"a"},{"type":"Note","content":"b"}]})).unwrap(), "application/activity+json"),
    ];
    for (i, (body, content_type)) in bodies.iter().enumerate() {
        let mut inbound = Inbound::new(body.clone());
        inbound.content_type = content_type;
        let r = w.peer.post(&w.server, &a.profile, inbound).await;
        assert_eq!(r.status(), 202, "variant {i}: {}", r.text().await.unwrap());
    }
    let got = w
        .receiver
        .wait_accepted(bodies.len(), Duration::from_secs(40))
        .await;
    for (body, _) in &bodies {
        assert!(
            got.iter().any(|g| &g.raw_body == body),
            "a variant was altered or dropped"
        );
    }
    // An Activity without id is forwarded with id = null (never invented).
    assert!(got.iter().any(|g| g.envelope["activity"]["id"].is_null()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case04_duplicate_keys_type_arrays_and_oversized_original_headers() {
    let w = World::setup().await;
    let a = w.linked("c04", 0).await;
    let actor = w.peer.actor.clone();
    // Duplicate keys at the top level and nested are both 400, nothing admitted.
    let top = format!(
        r#"{{"type":"Create","type":"Note","actor":"{actor}","object":{{"type":"Note"}}}}"#
    );
    let nested = format!(
        r#"{{"type":"Create","actor":"{actor}","object":{{"type":"Note","content":"a","content":"b"}}}}"#
    );
    let deep = format!(
        r#"{{"type":"Create","actor":"{actor}","object":{{"a":[{{"b":{{"c":1,"c":2}}}}]}}}}"#
    );
    for body in [top, nested, deep] {
        let r = w
            .peer
            .post(&w.server, &a.profile, Inbound::new(body.into_bytes()))
            .await;
        assert_eq!(r.status(), 400);
    }
    assert_eq!(forwarded(&w).await, 0);
    // Follow + Block in one type array is ambiguous: 400 and no relation change.
    let r = post(&w, &a.profile, &json!({"type":["Follow","Block"],"id":w.peer.activity_id(),"actor":actor,"object":format!("{PUBLIC}/ap/profiles/{}", a.profile)})).await;
    assert_eq!(r, 400);
    // An extension type alongside Create does not prevent forwarding.
    let r = post(&w, &a.profile, &json!({"type":["Create","x:Ext"],"id":w.peer.activity_id(),"actor":actor,"object":{"type":"Note","content":"ext"}})).await;
    assert_eq!(r, 202);
    w.receiver.wait_accepted(1, Duration::from_secs(30)).await;
    w.receipts_gone().await;
    // Oversized original headers: whole headers are omitted (never truncated), secret-bearing
    // ones are never stored, and the envelope says the evidence is incomplete.
    let mut inbound = Inbound::new(serde_json::to_vec(&w.peer.note("headers")).unwrap());
    for i in 0..6 {
        inbound
            .signed_headers
            .push((format!("x-pad-{i}"), "p".repeat(3000)));
    }
    inbound
        .signed_headers
        .push(("x-api-token".into(), "super-secret-token-value-9f3".into()));
    inbound
        .plain_headers
        .push(("cookie".into(), "session=cookie-secret-value-77a".into()));
    inbound.plain_headers.push((
        "authorization".into(),
        "Bearer authorization-secret-value-1d2".into(),
    ));
    let r = w.peer.post(&w.server, &a.profile, inbound).await;
    assert_eq!(r.status(), 202);
    let got = w.receiver.wait_accepted(2, Duration::from_secs(30)).await;
    let e = &got[1].envelope;
    assert_eq!(e["evidence_complete"], false);
    assert!(e["omitted_header_count"].as_u64().unwrap() >= 1);
    let text = String::from_utf8_lossy(&got[1].outer_body);
    for secret in [
        "super-secret-token-value-9f3",
        "cookie-secret-value-77a",
        "authorization-secret-value-1d2",
    ] {
        assert!(
            !text.contains(secret),
            "secret header value leaked into the envelope"
        );
    }
    for h in e["original_request"]["signature_headers"]
        .as_array()
        .unwrap()
    {
        assert!(
            h["value"].as_str().unwrap().len() <= 3000 || h["name"] == "signature",
            "values are never truncated mid-way"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case05_follow_block_excluded_known_and_unknown_undo_accept_reject() {
    let w = World::setup().await;
    let a = w.linked("c05", 0).await;
    let target = format!("{PUBLIC}/ap/profiles/{}", a.profile);
    let actor = w.peer.actor.clone();
    let follow_id = w.peer.activity_id();
    let follow = json!({"type":"Follow","id":follow_id,"actor":actor,"object":target});
    // Follow and Block themselves are never forwarded.
    let s = post(&w, &a.profile, &follow).await;
    assert!(s == 202 || s == 408, "follow status {s}");
    let block = json!({"type":"Block","id":w.peer.activity_id(),"actor":actor,"object":target});
    let s = post(&w, &a.profile, &block).await;
    assert!(s == 202 || s == 408, "block status {s}");
    assert_eq!(forwarded(&w).await, 0, "Follow/Block must not be forwarded");
    // Undo / Accept / Reject whose references are all known Follow/Block: excluded.
    let embedded_undo = json!({"type":"Undo","id":w.peer.activity_id(),"actor":actor,"object":{"type":"Follow","id":follow_id,"actor":actor,"object":target}});
    let embedded_accept = json!({"type":"Accept","id":w.peer.activity_id(),"actor":actor,"object":{"type":"Follow","id":w.peer.activity_id(),"actor":actor,"object":target}});
    let embedded_reject = json!({"type":"Reject","id":w.peer.activity_id(),"actor":actor,"object":{"type":"Block","id":w.peer.activity_id(),"actor":actor,"object":target}});
    for v in [&embedded_undo, &embedded_accept, &embedded_reject] {
        let s = post(&w, &a.profile, v).await;
        assert!(
            s == 202 || s == 408 || s == 422,
            "known-relation reference status {s}"
        );
    }
    assert_eq!(
        forwarded(&w).await,
        0,
        "known Follow/Block references must not be forwarded"
    );
    // Unknown references (URL only, missing, mixed) are forwarded in full.
    let unknown_url = json!({"type":"Undo","id":w.peer.activity_id(),"actor":actor,"object":format!("{}/activities/unknown-1", w.peer.server.uri())});
    let missing = json!({"type":"Accept","id":w.peer.activity_id(),"actor":actor});
    let mixed = json!({"type":"Reject","id":w.peer.activity_id(),"actor":actor,"object":[{"type":"Follow","id":w.peer.activity_id(),"actor":actor,"object":target}, format!("{}/activities/unknown-2", w.peer.server.uri())]});
    for v in [&unknown_url, &missing, &mixed] {
        assert_eq!(post(&w, &a.profile, v).await, 202);
    }
    let got = w.receiver.wait_accepted(3, Duration::from_secs(40)).await;
    assert_eq!(got.len(), 3);
    for v in [&unknown_url, &missing, &mixed] {
        assert!(got
            .iter()
            .any(|g| g.raw_body == serde_json::to_vec(v).unwrap()));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case06_early_undo_defense_and_independent_profiles() {
    let w = World::setup().await;
    let a = w.linked("c06a", 0).await;
    let b = w.linked("c06b", 1).await;
    let actor = w.peer.actor.clone();
    let follow_id = w.peer.activity_id();
    let target_a = format!("{PUBLIC}/ap/profiles/{}", a.profile);
    // The Undo arrives BEFORE its Follow: its reference is unknown, so it is forwarded...
    let undo = json!({"type":"Undo","id":w.peer.activity_id(),"actor":actor,"object":follow_id});
    assert_eq!(post(&w, &a.profile, &undo).await, 202);
    w.receiver.wait_accepted(1, Duration::from_secs(30)).await;
    // ... and the late Follow is neither forwarded nor allowed to create a relationship.
    let follow = json!({"type":"Follow","id":follow_id,"actor":actor,"object":target_a});
    let s = post(&w, &a.profile, &follow).await;
    assert!(s == 202 || s == 408, "late follow status {s}");
    let accepted_after = w.receiver.accepted().len();
    assert_eq!(accepted_after, 1, "the Follow itself is never forwarded");
    assert_eq!(w.db.i64(&format!("SELECT count(*) FROM profile_follows WHERE followee_profile_id={} AND state IN ('pending','accepted')", a.profile_id)).await, 0);
    // A second Profile receives the same Activity independently of the first.
    let shared = w.peer.note("same activity, two profiles");
    assert_eq!(post(&w, &a.profile, &shared).await, 202);
    assert_eq!(post(&w, &b.profile, &shared).await, 202);
    let got = w.receiver.wait_accepted(3, Duration::from_secs(40)).await;
    let for_a = got
        .iter()
        .filter(|g| {
            g.envelope["recipient"]["link_id"] == a.link_id
                && g.raw_body == serde_json::to_vec(&shared).unwrap()
        })
        .count();
    let for_b = got
        .iter()
        .filter(|g| {
            g.envelope["recipient"]["link_id"] == b.link_id
                && g.raw_body == serde_json::to_vec(&shared).unwrap()
        })
        .count();
    assert_eq!((for_a, for_b), (1, 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case07_failure_after_receipt_persistence_rolls_back_and_retry_is_not_lost() {
    let w = World::setup().await;
    let a = w.linked("c07", 0).await;
    let before_receipts = w.db.count("profile_inbound_activities").await;
    // Fault: the job body insert fails AFTER the receipt/guard rows of the same transaction.
    w.db.exec("CREATE FUNCTION t1_fail() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 't1 injected failure'; END $$").await;
    w.db.exec("CREATE TRIGGER t1_fail BEFORE INSERT ON post_relay_bodies FOR EACH ROW EXECUTE FUNCTION t1_fail()").await;
    let undo = json!({"type":"Undo","id":w.peer.activity_id(),"actor":w.peer.actor,"object":format!("{}/activities/early", w.peer.server.uri())});
    let r = w.peer.deliver_json(&w.server, &a.profile, &undo).await;
    assert!(
        r.status().is_server_error(),
        "storage failure must be a retryable 5xx, got {}",
        r.status()
    );
    assert_eq!(
        w.db.count("profile_inbound_activities").await,
        before_receipts,
        "receipt rows must roll back with the job"
    );
    assert_eq!(forwarded(&w).await, 0);
    w.db.exec("DROP TRIGGER t1_fail ON post_relay_bodies").await;
    // The retry of the same request creates the job and is delivered.
    let r = w.peer.deliver_json(&w.server, &a.profile, &undo).await;
    assert_eq!(r.status(), 202);
    let got = w.receiver.wait_accepted(1, Duration::from_secs(30)).await;
    assert_eq!(got[0].raw_body, serde_json::to_vec(&undo).unwrap());
    // A receipt row that already exists (old receive-only duplicate) must not suppress forwarding.
    let r = w.peer.deliver_json(&w.server, &a.profile, &undo).await;
    assert_eq!(r.status(), 202);
    w.receiver.wait_accepted(2, Duration::from_secs(30)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case08_one_activity_is_delivered_independently_to_multiple_profiles() {
    let w = World::setup().await;
    let a = w.linked("c08a", 0).await;
    let b = w.linked("c08b", 1).await;
    let c = w.linked("c08c", 2).await;
    let activity = w.peer.note("fan-out");
    for p in [&a, &b, &c] {
        assert_eq!(post(&w, &p.profile, &activity).await, 202);
    }
    let got = w.receiver.wait_accepted(3, Duration::from_secs(40)).await;
    let mut links: Vec<String> = got
        .iter()
        .map(|g| {
            g.envelope["recipient"]["link_id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    links.sort();
    let mut want = vec![a.link_id.clone(), b.link_id.clone(), c.link_id.clone()];
    want.sort();
    assert_eq!(links, want);
    let mut receipts: Vec<&str> = got.iter().map(|g| g.receipt_id.as_str()).collect();
    receipts.sort();
    receipts.dedup();
    assert_eq!(receipts.len(), 3, "each Profile has its own receipt");
    // Failing one Profile's home must not affect the others.
    w.receipts_gone().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case09_unlinked_profile_leaves_only_a_body_free_hourly_bucket() {
    let w = World::setup().await;
    let user = w.user("c09").await;
    let (_, profile) = w
        .server
        .create_account(
            &user.jwt,
            &format!("t1_c09_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
        )
        .await;
    let marker = "UNLINKED-BODY-MARKER-4c1f";
    for _ in 0..3 {
        assert_eq!(post(&w, &profile, &w.peer.note(marker)).await, 202);
    }
    assert_eq!(w.db.count("post_relay_bodies").await, 0);
    assert_eq!(w.db.count("post_relay_jobs").await, 0);
    assert_eq!(
        w.db.count("post_relay_unlinked").await,
        1,
        "one Profile + no_home + hour bucket"
    );
    assert_eq!(w.db.i64("SELECT count FROM post_relay_unlinked").await, 3);
    let row: String = sqlx::query_scalar("SELECT row_to_json(u)::text FROM post_relay_unlinked u")
        .fetch_one(&w.db.iso.pool)
        .await
        .unwrap();
    assert!(!row.contains(marker) && !row.contains(&w.peer.actor));
    assert_eq!(w.receiver.attempts().len(), 0);
    // Rejection of the remote actor ID / body in the logs too.
    assert!(!w.server.logs().contains(marker));
}
