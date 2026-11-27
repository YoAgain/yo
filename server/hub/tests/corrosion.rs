//! Two local agents, two hubs, one inbox. Set YO_CORROSION_BIN to run.

use libsignal_protocol::KeyPair;
use rusqlite::{Connection, OpenFlags};
use std::{
    fs,
    net::{SocketAddr, TcpListener, UdpSocket},
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::Arc,
    time::Duration,
};
use tokio::net::TcpListener as Listener;
use yo_hub::{
    config::{Config, CorrosionConfig},
    now_ms, push, Hub,
};
use yoip::net::{api, cert, devices, handle::Handle, request};

struct Cluster {
    root: PathBuf,
    agents: Vec<Child>,
}

impl Drop for Cluster {
    fn drop(&mut self) {
        for child in &mut self.agents {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("timed out waiting for replication or startup");
}

fn count(path: &Path, sql: &str) -> Option<i64> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?.query_row(sql, [], |r| r.get(0)).ok()
}

impl Cluster {
    async fn start(binary: &str) -> (Self, Vec<CorrosionConfig>) {
        let root = std::env::temp_dir().join(format!("yo-corro-{}-{}", std::process::id(), now_ms()));
        fs::create_dir(&root).unwrap();
        let schema = root.join("schema");
        fs::create_dir(&schema).unwrap();
        fs::write(schema.join("yo.sql"), include_str!("../corrosion/yo.sql")).unwrap();
        let mut cluster = Self { root, agents: Vec::new() };
        let gossip = [(); 2].map(|_| UdpSocket::bind("127.0.0.1:0").unwrap());
        let peers = gossip.each_ref().map(|s| s.local_addr().unwrap().to_string());
        let listeners = [(); 2].map(|_| TcpListener::bind("127.0.0.1:0").unwrap());
        let mut configs = Vec::new();
        for (i, (socket, listener)) in gossip.into_iter().zip(listeners).enumerate() {
            let dir = cluster.root.join(i.to_string());
            fs::create_dir(&dir).unwrap();
            let db = dir.join("data.db");
            let conf = dir.join("config.toml");
            let api = listener.local_addr().unwrap();
            let bootstrap = &peers[1 - i];
            let addr = socket.local_addr().unwrap();
            let admin = dir.join("a.sock");
            fs::write(
                &conf,
                format!(
                    r#"[db]
path = {db:?}
schema_paths = [{schema:?}]
cache_size_kib = -16384
[gossip]
addr = "{addr}"
bootstrap = ["{bootstrap}"]
plaintext = true
[api]
addr = "{api}"
[admin]
path = {admin:?}
"#
                ),
            )
            .unwrap();
            drop((listener, socket));
            cluster.agents.push(Command::new(binary).args(["-c", conf.to_str().unwrap(), "agent"]).spawn().unwrap());
            until(|| count(&db, "SELECT count(*) FROM message").is_some() && std::net::TcpStream::connect(api).is_ok())
                .await;
            configs.push(CorrosionConfig {
                api: format!("http://{api}"),
                db: db.to_str().unwrap().into(),
                hub: format!("hub{i}"),
                index: i as u8,
                upstream: None,
            });
        }
        (cluster, configs)
    }
}

struct Phone {
    user: yoip::UserId,
    device: yoip::DeviceId,
    hash: [u8; 32],
    auth: KeyPair,
}

fn public(pair: &KeyPair) -> [u8; 32] {
    pair.public_key.public_key_bytes().try_into().unwrap()
}

impl Phone {
    async fn register(url: &str, name: &str, n: u8) -> Self {
        let account = KeyPair::generate(&mut rand::rng());
        let auth = KeyPair::generate(&mut rand::rng());
        let key = public(&KeyPair::generate(&mut rand::rng()));
        let device = [n; 16];
        let handle = Handle::username(name);
        let sign =
            |msg: &[u8]| (*account.private_key.calculate_signature(msg, &mut rand::rng()).unwrap()).try_into().unwrap();
        let cert =
            cert::DeviceCertificate { account_key: public(&account), signature: sign(&cert::to_be_signed(&key)) };
        let pk = api::SignedPrekey { id: 1, key: vec![1; 32].into(), signature: vec![1; 64].into() };
        let body = api::Register {
            username: handle.value.clone(),
            attest: None,
            device: api::NewDevice {
                device_id: device,
                device_key: key,
                auth_key: public(&auth),
                certificate: cert.to_bytes().into(),
                push: api::Push::none(),
                registration_id: 1,
                proof: sign(&api::registration_to_be_signed(&handle, &device, &key, &public(&auth))),
                prekeys: api::Prekeys {
                    signed: Some(pk.clone()),
                    last_resort: Some(pk),
                    one_time: vec![],
                    kyber: vec![],
                },
            },
        };
        let bytes = reqwest::Client::new()
            .post(format!("{url}{}", api::REGISTER))
            .body(yoip::encode(&body))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let user = yoip::decode::<api::Registered>(&bytes).unwrap().user_id;
        Self { user, device, hash: devices::hash(&public(&account), &[key]), auth }
    }

    fn headers(&self, method: &str, path: &str, body: &[u8]) -> [(&'static str, String); 3] {
        let time = now_ms();
        let data = request::to_be_signed(method, path, time, body);
        let sig = self.auth.private_key.calculate_signature(&data, &mut rand::rng()).unwrap();
        [
            (request::HEADER_TIME, time.to_string()),
            (request::HEADER_SIGNATURE, yoip::b64(&sig)),
            (request::HEADER_DEVICE, yoip::b64(&self.device)),
        ]
    }

    async fn wrapped_get(&self, url: &str, path: &str) -> Vec<u8> {
        use yo_hub::config::{TEST_OHTTP_KEY, TEST_OHTTP_KEY_ID};
        use yoip::net::limits;
        let mut message = bhttp::Message::request(b"GET".to_vec(), b"https".to_vec(), Vec::new(), path.into());
        for (name, value) in self.headers("GET", path, &[]) {
            message.put_header(name, value);
        }
        message.put_header("date", httpdate::fmt_http_date(std::time::SystemTime::now()));
        let mut plain = Vec::new();
        message.write_bhttp(bhttp::Mode::KnownLength, &mut plain).unwrap();
        plain.resize(limits::ohttp_padded(plain.len(), limits::OHTTP_REQUEST_OVERHEAD).unwrap(), 0);
        let key = yo_hub::ohttp::key_config(TEST_OHTTP_KEY_ID, &TEST_OHTTP_KEY).unwrap();
        let (wrapped, opener) = ohttp::ClientRequest::from_encoded_config(&key).unwrap().encapsulate(&plain).unwrap();
        let response = reqwest::Client::new()
            .post(format!("{url}{}", api::OHTTP))
            .header("content-type", api::OHTTP_REQUEST_TYPE)
            .body(wrapped)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let plain = opener.decapsulate(&response).unwrap();
        let reply = bhttp::Message::read_bhttp(&mut std::io::Cursor::new(plain)).unwrap();
        assert_eq!(reply.control().status().unwrap().code(), 200);
        reply.content().to_vec()
    }

    async fn signed(&self, url: &str, method: reqwest::Method, path: &str, body: Vec<u8>) -> bytes::Bytes {
        let headers = self.headers(method.as_str(), path, &body);
        let mut req = reqwest::Client::new().request(method, format!("{url}{path}"));
        for (name, value) in headers {
            req = req.header(name, value);
        }
        req.body(body).send().await.unwrap().error_for_status().unwrap().bytes().await.unwrap()
    }

    async fn inbox(&self, url: &str, wait: u64) -> api::Inbox {
        yoip::decode(&self.signed(url, reqwest::Method::GET, &api::inbox_path(wait), vec![]).await).unwrap()
    }

    async fn deliver(&self, url: &str) {
        let body = vec![yoip::protocol::envelope::SEALED; 1024];
        reqwest::Client::new()
            .post(format!("{url}{}/{}", api::INBOX, yoip::b64(&self.user)))
            .header(api::HEADER_DEVICES, yoip::b64(&self.hash))
            .body(body)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
}

async fn listener() -> (Listener, String) {
    let listener = Listener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    (listener, origin)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_hubs_share_delivery_wakes_and_counts() {
    let Ok(bin) = std::env::var("YO_CORROSION_BIN") else {
        eprintln!("skipping: set YO_CORROSION_BIN");
        return;
    };
    let (cluster, mut configs) = Cluster::start(&bin).await;
    let (upstream, authority) = listener().await;
    configs[1].upstream = Some(authority.clone());
    let config = |i: usize| {
        let mut c = Config::for_tests();
        c.db_path = cluster.root.join(format!("hub{i}.db")).to_str().unwrap().into();
        c.blobs.dir = cluster.root.join(format!("blobs{i}"));
        c.corrosion = Some(configs[i].clone());
        c
    };
    let central_config = config(0);
    // A pending envelope from a standalone hub keeps the id an older client may have seen.
    drop(yo_hub::db::Db::open(&central_config.db_path).unwrap());
    Connection::open(&central_config.db_path)
        .unwrap()
        .execute(
            "INSERT INTO message(id,device_id,envelope,received_at) VALUES(42,?,?,?)",
            rusqlite::params![[2u8; 16], vec![7u8; 1024], now_ms()],
        )
        .unwrap();
    let central = Hub::new(central_config.clone(), Arc::new(push::Recorder::default())).unwrap();
    let edge = Hub::new(config(1), Arc::new(push::Recorder::default())).unwrap();
    assert_eq!(count(Path::new(&central_config.db_path), "SELECT count(*) FROM message"), Some(0));
    // Serve the authority before its mirror starts, so the negative fallback is deterministic.
    let router = central.router().into_make_service_with_connect_info::<SocketAddr>();
    let authority_task = tokio::spawn(async move {
        axum::serve(upstream, router).await.unwrap();
    });
    let (edge_socket, edge_url) = listener().await;
    let (edge_stop, stop) = tokio::sync::oneshot::channel();
    let edge_task = tokio::spawn(edge.clone().serve(edge_socket, async {
        let _ = stop.await;
    }));
    let phone = Arc::new(Phone::register(&authority, "MAILBOXX", 1).await);
    let other = Phone::register(&authority, "ANOTHERBOX", 2).await;
    assert_eq!(count(Path::new(&configs[1].db), "SELECT count(*) FROM account"), Some(0));
    other.deliver(&edge_url).await;
    let forwarded: api::Inbox = yoip::decode(&other.wrapped_get(&edge_url, api::INBOX).await).unwrap();
    let devices = other.wrapped_get(&edge_url, api::DEVICES).await;
    assert_eq!(yoip::decode::<api::OwnDevices>(&devices).unwrap().devices.len(), 1);
    assert_eq!(forwarded.items.len(), 2);
    assert_eq!(forwarded.items[0].id, 42);
    let id = forwarded.items[1].id;
    assert_eq!((id >> 18) & 15, 0);
    other.signed(&edge_url, reqwest::Method::POST, api::INBOX_ACK, yoip::encode(&api::Ack { ids: vec![42, id] })).await;

    let (socket, url) = listener().await;
    let (central_stop, stop) = tokio::sync::oneshot::channel();
    let central_task = tokio::spawn(central.clone().serve(socket, async {
        let _ = stop.await;
    }));
    until(|| count(Path::new(&configs[1].db), "SELECT count(*) FROM account_device") == Some(2)).await;
    authority_task.abort();
    let _ = authority_task.await;
    let polling = phone.clone();
    let poll_url = url.clone();
    let poll = tokio::spawn(async move { polling.inbox(&poll_url, 30).await });
    until(|| central.under_way().0 == 1).await;
    phone.deliver(&edge_url).await;
    let inbox = tokio::time::timeout(Duration::from_secs(15), poll).await.unwrap().unwrap();
    assert_eq!(inbox.items.len(), 1);
    let id = inbox.items[0].id;
    assert_eq!((id >> 18) & 15, 1);
    assert_eq!(phone.inbox(&edge_url, 0).await.items, inbox.items);
    let wrapped: api::Inbox = yoip::decode(&phone.wrapped_get(&edge_url, api::INBOX).await).unwrap();
    assert_eq!(wrapped.items, inbox.items);
    phone.signed(&edge_url, reqwest::Method::POST, api::INBOX_ACK, yoip::encode(&api::Ack { ids: vec![id] })).await;
    until(|| count(Path::new(&configs[0].db), "SELECT count(*) FROM message") == Some(0)).await;
    assert!(phone.inbox(&url, 0).await.items.is_empty());

    edge.flush_events(now_ms()).unwrap();
    until(|| {
        count(Path::new(&configs[0].db), "SELECT sum(n) FROM hub_stat_minute WHERE hub='hub1' AND kind='yo-in'")
            == Some(1)
    })
    .await;
    central.flush_events(now_ms()).unwrap();
    central.flush_events(now_ms()).unwrap();
    assert_eq!(count(&cluster.root.join("hub0.db"), "SELECT sum(value) FROM stat WHERE metric='deliveries'"), Some(2));
    phone.deliver(&edge_url).await;
    let rollback_id = phone.inbox(&edge_url, 0).await.items[0].id;
    until(|| count(Path::new(&configs[0].db), "SELECT id FROM message") == Some(rollback_id as i64)).await;
    central_stop.send(()).unwrap();
    edge_stop.send(()).unwrap();
    central_task.await.unwrap().unwrap();
    edge_task.await.unwrap().unwrap();
    for _ in 0..2 {
        yo_hub::db::Db::corrosion_back(&central_config).unwrap();
    }
    let local = yo_hub::db::Db::open(&central_config.db_path).unwrap();
    let restored = local.fetch(&phone.device, 10).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].0, rollback_id);
}
