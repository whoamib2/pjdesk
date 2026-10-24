//! Password-before-approval on a real QUIC connection.
//!
//! An unattended host with a permanent password accepts that password and
//! never asks anyone. A wrong password is a distinct, immediate refusal.
//! Five failures lock the source; the next try is told how long to wait and
//! the password is not checked again.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pjdesk_link::admit::{self, Admit};
use pjdesk_link::lockout::{self, Lockout};
use pjdesk_link::p2p;
use anyhow::Error;
use pjdesk_link::session::{self, grant_auth, Authenticated, LockedOut, WrongPassword};
use pjdesk_proto::signal::Policy;
use pjdesk_proto::tls::TlsIdentity;
use pjdesk_proto::PEER_SERVER_NAME;

struct Pair {
    host: quinn::Endpoint,
    controller: quinn::Endpoint,
    addr: std::net::SocketAddr,
}

fn pair() -> Pair {
    let identity = TlsIdentity::generate(PEER_SERVER_NAME).unwrap();
    let fingerprint = identity.fingerprint();
    let host_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    host_sock.set_nonblocking(true).unwrap();
    let addr = host_sock.local_addr().unwrap();
    let host = p2p::host_endpoint(host_sock, &identity).unwrap();
    let ctrl_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    ctrl_sock.set_nonblocking(true).unwrap();
    let controller = p2p::controller_endpoint(ctrl_sock, fingerprint).unwrap();
    Pair { host, controller, addr }
}

fn must_err(result: Result<Authenticated, Error>) -> Error {
    match result {
        Ok(_) => panic!("expected the host to refuse"),
        Err(err) => err,
    }
}

async fn dial(controller: &quinn::Endpoint, addr: std::net::SocketAddr) -> quinn::Connection {
    controller.connect(addr, PEER_SERVER_NAME).unwrap().await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unattended_permanent_password_opens_with_no_approval() {
    let pair = pair();
    let approvals = Arc::new(AtomicU32::new(0));
    let counted = approvals.clone();
    let host = pair.host;
    let serve = tokio::spawn(async move {
        let incoming = host.accept().await.expect("the controller connects");
        let connection = incoming.await.unwrap();
        let passwords = vec!["one-time-pass".into(), "permanent-secret".into()];
        let attempt = AtomicBool::new(false);
        let fail = || "wrong password".to_string();
        let mut auth = session::host_auth(&connection, &passwords, "CABINDESK", &attempt, &fail).await.unwrap();
        let matched = admit::matched_permanent(&passwords, auth.password_slot, Some("permanent-secret"));
        assert!(matched, "the permanent password is slot 1");
        match admit::after_password("unattended", matched, true, &Policy::default(), false) {
            Admit::Open(_) => grant_auth(&mut auth.send, "CABINDESK").await.unwrap(),
            Admit::Ask => {
                counted.fetch_add(1, Ordering::SeqCst);
            }
        }
        // The real host keeps this connection for the session. Hold it until
        // the controller has read the success verdict.
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(auth);
        drop(connection);
    });

    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let connection = dial(&pair.controller, pair.addr).await;
        session::controller_auth(&connection, "permanent-secret", "BENLENOVODESK").await
    })
    .await
    .expect("the password check finished");
    let auth = result.expect("a valid permanent password is accepted");
    assert_eq!(auth.peer_name, "CABINDESK");
    assert_eq!(auth.password_slot, 1);
    serve.await.unwrap();
    assert_eq!(approvals.load(Ordering::SeqCst), 0, "no Allow prompt was requested");
    assert!(matches!(
        admit::after_password("prompt", true, true, &Policy::default(), false),
        Admit::Open(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_password_is_refused_immediately_and_does_not_ask() {
    let pair = pair();
    let approvals = Arc::new(AtomicU32::new(0));
    let counted = approvals.clone();
    let host = pair.host;
    let serve = tokio::spawn(async move {
        let incoming = host.accept().await.expect("the controller connects");
        let connection = incoming.await.unwrap();
        let passwords = vec!["one-time-pass".into(), "permanent-secret".into()];
        let attempt = AtomicBool::new(false);
        let fail = || "wrong password".to_string();
        let err = must_err(session::host_auth(&connection, &passwords, "CABINDESK", &attempt, &fail).await);
        assert!(err.is::<WrongPassword>(), "{err:#}");
        // Reaching an Allow prompt would count here. A wrong password must not.
        let _ = counted;
    });

    let started = Instant::now();
    let err = tokio::time::timeout(Duration::from_secs(8), async {
        let connection = dial(&pair.controller, pair.addr).await;
        session::controller_auth(&connection, "not-the-password", "BENLENOVODESK").await
    })
    .await
    .expect("the host answered");
    let err = must_err(err);
    assert!(started.elapsed() < Duration::from_secs(2), "a wrong password must not wait on an Allow prompt");
    assert!(err.is::<WrongPassword>(), "{err:#}");
    assert!(!err.is::<LockedOut>());
    serve.await.unwrap();
    assert_eq!(approvals.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn five_failures_lock_the_source_and_a_later_try_is_told_to_wait() {
    let pair = pair();
    let lock = Arc::new(Mutex::new(Lockout::default()));
    let checks = Arc::new(AtomicU32::new(0));
    let host = pair.host;
    let host_lock = lock.clone();
    let host_checks = checks.clone();
    tokio::spawn(async move {
        for _ in 0..6 {
            let Some(incoming) = host.accept().await else { break };
            let lock = host_lock.clone();
            let checks = host_checks.clone();
            tokio::spawn(async move {
                let connection = incoming.await.unwrap();
                let source = lockout::source_key(connection.remote_address().ip(), None, None, None, "BEN");
                let locked = lock.lock().unwrap().check(&source, std::time::Instant::now());
                if locked.locked {
                    let _ = session::refuse_before_password(&connection, &lockout::message(locked.minutes)).await;
                    return;
                }
                let passwords = vec!["one-time-pass".into(), "permanent-secret".into()];
                let attempt = AtomicBool::new(false);
                let lock = lock.clone();
                let source = source.clone();
                let checks = checks.clone();
                let fail = move || {
                    checks.fetch_add(1, Ordering::SeqCst);
                    let status = lock.lock().unwrap().fail(&source, std::time::Instant::now());
                    if status.locked { lockout::message(status.minutes) } else { "wrong password".into() }
                };
                let _ = session::host_auth(&connection, &passwords, "CABINDESK", &attempt, &fail).await;
            });
        }
    });

    for n in 0..4 {
        let err = tokio::time::timeout(Duration::from_secs(8), async {
            let connection = dial(&pair.controller, pair.addr).await;
            session::controller_auth(&connection, "guess", "BENLENOVODESK").await
        })
        .await
        .unwrap_or_else(|_| panic!("attempt {n} timed out"));
        let err = must_err(err);
        assert!(err.is::<WrongPassword>(), "attempt {n}: {err:#}");
    }

    let tripped = tokio::time::timeout(Duration::from_secs(8), async {
        let connection = dial(&pair.controller, pair.addr).await;
        session::controller_auth(&connection, "guess", "BENLENOVODESK").await
    })
    .await
    .expect("the fifth answer arrived");
    let tripped = must_err(tripped);
    assert!(tripped.is::<LockedOut>(), "{tripped:#}");
    let text = tripped.to_string();
    assert!(text.starts_with("locked out, try again in "), "{text}");
    assert!(text.ends_with(" minutes"), "{text}");
    assert_eq!(checks.load(Ordering::SeqCst), 5);

    let still = tokio::time::timeout(Duration::from_secs(8), async {
        let connection = dial(&pair.controller, pair.addr).await;
        // The real password must not get through, and must not be checked.
        session::controller_auth(&connection, "permanent-secret", "BENLENOVODESK").await
    })
    .await
    .expect("a locked source is answered");
    let still = must_err(still);
    assert!(still.is::<LockedOut>(), "{still:#}");
    assert_eq!(still.to_string(), text);
    assert_eq!(checks.load(Ordering::SeqCst), 5, "a locked attempt does not check the password");
}
