#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Debug;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use jfn_instance_ipc::{Listener, Start, Stream};
use jfn_platform_abi::Instance;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::runtime::Runtime;

#[derive(Debug, Serialize, Deserialize)]
struct Req {
    payload: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Resp {
    len: usize,
}

fn echo_len(r: &Req) -> Resp {
    Resp {
        len: r.payload.len(),
    }
}

fn tag_a(_r: &Req) -> Resp {
    Resp { len: 1 }
}

fn tag_b(_r: &Req) -> Resp {
    Resp { len: 2 }
}

fn rt() -> Runtime {
    Runtime::new().unwrap()
}

fn scratch_instance() -> (TempDir, Instance) {
    let dir = tempfile::tempdir().unwrap();
    let instance = Instance::for_config_dir(dir.path()).unwrap();
    (dir, instance)
}

fn serving<Req, Resp>(rt: &Runtime, instance: &Instance, handle: fn(&Req) -> Resp) -> Listener
where
    Req: DeserializeOwned + Debug + Send + 'static,
    Resp: Serialize + Send + Sync + 'static,
{
    match rt.block_on(Listener::try_start(instance, handle)) {
        Start::Started(listener) => listener,
        Start::AlreadyRunning => panic!("unexpected AlreadyRunning"),
        Start::Failed(e) => panic!("start failed: {e}"),
    }
}

#[test]
fn round_trip_no_truncation() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let _listener = serving(&rt, &instance, echo_len);
    let len = rt.block_on(async {
        let mut stream = Stream::connect(&instance).await.unwrap();
        stream
            .send(&Req {
                payload: "x".repeat(4096),
            })
            .await
            .unwrap();
        stream.recv::<Resp>().await.unwrap().unwrap().len
    });
    assert_eq!(len, 4096);
}

#[test]
fn many_frames_one_connection() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let _listener = serving(&rt, &instance, echo_len);
    rt.block_on(async {
        let mut stream = Stream::connect(&instance).await.unwrap();
        stream
            .send(&Req {
                payload: "abc".into(),
            })
            .await
            .unwrap();
        assert_eq!(stream.recv::<Resp>().await.unwrap().unwrap().len, 3);
        stream
            .send(&Req {
                payload: "hello".into(),
            })
            .await
            .unwrap();
        assert_eq!(stream.recv::<Resp>().await.unwrap().unwrap().len, 5);
    });
}

#[test]
fn second_bind_reports_already_running() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let _first = serving(&rt, &instance, echo_len);
    match rt.block_on(Listener::try_start(&instance, echo_len)) {
        Start::AlreadyRunning => {}
        Start::Started(_) => panic!("expected AlreadyRunning, got Started"),
        Start::Failed(e) => panic!("expected AlreadyRunning, got Failed: {e}"),
    }
}

#[cfg(unix)]
#[test]
fn stale_socket_is_reclaimed() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let path = jfn_instance_ipc::Name::for_instance(&instance)
        .unwrap()
        .path()
        .to_path_buf();
    // Only a bound-then-dropped socket yields ECONNREFUSED on connect; a plain
    // file gives ENOTSOCK and never reaches the stale path.
    let dead = std::os::unix::net::UnixListener::bind(&path).unwrap();
    drop(dead);
    assert!(path.exists());

    let _listener = serving(&rt, &instance, echo_len);
    let len = rt.block_on(async {
        let mut stream = Stream::connect(&instance).await.unwrap();
        stream
            .send(&Req {
                payload: "zz".into(),
            })
            .await
            .unwrap();
        stream.recv::<Resp>().await.unwrap().unwrap().len
    });
    assert_eq!(len, 2);
}

#[cfg(unix)]
#[test]
fn clean_drop_frees_name() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let path = jfn_instance_ipc::Name::for_instance(&instance)
        .unwrap()
        .path()
        .to_path_buf();

    let listener = serving(&rt, &instance, echo_len);
    assert!(path.exists());
    rt.block_on(listener.shutdown());
    assert!(!path.exists());

    let _again = serving(&rt, &instance, echo_len);
}

#[test]
fn drop_with_open_connection_does_not_hang() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let listener = serving(&rt, &instance, echo_len);
    let _client = rt.block_on(Stream::connect(&instance)).unwrap();

    let (tx, rx) = mpsc::channel();
    let joiner = thread::spawn(move || {
        drop(listener);
        tx.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(5))
        .expect("dropping Listener with an open connection hung");
    joiner.join().unwrap();
}

#[test]
fn distinct_instances_are_isolated() {
    let rt = rt();
    let (_dir_a, a) = scratch_instance();
    let (_dir_b, b) = scratch_instance();
    let _la = serving(&rt, &a, tag_a);
    let _lb = serving(&rt, &b, tag_b);

    let (ra, rb) = rt.block_on(async {
        let mut sa = Stream::connect(&a).await.unwrap();
        sa.send(&Req {
            payload: String::new(),
        })
        .await
        .unwrap();
        let ra = sa.recv::<Resp>().await.unwrap().unwrap().len;

        let mut sb = Stream::connect(&b).await.unwrap();
        sb.send(&Req {
            payload: String::new(),
        })
        .await
        .unwrap();
        let rb = sb.recv::<Resp>().await.unwrap().unwrap().len;
        (ra, rb)
    });
    assert_eq!(ra, 1);
    assert_eq!(rb, 2);
}

// =====================================================================
// Hostile peers
//
// The name is reachable by any local process that knows it, so everything
// below is what such a process can do to the running app: a frame that never
// ends, a message of the wrong shape, a connection it never uses, a crowd.
// The invariant in every case is that the listener keeps serving.
// =====================================================================

#[derive(Debug, Serialize)]
struct WrongShape {
    payload: u32,
}

/// One well-formed exchange, used after each hostile one to prove the
/// listener survived it.
fn still_serving(rt: &Runtime, instance: &Instance) {
    let len = rt.block_on(async {
        let mut stream = Stream::connect(instance).await.unwrap();
        stream
            .send(&Req {
                payload: "still here".into(),
            })
            .await
            .unwrap();
        stream.recv::<Resp>().await.unwrap().unwrap().len
    });
    assert_eq!(len, 10);
}

#[test]
fn the_listener_name_is_derived_from_the_instance_id() {
    let (_dir, instance) = scratch_instance();
    let name = jfn_instance_ipc::Name::for_instance(&instance).unwrap();
    let text = name.path().to_string_lossy().into_owned();
    assert!(text.contains(&instance.id().to_string()), "{text}");
    assert!(text.contains("astrofin"), "{text}");

    let (_other_dir, other) = scratch_instance();
    assert_ne!(
        jfn_instance_ipc::Name::for_instance(&other).unwrap().path(),
        name.path()
    );
}

#[test]
fn ping_is_answered_with_pong() {
    use jfn_instance_ipc::jfn::{Request, Response};

    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let _listener = serving(&rt, &instance, jfn_instance_ipc::jfn::handle);
    let answer = rt.block_on(async {
        let mut stream = Stream::connect(&instance).await.unwrap();
        stream.send(&Request::Ping).await.unwrap();
        stream.recv::<Response>().await.unwrap()
    });
    assert!(matches!(answer, Some(Response::Pong)));
}

/// A peer that writes past the frame cap is cut off. Without the cap it could
/// spend the app's memory one buffer at a time.
#[test]
fn an_oversized_frame_closes_the_connection_and_nothing_else() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let _listener = serving(&rt, &instance, echo_len);

    rt.block_on(async {
        let mut stream = Stream::connect(&instance).await.unwrap();
        let payload = "x".repeat(jfn_instance_ipc::MAX_FRAME_BYTES * 2);
        // The write itself may fail once the peer hangs up mid-frame.
        let _ = stream.send(&Req { payload }).await;
        let answered = stream.recv::<Resp>().await;
        assert!(
            !matches!(answered, Ok(Some(_))),
            "an oversized frame was answered"
        );
    });

    still_serving(&rt, &instance);
}

/// The largest frame that is still inside the cap goes through, so the cap is
/// a cap and not a smaller de-facto limit.
#[test]
fn a_frame_just_inside_the_cap_is_answered() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let _listener = serving(&rt, &instance, echo_len);
    // `{"payload":"…"}` plus the newline is 15 bytes of framing.
    let payload = "x".repeat(jfn_instance_ipc::MAX_FRAME_BYTES - 32);
    let want = payload.len();
    let len = rt.block_on(async {
        let mut stream = Stream::connect(&instance).await.unwrap();
        stream.send(&Req { payload }).await.unwrap();
        stream.recv::<Resp>().await.unwrap().unwrap().len
    });
    assert_eq!(len, want);
}

#[test]
fn a_message_of_the_wrong_shape_closes_the_connection_and_nothing_else() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let _listener = serving(&rt, &instance, echo_len);

    rt.block_on(async {
        let mut stream = Stream::connect(&instance).await.unwrap();
        stream.send(&WrongShape { payload: 42 }).await.unwrap();
        let answered = stream.recv::<Resp>().await;
        assert!(
            !matches!(answered, Ok(Some(_))),
            "a wrong-shaped message was answered"
        );
    });

    still_serving(&rt, &instance);
}

/// A client that opens the connection and then goes quiet must not stop the
/// accept loop from serving anybody else.
#[test]
fn a_client_that_never_sends_blocks_nobody() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let _listener = serving(&rt, &instance, echo_len);

    let _silent = rt.block_on(Stream::connect(&instance)).unwrap();

    still_serving(&rt, &instance);
}

/// Same for a client that hangs up without saying anything.
#[test]
fn a_client_that_disconnects_immediately_blocks_nobody() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let _listener = serving(&rt, &instance, echo_len);

    for _ in 0..8 {
        drop(rt.block_on(Stream::connect(&instance)).unwrap());
    }

    still_serving(&rt, &instance);
}

/// Replaces `many_concurrent_clients_are_all_answered`, which pinned the
/// unbounded behaviour: a crowd of peers is now capped, so what a client is
/// promised is its own answer *or* a closed connection — never a wrong answer,
/// never a hang. The first eight accepted are always served, because the count
/// of in-flight connections only ever falls while the crowd is being accepted.
#[test]
fn concurrent_clients_are_answered_up_to_the_connection_cap() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();
    let _listener = serving(&rt, &instance, echo_len);

    let answers = rt.block_on(async {
        let mut tasks = Vec::new();
        for i in 0..32usize {
            tasks.push(tokio::spawn(async move {
                let mut stream = Stream::connect(&instance).await.unwrap();
                // Both the write and the read may fail on a refused peer.
                if stream
                    .send(&Req {
                        payload: "y".repeat(i),
                    })
                    .await
                    .is_err()
                {
                    return (i, None);
                }
                (i, stream.recv::<Resp>().await.ok().flatten().map(|r| r.len))
            }));
        }
        let mut answers = Vec::new();
        for task in tasks {
            answers.push(task.await.unwrap());
        }
        answers
    });

    for (i, len) in &answers {
        if let Some(n) = len {
            assert_eq!(n, i, "client {i} was answered {n}");
        }
    }
    let served = answers.iter().filter(|(_, len)| len.is_some()).count();
    assert!(served >= 8, "only {served} of 32 clients were served");

    still_serving(&rt, &instance);
}

/// `clean_drop_frees_name` proves the same thing through the filesystem, but
/// it can only run where the name *is* a file. Everywhere else the observable
/// guarantee is that the name stops answering a probe.
#[test]
fn shutdown_stops_the_name_answering_a_probe() {
    let rt = rt();
    let (_dir, instance) = scratch_instance();

    let listener = serving(&rt, &instance, echo_len);
    // While it is up, a second bind is refused.
    assert!(matches!(
        rt.block_on(Listener::try_start::<Req, Resp>(&instance, echo_len)),
        Start::AlreadyRunning
    ));

    rt.block_on(listener.shutdown());

    // After the shutdown the probe no longer finds a server, so a second
    // start is never told the instance is already running.
    let after = rt.block_on(Listener::try_start::<Req, Resp>(&instance, echo_len));
    assert!(
        !matches!(after, Start::AlreadyRunning),
        "the name still answered a probe after shutdown"
    );
    if let Start::Started(l) = after {
        rt.block_on(l.shutdown());
    }
}
