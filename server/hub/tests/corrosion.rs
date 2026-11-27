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
    async fn start(binary: &str, joined: bool) -> (Self, Vec<CorrosionConfig>) {
        let root =
            std::env::temp_dir().join(format!("yo-corro-{}-{}-{joined}", std::process::id(), rand::random::<u64>()));
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
            let bootstrap = if joined { format!("{:?}", peers[1 - i]) } else { String::new() };
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
bootstrap = [{bootstrap}]
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
                mesh_peers: None,
                peer_token: None,
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

    async fn wrapped_get(&self, url: &str, path: &str, status: u16) -> Vec<u8> {
        self.wrapped_get_with(url, path, status, self.headers("GET", path, &[])).await
    }

    async fn wrapped_get_with(&self, url: &str, path: &str, status: u16, headers: [(&str, String); 3]) -> Vec<u8> {
        use yo_hub::config::{TEST_OHTTP_KEY, TEST_OHTTP_KEY_ID};
        use yoip::net::limits;
        let mut message = bhttp::Message::request(b"GET".to_vec(), b"https".to_vec(), Vec::new(), path.into());
        for (name, value) in headers {
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
        assert_eq!(reply.control().status().unwrap().code(), status);
        reply.content().to_vec()
    }

    async fn signed(&self, url: &str, method: reqwest::Method, path: &str, body: Vec<u8>) -> bytes::Bytes {
        let headers = self.headers(method.as_str(), path, &body);
        let mut req = reqwest::Client::new().request(method, format!("{url}{path}"));
        for (name, value) in headers {
            req = req.header(name, value);
        }
        let response = req.body(body).send().await.unwrap().error_for_status().unwrap();
        if path.starts_with(api::INBOX) && path != api::INBOX_ACK {
            assert_eq!(response.status(), 200);
        }
        response.bytes().await.unwrap()
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
    let (cluster, mut configs) = Cluster::start(&bin, true).await;
    let (upstream, authority) = listener().await;
    configs[1].upstream = Some(authority.clone());
    let edge_token = yoip::b64(&rand::random::<[u8; 32]>());
    configs[1].peer_token = Some(edge_token.clone());
    let config = |i: usize| {
        let mut c = Config::for_tests();
        c.db_path = cluster.root.join(format!("hub{i}.db")).to_str().unwrap().into();
        c.blobs.dir = cluster.root.join(format!("blobs{i}"));
        c.corrosion = Some(configs[i].clone());
        c
    };
    let mut central_config = config(0);
    let admin_token = yoip::b64(&rand::random::<[u8; 32]>());
    central_config.admin_token = Some(admin_token.clone());
    // A pending envelope from a standalone hub keeps the id an older client may have seen.
    drop(yo_hub::db::Db::open(&central_config.db_path).unwrap());
    Connection::open(&central_config.db_path)
        .unwrap()
        .execute(
            "INSERT INTO message(id,device_id,envelope,received_at) VALUES(42,?,?,?)",
            rusqlite::params![[2u8; 16], vec![7u8; 1024], now_ms()],
        )
        .unwrap();
    central_config.apns = Some(yo_hub::config::ApnsConfig {
        key_pem: include_str!("../../../tests/fixtures/test-apns.p8").into(),
        key_id: "test".into(),
        team_id: "test".into(),
        topic: "test".into(),
    });
    let recorder = Arc::new(push::Recorder::default());
    *recorder.device_status.lock().unwrap() = Some(429);
    let central = Hub::new(central_config.clone(), recorder).unwrap();
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
    let push =
        api::Push { platform: api::PUSH_APNS, token: format!("{:064x}", rand::random::<u128>()).into_bytes().into() };
    other.signed(&authority, reqwest::Method::PUT, api::PUSH, yoip::encode(&push)).await;
    other.deliver(&edge_url).await;
    let forwarded: api::Inbox = yoip::decode(&other.wrapped_get(&edge_url, api::INBOX, 200).await).unwrap();
    let devices = other.wrapped_get(&edge_url, api::DEVICES, 200).await;
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
    let denied = reqwest::get(format!("{edge_url}/v1/peer/inbox/{}", yoip::b64(&phone.device))).await.unwrap();
    assert_eq!(denied.status(), 404, "unauthorized peer reads stay on the edge");
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
    let peer = reqwest::Client::new()
        .get(format!("{edge_url}/v1/peer/inbox/{}", yoip::b64(&phone.device)))
        .header("yo-peer-token", edge_token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let peer: api::Inbox = yoip::decode(&peer).unwrap();
    assert_eq!(peer.prekeys_left, 0);
    assert_eq!(peer.items, inbox.items);
    let wrapped: api::Inbox = yoip::decode(&phone.wrapped_get(&edge_url, api::INBOX, 200).await).unwrap();
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
    let stats: serde_json::Value = reqwest::Client::new()
        .get(format!("{url}/v1/admin/stats"))
        .header("yo-admin-token", admin_token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let total = |hub: &str, kind: &str| {
        let bins = stats["hubs"][hub][kind].as_array().unwrap();
        assert_eq!(bins.len(), 144);
        bins.iter().map(|v| v.as_u64().unwrap()).sum::<u64>()
    };
    assert_eq!(total("hub0", "push-fail-apns"), 1);
    assert_eq!(total("hub0", "push-fail-apns-throttled"), 1);
    assert_eq!(total("hub0", "accepted"), 1);
    assert_eq!(total("hub1", "accepted"), 1);
    assert_eq!(total("hub0", "delivered"), 2);
    assert_eq!(total("hub1", "delivered"), 1);
    assert_eq!(total("hub0", "sync-in"), 1);
    assert_eq!(
        stats["hubs"]["hub0"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with("sync-lt") || k.as_str() == "sync-ge5000")
            .flat_map(|(_, v)| v.as_array().unwrap())
            .map(|v| v.as_u64().unwrap())
            .sum::<u64>(),
        1
    );
    assert!(stats["hubs"]["hub1"].get("yo-in").is_none());
    phone.deliver(&edge_url).await;
    let rollback_id = phone.inbox(&edge_url, 0).await.items[0].id;
    until(|| count(Path::new(&configs[0].db), "SELECT id FROM message") == Some(rollback_id as i64)).await;
    let result: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/v1/transactions", configs[0].api))
        .json(&["INSERT INTO message(id,hub) VALUES (10,'old'),(11,'old')"])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["results"][0]["rows_affected"], 2);
    central.prune(now_ms()).unwrap();
    central.flush_events(now_ms()).unwrap();
    assert_eq!(
        count(Path::new(&configs[0].db), "SELECT sum(n) FROM hub_stat_minute WHERE kind='mesh-expired'"),
        Some(2)
    );
    central_stop.send(()).unwrap();
    edge_stop.send(()).unwrap();
    central_task.await.unwrap().unwrap();
    edge_task.await.unwrap().unwrap();
    let now = now_ms();
    let accepted = |hub: &str| {
        count(
            Path::new(&configs[1].db),
            &format!("SELECT sum(n) FROM hub_stat_minute WHERE hub='{hub}' AND kind='accepted'"),
        )
    };
    edge.flush_events(now).unwrap();
    assert_eq!(accepted("hub1"), Some(2));
    edge.flush_events(now + 47 * 3_600_000).unwrap();
    assert_eq!(accepted("hub1"), Some(2));
    edge.flush_events(now + 48 * 3_600_000).unwrap();
    assert_eq!(accepted("hub1"), None);
    until(|| accepted("hub0") == Some(1)).await;
    for _ in 0..2 {
        yo_hub::db::Db::corrosion_back(&central_config).unwrap();
    }
    let local = yo_hub::db::Db::open(&central_config.db_path).unwrap();
    let restored = local.fetch(&phone.device, 10).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].0, rollback_id);
}

// One real mesh listener forwards to B. A uses the outbound-only client, and the heartbeat of a node of its own.
async fn peer_mesh(
    root: &Path,
    name: &str,
    forward: SocketAddr,
    stalled: SocketAddr,
) -> (PathBuf, tokio::task::JoinHandle<yo_mesh::Result<()>>) {
    use rcgen::{BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair};
    let dir = root.join("tls");
    fs::create_dir(&dir).unwrap();
    let mut ca = CertificateParams::default();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let key = KeyPair::generate().unwrap();
    fs::write(dir.join("ca.crt"), ca.self_signed(&key).unwrap().pem()).unwrap();
    let issuer = Issuer::new(ca, key);
    let node = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec![name.into()]).unwrap();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
    fs::write(dir.join("node.key"), node.serialize_pem()).unwrap();
    fs::write(dir.join("node.crt"), params.signed_by(&node, &issuer).unwrap().pem()).unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = socket.local_addr().unwrap();
    let reader = TcpListener::bind("127.0.0.1:0").unwrap();
    let reader_listen = reader.local_addr().unwrap();
    drop((socket, reader));
    let path = root.join("mesh.toml");
    let status = root.join("reader-status.json");
    fs::write(
        &path,
        format!("name='reader'\ncerts={dir:?}\nstatus={status:?}\n[peers]\n{name}='{listen}'\nstalled='{stalled}'\n"),
    )
    .unwrap();
    tokio::spawn(yo_mesh::run(yo_mesh::Config {
        name: "reader".into(),
        listen: reader_listen,
        peers: [(name.into(), listen), ("stalled".into(), stalled)].into(),
        certs: Some(dir.clone()),
        forward: None,
        status: Some(status.clone()),
        history: Some(root.join("reader-history.json")),
    }));
    let task = tokio::spawn(yo_mesh::run(yo_mesh::Config {
        name: name.into(),
        listen,
        peers: Default::default(),
        certs: Some(dir),
        forward: Some(forward),
        status: Some(root.join("mesh-status")),
        history: None,
    }));
    until(|| std::net::TcpStream::connect(listen).is_ok()).await;
    // The stalled peer's heartbeat times out: a short fetch does not wait for it.
    until(|| {
        fs::read(&status)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .is_some_and(|s| s["peers"]["stalled"]["online"] == false && s["peers"]["stalled"]["error"].is_string())
    })
    .await;
    (path, task)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_only_ack_survives_replication() {
    let Ok(bin) = std::env::var("YO_CORROSION_BIN") else { return };
    let (mut cluster, mut agents) = Cluster::start(&bin, false).await;
    let (socket_b, origin_b) = listener().await;
    // This peer accepts TCP but never completes TLS. Every fan-out must still return B's answer.
    let stalled = TcpListener::bind("127.0.0.1:0").unwrap();
    let (mesh_path, mesh) =
        peer_mesh(&cluster.root, "hub1", socket_b.local_addr().unwrap(), stalled.local_addr().unwrap()).await;
    let token = yoip::b64(&rand::random::<[u8; 32]>());
    agents[0].mesh_peers = Some(mesh_path);
    for agent in &mut agents {
        agent.peer_token = Some(token.clone());
    }
    let config = |i: usize| {
        let mut c = Config::for_tests();
        c.db_path = cluster.root.join(format!("hub{i}.db")).to_str().unwrap().into();
        c.blobs.dir = cluster.root.join(format!("blobs{i}"));
        c.corrosion = Some(agents[i].clone());
        c
    };
    let a = config(0);
    let b = config(1);
    let db_b = b.db_path.clone();
    let hub_b = Hub::new(b, Arc::new(push::Recorder::default())).unwrap();
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let router = hub_b
        .router()
        .layer(axum::middleware::from_fn(move |req: axum::extract::Request, next: axum::middleware::Next| {
            if req.uri().path().starts_with("/v1/peer/") {
                counter.fetch_add(1, Ordering::Relaxed);
            }
            async move { next.run(req).await }
        }))
        .into_make_service_with_connect_info::<SocketAddr>();
    let server_b = tokio::spawn(async move { axum::serve(socket_b, router).await.unwrap() });
    let phone = Phone::register(&origin_b, "PEERBOX", 1).await;
    // Both authorities know the same device before the disconnected writes.
    Connection::open(&db_b).unwrap().execute("VACUUM INTO ?", [&a.db_path]).unwrap();
    let db_a = PathBuf::from(&a.db_path);
    let hub_a = Hub::new(a, Arc::new(push::Recorder::default())).unwrap();
    let (socket_a, origin_a) = listener().await;
    let router = hub_a.router().into_make_service_with_connect_info::<SocketAddr>();
    let server_a = tokio::spawn(async move { axum::serve(socket_a, router).await.unwrap() });
    phone.deliver(&origin_b).await;
    let remote = phone.inbox(&origin_b, 0).await.items;
    assert_eq!(remote.len(), 1);
    assert_eq!(count(Path::new(&agents[0].db), "SELECT count(*) FROM message"), Some(0));
    assert!(phone.inbox(&origin_a, 1).await.items.is_empty());
    phone.deliver(&origin_a).await;
    let split = |step: u8| {
        let phone = &phone;
        let url = &origin_a;
        async move {
            let path =
                if step == 1 { format!("{}?wait=0&split=1", api::INBOX) } else { format!("{}?split=2", api::INBOX) };
            let mut req = reqwest::Client::new().get(format!("{url}{path}"));
            for (name, value) in phone.headers("GET", &path, &[]) {
                req = req.header(name, value);
            }
            let reply = req.send().await.unwrap();
            assert_eq!(reply.status().as_u16(), if step == 1 { 206 } else { 200 });
            yoip::decode::<api::Inbox>(&reply.bytes().await.unwrap()).unwrap().items
        }
    };
    let local = split(1).await;
    assert_eq!(local.len(), 1);
    assert_ne!(local[0].id, remote[0].id);
    assert_eq!(calls.load(Ordering::Relaxed), 0, "long polls and split=1 never queried B");
    assert_eq!(tokio::time::timeout(Duration::from_secs(2), split(2)).await.unwrap(), remote);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let wrapped: api::Inbox =
        yoip::decode(&phone.wrapped_get(&origin_a, "/v1/inbox?wait=0&split=1", 206).await).unwrap();
    assert_eq!(wrapped.items, local);
    let endpoint = format!("/v1/peer/inbox/{}", yoip::b64(&phone.device));
    assert!(!yo_hub::ohttp::served_inside(&endpoint));
    phone.wrapped_get(&origin_b, &endpoint, 404).await;
    let denied = reqwest::Client::new()
        .get(format!("{origin_b}{endpoint}"))
        .header("yo-peer-token", "wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 404);
    let without_peers = phone.signed(&origin_b, reqwest::Method::GET, "/v1/inbox?wait=0&split=1", vec![]).await;
    assert_eq!(yoip::decode::<api::Inbox>(&without_peers).unwrap().items, remote);
    let followup = phone.signed(&origin_b, reqwest::Method::GET, "/v1/inbox?split=2", vec![]).await;
    assert!(yoip::decode::<api::Inbox>(&followup).unwrap().items.is_empty());
    let mut both = [local.clone(), remote.clone()].concat();
    both.sort_by_key(|item| item.id);
    assert_eq!(phone.inbox(&origin_a, 0).await.items, both);
    let id = remote[0].id;
    let other = Phone::register(&origin_a, "OTHERBOX", 2).await;
    let ack = yoip::encode(&api::Ack { ids: vec![id] });
    other.signed(&origin_a, reqwest::Method::POST, api::INBOX_ACK, ack.clone()).await;
    phone.signed(&origin_a, reqwest::Method::POST, api::INBOX_ACK, ack).await;
    assert_eq!(phone.inbox(&origin_b, 0).await.items, remote);
    assert_eq!(phone.inbox(&origin_a, 0).await.items, local);
    phone.signed(&origin_a, reqwest::Method::POST, api::INBOX_ACK, yoip::encode(&api::Ack { ids: vec![id] })).await;
    hub_a.flush_events(now_ms()).unwrap();
    for (kind, n) in [("accepted", 1), ("delivered", 1), ("peer-hit", 2), ("peer-items", 2), ("peer-miss", 3)] {
        assert_eq!(
            count(
                Path::new(&agents[0].db),
                &format!("SELECT sum(n) FROM hub_stat_minute WHERE hub='hub0' AND kind='{kind}'")
            ),
            Some(n),
            "{kind}"
        );
    }
    assert_eq!(count(&db_a, "SELECT sum(value) FROM stat WHERE metric='yo-out'"), Some(1));
    assert_eq!(count(&db_a, "SELECT coalesce(sum(value),0) FROM stat WHERE metric LIKE 'latency-%'"), Some(0));
    // Start the checker with both events past its grace period while gossip is still cut.
    hub_a.shift_clock(35_000);
    let (socket, _) = listener().await;
    let (stop, done) = tokio::sync::oneshot::channel();
    let checker = tokio::spawn(hub_a.clone().serve(socket, async {
        let _ = done.await;
    }));
    until(|| {
        hub_a.flush_events(now_ms()).unwrap();
        ["mesh-stuck", "mesh-ack-lost", "mesh-check-skip"].into_iter().all(|kind| {
            count(
                Path::new(&agents[0].db),
                &format!("SELECT sum(n) FROM hub_stat_minute WHERE hub='hub0' AND kind='{kind}'"),
            ) == Some(1)
        })
    })
    .await;
    assert_eq!(count(Path::new(&agents[0].db), "SELECT sum(n) FROM hub_stat_minute WHERE kind='mesh-dup'"), Some(1));
    hub_a.shift_clock(0);
    // An independent delivery in each direction witnesses replication after reconnecting.
    phone.deliver(&origin_b).await;
    let witness = phone.inbox(&origin_b, 0).await.items.into_iter().find(|item| item.id != id).unwrap();
    let mut survivors = local;
    survivors.push(witness);
    survivors.sort_by_key(|item| item.id);
    let conf_a = fs::read_to_string(cluster.root.join("0/config.toml")).unwrap();
    let addr_a = conf_a.split("[gossip]\naddr = ").nth(1).unwrap().lines().next().unwrap();
    let conf_b = cluster.root.join("1/config.toml");
    let joined = fs::read_to_string(&conf_b).unwrap().replace("bootstrap = []", &format!("bootstrap = [{addr_a}]"));
    cluster.agents[1].kill().unwrap();
    cluster.agents[1].wait().unwrap();
    fs::write(&conf_b, joined).unwrap();
    cluster.agents[1] = Command::new(&bin).arg("-c").arg(&conf_b).arg("agent").spawn().unwrap();
    let converged =
        format!("SELECT count(*)=2 AND min(id)={} AND max(id)={} FROM message", survivors[0].id, survivors[1].id);
    until(|| agents.iter().all(|a| count(Path::new(&a.db), &converged) == Some(1))).await;
    assert_eq!(phone.inbox(&origin_a, 0).await.items, survivors);
    assert_eq!(phone.inbox(&origin_b, 0).await.items, survivors);
    let answer: serde_json::Value = reqwest::Client::new()
        .post(format!("{origin_b}/v1/peer/check"))
        .header("yo-peer-token", &token)
        .json(&[id, survivors[0].id, 7])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(answer["deleted"], serde_json::json!([id]));
    assert_eq!(answer["alive"], serde_json::json!([survivors[0].id]));
    stop.send(()).unwrap();
    checker.await.unwrap().unwrap();
    for task in [server_a, server_b] {
        task.abort();
    }
    mesh.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edge_off_forwards_wakes_polls_and_drains_deleted_mail() {
    let Ok(bin) = std::env::var("YO_CORROSION_BIN") else { return };
    let (mut cluster, mut configs) = Cluster::start(&bin, true).await;
    let (socket, central_url) = listener().await;
    let (peer_socket, _) = listener().await;
    let stalled = TcpListener::bind("127.0.0.1:0").unwrap();
    let (mesh_path, mesh) =
        peer_mesh(&cluster.root, "yo-falkenstein", peer_socket.local_addr().unwrap(), stalled.local_addr().unwrap())
            .await;
    let token = yoip::b64(&rand::random::<[u8; 32]>());
    configs[0].hub = "falkenstein".into();
    configs[0].peer_token = Some(token.clone());
    let mut config = Config::for_tests();
    config.corrosion = Some(configs[0].clone());
    config.db_path = cluster.root.join("central.db").to_str().unwrap().into();
    let central = Hub::new(config.clone(), Arc::new(push::Recorder::default())).unwrap();
    let (stop, done) = tokio::sync::oneshot::channel();
    let central_task = tokio::spawn(central.clone().serve(socket, async {
        let _ = done.await;
    }));
    let (relay, relay_url) = listener().await;
    let relay_task = tokio::spawn(async move {
        axum::serve(relay, axum::Router::new().fallback(|| async { "relay" })).await.unwrap();
    });
    // A separate process gives this edge a private off file and captures the actual operator-visible drain log.
    let root = &cluster.root;
    let off = root.join("off");
    let log = root.join("edge.log");
    let (socket, edge_url) = listener().await;
    let addr = socket.local_addr().unwrap();
    drop(socket);
    for (name, bytes) in
        [("key", config.hub_key.as_slice()), ("cert", &config.server_certificate), ("zk", &config.zk_params)]
    {
        fs::write(root.join(name), bytes).unwrap();
    }
    cluster.agents.push(
        Command::new(env!("CARGO_BIN_EXE_yo-hub"))
            .env_clear()
            .env("YO_DB", root.join("edge.db"))
            .env("YO_LISTEN", addr.to_string())
            .env("YO_HUB_NAME", "edge")
            .env("YO_HUB_INDEX", "1")
            .env("YO_EDGE_UPSTREAM", &relay_url)
            .env("YO_CORROSION_API", &configs[1].api)
            .env("YO_CORROSION_DB", &configs[1].db)
            .env("YO_MESH_PEERS", mesh_path)
            .env("YO_EDGE_OFF_FILE", &off)
            .env("YO_PEER_TOKEN", &token)
            .env("YO_HUB_KEY_FILE", root.join("key"))
            .env("YO_SERVER_CERT_FILE", root.join("cert"))
            .env("YO_ZK_PARAMS_FILE", root.join("zk"))
            .stderr(fs::File::create(&log).unwrap())
            .spawn()
            .unwrap(),
    );
    until(|| std::net::TcpStream::connect(addr).is_ok()).await;
    let phone = Arc::new(Phone::register(&central_url, "SWITCHBOX", 1).await);
    until(|| count(Path::new(&configs[1].db), "SELECT count(*) FROM account_device") == Some(1)).await;
    phone.deliver(&edge_url).await;
    let items = phone.inbox(&edge_url, 0).await.items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id >> 18 & 15, 1);
    phone
        .signed(&edge_url, reqwest::Method::POST, api::INBOX_ACK, yoip::encode(&api::Ack { ids: vec![items[0].id] }))
        .await;

    let polling = phone.clone();
    let url = edge_url.clone();
    let poll = tokio::spawn(async move { polling.inbox(&url, 60).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    fs::write(&off, []).unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(4), poll).await.unwrap().unwrap().items.is_empty());
    // The central mesh endpoint is still unreachable, though gossip has replicated the tombstone.
    until(|| fs::read_to_string(&log).unwrap().contains("EDGE-OFF waiting n=1")).await;
    assert!(!fs::read_to_string(&log).unwrap().contains("EDGE-OFF drained"));
    let peer_router = central.router().into_make_service_with_connect_info::<SocketAddr>();
    let peer_task = tokio::spawn(async move { axum::serve(peer_socket, peer_router).await.unwrap() });
    until(|| fs::read_to_string(&log).unwrap().contains("EDGE-OFF drained n=1")).await;
    let root_response = || async { reqwest::get(format!("{edge_url}/")).await.unwrap().text().await.unwrap() };
    assert_eq!(root_response().await, "relay");
    let command = |action: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_yo-hub"))
            .env_clear()
            .env("YO_CORROSION_API", &configs[0].api)
            .env("YO_HUB_NAME", "falkenstein")
            .env("YO_HUB_INDEX", "0")
            .env("YO_CORROSION_DB", &configs[0].db)
            .args(["edges", action])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    };
    assert_eq!(command("status"), "edges on\n");
    assert_eq!(command("off"), "edges off\n");
    assert_eq!(command("status"), "edges off\n");
    until(|| count(Path::new(&configs[1].db), "SELECT value='off' FROM hub_setting WHERE key='edges'") == Some(1))
        .await;
    fs::remove_file(off).unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(root_response().await, "relay");
    assert_eq!(command("on"), "edges on\n");
    until(|| fs::read_to_string(&log).unwrap().contains("EDGE-ON")).await;
    assert_ne!(root_response().await, "relay");
    mesh.abort();
    relay_task.abort();
    peer_task.abort();
    stop.send(()).unwrap();
    central_task.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edges_share_replays_limits_and_push_upkeep() {
    let Ok(bin) = std::env::var("YO_CORROSION_BIN") else { return };
    let (cluster, mut agents) = Cluster::start(&bin, true).await;
    let (central_socket, central_url) = listener().await;
    let (edge_socket, edge_url) = listener().await;
    agents[1].upstream = Some(central_url.clone());
    let token = yoip::b64(&rand::random::<[u8; 32]>());
    for a in &mut agents {
        a.peer_token = Some(token.clone());
    }
    let config = |i: usize| {
        let mut c = Config::for_tests();
        c.db_path = cluster.root.join(format!("hub{i}.db")).to_str().unwrap().into();
        c.blobs.dir = cluster.root.join(format!("blobs{i}"));
        c.corrosion = Some(agents[i].clone());
        c.limits.signed_per_device_minute = 6;
        c
    };
    let main = cluster.root.join("hub0.db");
    let central = Hub::new(config(0), Arc::new(push::Recorder::default())).unwrap();
    let mut ec = config(1);
    ec.apns = Some(yo_hub::config::ApnsConfig {
        key_pem: include_str!("../../../tests/fixtures/test-apns.p8").into(),
        key_id: "test".into(),
        team_id: "test".into(),
        topic: "test".into(),
    });
    let recorder = Arc::new(push::Recorder::default());
    *recorder.device_status.lock().unwrap() = Some(410);
    let edge = Hub::new(ec, recorder.clone()).unwrap();
    let (central_stop, done) = tokio::sync::oneshot::channel();
    let central_task = tokio::spawn(central.serve(central_socket, async {
        let _ = done.await;
    }));
    let (edge_stop, done) = tokio::sync::oneshot::channel();
    let edge_task = tokio::spawn(edge.serve(edge_socket, async {
        let _ = done.await;
    }));
    let phone = Phone::register(&central_url, "REPLAYBOX", 1).await;
    let fresh = Phone::register(&central_url, "LIMITBOX", 2).await;
    let replica = Path::new(&agents[1].db);
    let authority = Path::new(&agents[0].db);
    until(|| count(replica, "SELECT count(*) FROM account_device") == Some(2)).await;
    let path = "/v1/inbox?wait=0";
    let get = |url: &str, headers: [(&str, String); 3]| {
        let mut req = reqwest::Client::new().get(format!("{url}{path}"));
        for (name, value) in headers {
            req = req.header(name, value);
        }
        async move { req.send().await.unwrap().status().as_u16() }
    };
    let same = phone.headers("GET", path, &[]);
    assert_eq!(get(&edge_url, same.clone()).await, 200);
    assert_eq!(get(&edge_url, same.clone()).await, 401);
    // A new OHTTP encapsulation carrying the same signed request also retains the edge's refusal.
    phone.wrapped_get_with(&edge_url, path, 401, same.clone()).await;
    until(|| count(authority, "SELECT count(*) FROM hub_seen WHERE hub='hub1'") == Some(1)).await;
    assert_eq!(get(&central_url, same).await, 401);

    for _ in 0..3 {
        assert_eq!(get(&edge_url, fresh.headers("GET", path, &[])).await, 200);
    }
    let spent = format!(
        "SELECT sum(json_extract(value,'$[1]'))=3 FROM hub_limit,json_each(slots)
         WHERE hub='hub1' AND hub_limit.key='signed:{}'",
        yoip::b64(&fresh.device)
    );
    until(|| count(authority, &spent) == Some(1)).await;
    for _ in 0..3 {
        assert_eq!(get(&central_url, fresh.headers("GET", path, &[])).await, 200);
    }
    assert_eq!(get(&central_url, fresh.headers("GET", path, &[])).await, 429);

    let push =
        api::Push { platform: api::PUSH_APNS, token: format!("{:064x}", rand::random::<u128>()).into_bytes().into() };
    phone.signed(&central_url, reqwest::Method::PUT, api::PUSH, yoip::encode(&push)).await;
    until(|| count(replica, "SELECT count(*) FROM account_device WHERE length(push_token)>0") == Some(1)).await;
    phone.deliver(&edge_url).await;
    until(|| !recorder.sent.lock().unwrap().is_empty()).await;
    let cleared = "SELECT count(*) FROM account_device WHERE push_token=x''";
    until(|| count(&main, cleared) == Some(2) && count(replica, cleared) == Some(2)).await;
    Connection::open(&main)
        .unwrap()
        .execute("UPDATE account_device SET last_seen_at=0 WHERE id=?", [phone.device])
        .unwrap();
    until(|| count(replica, "SELECT count(*) FROM account_device WHERE last_seen_at=0") == Some(1)).await;
    assert_eq!(get(&edge_url, phone.headers("GET", path, &[])).await, 200);
    let at = yo_hub::db::to_the_hour(now_ms());
    until(|| {
        Connection::open(&main)
            .unwrap()
            .query_row("SELECT last_seen_at FROM account_device WHERE id=?", [phone.device], |r| r.get::<_, u64>(0))
            .unwrap()
            == at
    })
    .await;
    central_stop.send(()).unwrap();
    edge_stop.send(()).unwrap();
    central_task.await.unwrap().unwrap();
    edge_task.await.unwrap().unwrap();
}
