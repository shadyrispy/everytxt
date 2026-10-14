//! R5 服务化验收：Inproc 对拍 + SocketClient 失败回退语义

use stcore::index::{IndexDoc, Writer};
use stcore::search::{search, SearchFilter, SearchMode};
use stcore::service::{InprocBackend, SearchService, ServiceSearchRequest, SocketClient};

fn seed(dir: &std::path::Path) {
    let mut w = Writer::open(dir).unwrap();
    let docs = [
        ("d1", "/data/docs/a.txt", "中华人民共和国 崛起"),
        ("d2", "/data/docs/b.md", "中华人民共和国 崛起"),
    ];
    for (name, path, body) in docs {
        w.upsert(&IndexDoc {
            path: path.to_string(),
            title: format!("{name}.txt"),
            body: body.to_string(),
            size: body.len() as u64,
            mtime: 0,
        })
        .unwrap();
    }
    w.commit().unwrap();
}

#[test]
fn inproc_matches_direct_search() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());

    let backend = InprocBackend::new(dir.path());
    let resp = backend
        .search(ServiceSearchRequest {
            query: "中华人民共和国".to_string(),
            ..Default::default()
        })
        .unwrap();

    let mgr = stcore::index::IndexManager::open(dir.path()).unwrap();
    let direct = search(
        &mgr,
        "中华人民共和国",
        SearchMode::Fuzzy,
        false,
        20,
        &SearchFilter::default(),
    )
    .unwrap();

    let got: Vec<String> = resp.hits.iter().map(|h| h.path.clone()).collect();
    let want: Vec<String> = direct.iter().map(|h| h.path.clone()).collect();
    assert_eq!(got, want);
    assert_eq!(got.len(), 2);
}

#[test]
fn inproc_stats_reflects_docs() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let stats = InprocBackend::new(dir.path()).stats().unwrap();
    assert_eq!(stats.docs, 2);
    assert!(stats.disk_bytes > 0);
}

#[test]
fn socket_client_missing_instance_returns_err() {
    // 主实例不在（socket 文件不存在）→ Err，调用方（CLI /s）据此回退本地直跑
    let dir = tempfile::tempdir().unwrap();
    let client = SocketClient::new(dir.path().join("no-such.sock"));
    let err = client
        .search(ServiceSearchRequest {
            query: "x".to_string(),
            ..Default::default()
        })
        .unwrap_err();
    assert!(!err.is_empty());

    assert!(client.stats().is_err());
}
