//! ローカル SMTP 模擬 (テスト専用)。実際の TCP 接続で SMTP の会話を行い、受信したメールを記録する。
//! 各応答は script で差し替えられる (認証失敗・宛先拒否・一時エラー・無応答など)。

use base64::Engine;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct SmtpScript {
    pub greeting: String,
    /// `Some((user, password))` なら AUTH PLAIN を要求する。
    pub credentials: Option<(String, String)>,
    pub advertise_starttls: bool,
    pub mail_reply: String,
    pub rcpt_reply: String,
    pub data_reply: String,
    /// true なら接続後に何も応答しない (timeout の試験)。
    pub silent: bool,
    /// true なら QUIT に応答しない (確定済みの結果を QUIT が覆さないことの試験)。
    pub silent_quit: bool,
    /// 指定した接頭辞 (`MAIL FROM` / `RCPT TO` / `DATA`) のコマンドに応答せず固まる。
    pub silent_on: Option<String>,
    /// 指定した接頭辞のコマンドを受けたら応答せずに接続を切る。
    pub close_on: Option<String>,
    /// true なら DATA の本文を受け取った後に応答しない。
    pub silent_data_end: bool,
}

impl Default for SmtpScript {
    fn default() -> Self {
        Self {
            greeting: "220 mock ESMTP ready".to_string(),
            credentials: None,
            advertise_starttls: false,
            mail_reply: "250 sender ok".to_string(),
            rcpt_reply: "250 recipient ok".to_string(),
            data_reply: "250 queued".to_string(),
            silent: false,
            silent_quit: false,
            silent_on: None,
            close_on: None,
            silent_data_end: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReceivedMail {
    pub from: String,
    pub rcpt: Vec<String>,
    pub data: String,
}

pub struct MockSmtp {
    pub addr: SocketAddr,
    pub mails: Arc<Mutex<Vec<ReceivedMail>>>,
    pub connections: Arc<AtomicUsize>,
}

impl MockSmtp {
    pub async fn start(script: SmtpScript) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mails = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let accept_mails = mails.clone();
        let accept_connections = connections.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                accept_connections.fetch_add(1, Ordering::SeqCst);
                let script = script.clone();
                let mails = accept_mails.clone();
                tokio::spawn(async move {
                    let _ = handle(stream, script, mails).await;
                });
            }
        });
        Self {
            addr,
            mails,
            connections,
        }
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn mails(&self) -> Vec<ReceivedMail> {
        self.mails.lock().unwrap().clone()
    }

    /// 受信した最後のメールから 8 桁の code を取り出す。
    pub fn last_code(&self) -> String {
        let mails = self.mails();
        let data = &mails.last().expect("a mail was received").data;
        let marker = "verification code is: ";
        let start = data.find(marker).expect("code line") + marker.len();
        data[start..start + 8].to_string()
    }
}

async fn handle(
    stream: tokio::net::TcpStream,
    script: SmtpScript,
    mails: Arc<Mutex<Vec<ReceivedMail>>>,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    if script.silent {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        return Ok(());
    }
    writer
        .write_all(format!("{}\r\n", script.greeting).as_bytes())
        .await?;
    if !script.greeting.starts_with("220") {
        return Ok(());
    }
    let mut current = ReceivedMail::default();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let command = line.trim_end().to_string();
        let upper = command.to_ascii_uppercase();
        if let Some(prefix) = &script.close_on {
            if upper.starts_with(prefix.as_str()) {
                return Ok(());
            }
        }
        if let Some(prefix) = &script.silent_on {
            if upper.starts_with(prefix.as_str()) {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                return Ok(());
            }
        }
        let reply = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            let mut reply = String::from("250-mock\r\n");
            if script.credentials.is_some() {
                reply.push_str("250-AUTH PLAIN\r\n");
            }
            if script.advertise_starttls {
                reply.push_str("250-STARTTLS\r\n");
            }
            reply.push_str("250 8BITMIME\r\n");
            reply
        } else if upper.starts_with("AUTH PLAIN") {
            let encoded = command[10..].trim();
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap_or_default();
            let parts: Vec<&[u8]> = decoded.split(|byte| *byte == 0).collect();
            let ok = match (&script.credentials, parts.as_slice()) {
                (Some((user, password)), [_, given_user, given_password]) => {
                    given_user == &user.as_bytes() && given_password == &password.as_bytes()
                }
                _ => false,
            };
            if ok {
                "235 2.7.0 authenticated\r\n".to_string()
            } else {
                "535 5.7.8 authentication failed\r\n".to_string()
            }
        } else if upper.starts_with("MAIL FROM") {
            current = ReceivedMail {
                from: command[10..].trim().to_string(),
                ..ReceivedMail::default()
            };
            format!("{}\r\n", script.mail_reply)
        } else if upper.starts_with("RCPT TO") {
            if script.rcpt_reply.starts_with('2') {
                current.rcpt.push(command[8..].trim().to_string());
            }
            format!("{}\r\n", script.rcpt_reply)
        } else if upper == "DATA" {
            writer.write_all(b"354 end with <CRLF>.<CRLF>\r\n").await?;
            let mut data = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await? == 0 {
                    return Ok(());
                }
                if line == ".\r\n" {
                    break;
                }
                data.push_str(&line);
            }
            if script.silent_data_end {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                return Ok(());
            }
            if script.data_reply.starts_with('2') {
                current.data = data;
                mails.lock().unwrap().push(current.clone());
            }
            format!("{}\r\n", script.data_reply)
        } else if upper == "QUIT" {
            if script.silent_quit {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                return Ok(());
            }
            writer.write_all(b"221 bye\r\n").await?;
            return Ok(());
        } else {
            "250 ok\r\n".to_string()
        };
        writer.write_all(reply.as_bytes()).await?;
    }
}
