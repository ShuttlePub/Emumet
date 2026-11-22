//! Cases 26-27: the P1 receiver procedure (contract 3.3) with NO link state. A test-owned "Emumet"
//! (own instance actor + key behind a plain HTTP actor endpoint) signs hand-built requests so that
//! every negative reason is isolated: each request is correctly signed except for the one defect
//! under test. The positive cases use a real envelope produced by the real server.
use super::receiver::{Receiver, TARGET};
use super::world::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const FAKE: &str = "https://emumet.fake.example";

struct FakeEmumet {
    private: rsa::RsaPrivateKey,
    base: String,
}

impl FakeEmumet {
    async fn start() -> Self {
        use rsa::pkcs8::EncodePublicKey;
        let private = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
        let pem = rsa::RsaPublicKey::from(&private)
            .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let pem = pem.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = head.split(' ').nth(1).unwrap_or("/").to_string();
                    let id = format!("{FAKE}{path}");
                    let doc = json!({
                        "id": id, "type": "Application",
                        "publicKey": [{"id": format!("{id}#main-key"), "owner": id, "publicKeyPem": pem}]
                    });
                    let body = serde_json::to_vec(&doc).unwrap();
                    let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/activity+json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()).as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });
        Self { private, base }
    }

    /// A correctly formed signed request; the mutators introduce exactly one defect.
    fn request(
        &self,
        receiver: &Receiver,
        body: &[u8],
        spec: Spec,
    ) -> (HashMap<String, String>, Vec<u8>) {
        use rsa::signature::{SignatureEncoding, Signer};
        let digest = spec
            .digest
            .unwrap_or_else(|| format!("SHA-256={}", b64(&Sha256::digest(body))));
        let date =
            httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(0) - spec.date_age);
        let host = spec.host.unwrap_or_else(|| receiver.authority.clone());
        let key_id = spec
            .key_id
            .unwrap_or_else(|| format!("{FAKE}/ap/instance#main-key"));
        let names = ["(request-target)", "host", "date", "digest"];
        let lines = format!(
            "(request-target): post {TARGET}\nhost: {host}\ndate: {date}\ndigest: {digest}"
        );
        let signer = spec.signer.as_ref().unwrap_or(&self.private);
        let signature =
            rsa::pkcs1v15::SigningKey::<Sha256>::new(signer.clone()).sign(lines.as_bytes());
        let header = format!(
            "keyId=\"{key_id}\",algorithm=\"rsa-sha256\",headers=\"{}\",signature=\"{}\"",
            spec.covered.unwrap_or_else(|| names.join(" ")),
            b64(&signature.to_bytes())
        );
        let headers = HashMap::from([
            ("host".to_string(), host),
            ("date".to_string(), date),
            ("digest".to_string(), digest),
            ("signature".to_string(), header),
        ]);
        (headers, body.to_vec())
    }
}

#[derive(Default)]
struct Spec {
    digest: Option<String>,
    host: Option<String>,
    key_id: Option<String>,
    covered: Option<String>,
    signer: Option<rsa::RsaPrivateKey>,
    date_age: Duration,
}

fn b64(bytes: &[u8]) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
}

fn envelope_for(template: &Value, receiver: &Receiver) -> Value {
    let mut e = template.clone();
    e["sender"]["origin"] = json!(FAKE);
    e["sender"]["instance_actor"] = json!(format!("{FAKE}/ap/instance"));
    let actor = e["recipient"]["profile_actor"]
        .as_str()
        .unwrap()
        .to_string();
    let path = actor.splitn(4, '/').nth(3).unwrap_or_default().to_string();
    e["recipient"]["profile_actor"] = json!(format!("{FAKE}/{path}"));
    e["recipient"]["origin"] = json!(receiver.origin);
    e
}

fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}

async fn expect(
    receiver: &Receiver,
    req: (HashMap<String, String>, Vec<u8>),
    status: u16,
    code: &str,
) {
    let got = receiver
        .verify_request("POST", TARGET, &req.0, &req.1)
        .await;
    match got {
        Err(r) => assert_eq!(
            (r.status, r.code),
            (status, code),
            "expected {status} {code}"
        ),
        Ok(_) => panic!("expected {status} {code}, but the request was accepted"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case26_27_receiver_p1_procedure_rejects_every_defect_and_needs_no_link_table() {
    // A real envelope from the real server is the template for a valid body.
    let w = World::setup().await;
    let a = w.linked("c26", 0).await;
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("template"))
            .await
            .status(),
        202
    );
    let template = w.envelope_for(1).await;

    let fake = FakeEmumet::start().await;
    let receiver = Receiver::start(FAKE, &fake.base).await;
    let good = envelope_for(&template, &receiver);
    let good_bytes = bytes(&good);

    // 27 (positive, no link state): link ids/generations were never registered anywhere in the
    // receiver, a never-seen link_id and a high generation are trusted claims of the configured Emumet.
    let (h, b) = fake.request(&receiver, &good_bytes, Spec::default());
    assert!(receiver
        .verify_request("POST", TARGET, &h, &b)
        .await
        .is_ok());
    let mut other_link = good.clone();
    other_link["receipt_id"] = json!(format!("{}", uuid::Uuid::new_v4()));
    for (k, v) in [
        ("link_id", json!(uuid::Uuid::new_v4().to_string())),
        ("link_generation", json!(7)),
    ] {
        if other_link["recipient"].get(k).is_some() {
            other_link["recipient"][k] = v;
        } else if other_link.get(k).is_some() {
            other_link[k] = v;
        }
    }
    let (h, b) = fake.request(&receiver, &bytes(&other_link), Spec::default());
    receiver
        .verify_request("POST", TARGET, &h, &b)
        .await
        .expect("unknown link id / generation is accepted at P1");
    // Date tolerance +-5 minutes: inside accepted.
    let (h, b) = fake.request(
        &receiver,
        &good_bytes,
        Spec {
            date_age: Duration::from_secs(280),
            ..Default::default()
        },
    );
    assert!(receiver
        .verify_request("POST", TARGET, &h, &b)
        .await
        .is_ok());
    let fetches_after_positive = receiver.instance_fetches();
    assert_eq!(
        fetches_after_positive, 1,
        "the instance actor key is fetched once and cached"
    );

    // 26 (negative): 1. trusted Emumet.
    let fresh = Receiver::start(FAKE, &fake.base).await;
    let good_f = bytes(&envelope_for(&template, &fresh));
    expect(
        &fresh,
        fake.request(
            &fresh,
            &good_f,
            Spec {
                key_id: Some("https://evil.example/ap/instance#main-key".into()),
                ..Default::default()
            },
        ),
        403,
        "unknown_emumet",
    )
    .await;
    assert_eq!(
        fresh.instance_fetches(),
        0,
        "an untrusted keyId origin is rejected before any fetch"
    );
    // 2. instance actor and key.
    expect(
        &fresh,
        fake.request(
            &fresh,
            &good_f,
            Spec {
                key_id: Some(format!("{FAKE}/ap/profiles/x#main-key")),
                ..Default::default()
            },
        ),
        403,
        "not_instance_actor",
    )
    .await;
    expect(
        &fresh,
        fake.request(
            &fresh,
            &good_f,
            Spec {
                key_id: Some(format!("{FAKE}/ap/instance#other-key")),
                ..Default::default()
            },
        ),
        401,
        "unknown_key",
    )
    .await;
    let stranger = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
    expect(
        &fresh,
        fake.request(
            &fresh,
            &good_f,
            Spec {
                signer: Some(stranger),
                ..Default::default()
            },
        ),
        401,
        "signature_invalid",
    )
    .await;
    expect(
        &fresh,
        fake.request(
            &fresh,
            &good_f,
            Spec {
                covered: Some("(request-target) host date".into()),
                ..Default::default()
            },
        ),
        401,
        "signature_does_not_cover_required_headers",
    )
    .await;
    // 3. Host, Date, Digest.
    expect(
        &fresh,
        fake.request(
            &fresh,
            &good_f,
            Spec {
                host: Some("other.example".into()),
                ..Default::default()
            },
        ),
        401,
        "host_mismatch",
    )
    .await;
    expect(
        &fresh,
        fake.request(
            &fresh,
            &good_f,
            Spec {
                date_age: Duration::from_secs(330),
                ..Default::default()
            },
        ),
        401,
        "date_out_of_range",
    )
    .await;
    expect(
        &fresh,
        fake.request(
            &fresh,
            &good_f,
            Spec {
                digest: Some(format!("SHA-256={}", b64(&Sha256::digest(b"other")))),
                ..Default::default()
            },
        ),
        401,
        "digest_mismatch",
    )
    .await;
    // 6. version, schema, size (each correctly signed).
    let mut v2 = envelope_for(&template, &fresh);
    v2["version"] = json!(2);
    expect(
        &fresh,
        fake.request(&fresh, &bytes(&v2), Spec::default()),
        400,
        "unsupported_version",
    )
    .await;
    let mut extra = envelope_for(&template, &fresh);
    extra["unexpected_field"] = json!(true);
    expect(
        &fresh,
        fake.request(&fresh, &bytes(&extra), Spec::default()),
        400,
        "schema_violation",
    )
    .await;
    let mut missing = envelope_for(&template, &fresh);
    missing.as_object_mut().unwrap().remove("receipt_id");
    expect(
        &fresh,
        fake.request(&fresh, &bytes(&missing), Spec::default()),
        400,
        "schema_violation",
    )
    .await;
    let huge = vec![b'x'; 2 * 1024 * 1024 + 1];
    expect(
        &fresh,
        fake.request(&fresh, &huge, Spec::default()),
        413,
        "envelope_too_large",
    )
    .await;
    expect(
        &fresh,
        fake.request(&fresh, b"not json", Spec::default()),
        400,
        "not_json",
    )
    .await;
    // 4. sender / Profile key origin, 5. recipient.
    let mut sender = envelope_for(&template, &fresh);
    sender["sender"]["origin"] = json!("https://other.example");
    expect(
        &fresh,
        fake.request(&fresh, &bytes(&sender), Spec::default()),
        403,
        "sender_mismatch",
    )
    .await;
    let mut profile = envelope_for(&template, &fresh);
    profile["recipient"]["profile_actor"] = json!("https://other.example/ap/profiles/x");
    expect(
        &fresh,
        fake.request(&fresh, &bytes(&profile), Spec::default()),
        403,
        "profile_origin_mismatch",
    )
    .await;
    let mut recipient = envelope_for(&template, &fresh);
    recipient["recipient"]["origin"] = json!("http://127.0.0.1:9");
    let r = fake.request(&fresh, &bytes(&recipient), Spec::default());
    let got = fresh.verify_request("POST", TARGET, &r.0, &r.1).await;
    assert!(
        got.is_err(),
        "an envelope addressed to another receiver is refused: {got:?}"
    );
    // Wrong method / target.
    let (h, b) = fake.request(&fresh, &good_f, Spec::default());
    assert_eq!(
        fresh
            .verify_request("GET", TARGET, &h, &b)
            .await
            .unwrap_err()
            .status,
        405
    );
    assert_eq!(
        fresh
            .verify_request("POST", "/elsewhere", &h, &b)
            .await
            .unwrap_err()
            .status,
        404
    );
    // Nothing above was accepted.
    assert!(fresh.accepted().is_empty());
}
