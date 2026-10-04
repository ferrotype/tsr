use super::*;
use serde_json::json;
use std::{io::Cursor, time::Duration};

fn session() -> crate::Session {
    let mut session = crate::Session::default();
    let initialize = json!({"jsonrpc":"2.0","id":0,"method":"test/initialize","params":{
        "version":2,"caseSensitive":true,"base":{"/fallback.ts":"base"},
        "symlinks":{},"callbacks":["readFile"],"options":{},"plugins":[]
    }})
    .to_string();
    session
        .receive(&crate::framing::parse_json(initialize.as_bytes()).unwrap())
        .unwrap();
    let token = session.pending_options().unwrap().token;
    session.complete_options(token, Ok(())).unwrap();
    session
}
fn frame(receiver: &mpsc::Receiver<Json>) -> Value {
    let raw = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("router must keep pumping");
    let mut framed = Vec::new();
    crate::framing::write(&mut framed, raw.get().as_bytes()).unwrap();
    let bytes = crate::framing::read(&mut Cursor::new(framed))
        .unwrap()
        .unwrap();
    serde_json::from_str(crate::framing::parse_json(&bytes).unwrap().as_str()).unwrap()
}

#[test]
fn several_parked_workers_do_not_block_progress_or_out_of_order_replies() {
    let session = session();
    let (send, receive) = mpsc::channel();
    let router = session.callback_router(send.clone()).unwrap();
    let (fs, _cancel) = router.filesystem();
    let mut workers = Vec::new();
    for number in 0..4 {
        let fs = fs.clone();
        workers.push(std::thread::spawn(move || {
            fs.read_file(format!("/{number}.ts").as_bytes())
                .unwrap()
                .unwrap()
        }));
    }
    let requests: Vec<_> = (0..4).map(|_| frame(&receive)).collect();
    assert_eq!(router.pending_count(), 4);
    send.send(
        wire!({"jsonrpc":"2.0","method":"testhost/progress","params":wire!({"phase":"report"})}),
    )
    .unwrap();
    assert_eq!(frame(&receive)["method"], "testhost/progress");
    for request in requests.iter().rev() {
        assert_eq!(request["method"], "readFile");
        assert!(router.complete(
            request["id"].as_str().unwrap(),
            Ok(json!({"content":request["params"]}))
        ));
    }
    for (number, worker) in workers.into_iter().enumerate() {
        assert_eq!(
            worker.join().unwrap().text.as_bytes(),
            format!("/{number}.ts").as_bytes()
        );
    }
    assert_eq!(router.pending_count(), 0);
    assert!(!router.complete(requests[0]["id"].as_str().unwrap(), Ok(Value::Null)));
}

#[test]
fn cancellation_drains_all_request_slots_and_rejects_new_calls() {
    let (send, receive) = mpsc::channel();
    let router = session().callback_router(send).unwrap();
    let (fs, cancel) = router.filesystem();
    let workers: Vec<_> = (0..3)
        .map(|_| {
            let fs = fs.clone();
            std::thread::spawn(move || fs.read_file(b"/pending.ts"))
        })
        .collect();
    let requests: Vec<_> = (0..3).map(|_| frame(&receive)).collect();
    cancel.cancel();
    for _ in 0..3 {
        assert_eq!(frame(&receive)["method"], "$/cancelRequest");
    }
    for worker in workers {
        assert_eq!(
            worker.join().unwrap().unwrap_err(),
            Error::Io(std::io::ErrorKind::Interrupted)
        );
    }
    assert_eq!(router.pending_count(), 0);
    assert_eq!(
        fs.file_exists(b"/fallback.ts").unwrap_err(),
        Error::Io(std::io::ErrorKind::Interrupted)
    );
    for request in requests {
        assert!(!router.complete(
            request["id"].as_str().unwrap(),
            Ok(json!({"content":"late"}))
        ));
    }
}

#[test]
fn reset_retires_old_filesystems_without_reusing_callback_ids() {
    let session = session();
    let (send, receive) = mpsc::channel();
    let old = session.callback_router(send.clone()).unwrap();
    let (fs, _) = old.filesystem();
    let old_worker = std::thread::spawn(move || fs.read_file(b"/old.ts"));
    let old_id = frame(&receive)["id"].as_str().unwrap().to_owned();
    old.retire();
    assert_eq!(frame(&receive)["method"], "$/cancelRequest");
    assert_eq!(
        old_worker.join().unwrap().unwrap_err(),
        Error::Io(std::io::ErrorKind::ConnectionAborted)
    );
    let new = session.callback_router(send).unwrap();
    let (fs, _) = new.filesystem();
    let new_worker = std::thread::spawn(move || fs.read_file(b"/new.ts"));
    let new_id = frame(&receive)["id"].as_str().unwrap().to_owned();
    assert_ne!(old_id, new_id);
    assert!(!new.complete(&old_id, Ok(json!({"content":"old"}))));
    assert_eq!(new.pending_count(), 1);
    assert!(new.complete(&new_id, Ok(json!({"content":"new"}))));
    assert_eq!(
        new_worker.join().unwrap().unwrap().unwrap().text.as_bytes(),
        b"new"
    );
}

#[test]
fn disconnect_and_failed_output_wake_waiters() {
    let (send, receive) = mpsc::channel();
    let router = session().callback_router(send).unwrap();
    let (fs, _) = router.filesystem();
    let worker = std::thread::spawn(move || fs.read_file(b"/pending.ts"));
    frame(&receive);
    drop(router);
    assert_eq!(
        worker.join().unwrap().unwrap_err(),
        Error::Io(std::io::ErrorKind::BrokenPipe)
    );
    let (send, receive) = mpsc::channel();
    let router = session().callback_router(send).unwrap();
    let (fs, _) = router.filesystem();
    drop(receive);
    assert_eq!(
        fs.read_file(b"/pending.ts").unwrap_err(),
        Error::Io(std::io::ErrorKind::BrokenPipe)
    );
    assert_eq!(router.pending_count(), 0);
}

#[test]
fn fallback_and_callback_decoding_use_the_s11_host_contract() {
    let (send, receive) = mpsc::channel();
    let router = session().callback_router(send).unwrap();
    let (fs, _) = router.filesystem();
    assert!(fs.file_exists(b"/fallback.ts").unwrap());
    assert!(receive.try_recv().is_err());
    let worker = std::thread::spawn(move || fs.read_file(b"/fallback.ts"));
    let id = frame(&receive)["id"].as_str().unwrap().to_owned();
    router.complete(&id, Ok(Value::Null));
    assert_eq!(
        worker.join().unwrap().unwrap().unwrap().text.as_bytes(),
        b"base"
    );
    let (fs, _) = router.filesystem();
    let worker = std::thread::spawn(move || fs.read_file(b"/x.ts"));
    let id = frame(&receive)["id"].as_str().unwrap().to_owned();
    router.complete(&id, Ok(json!({})));
    assert!(worker.join().unwrap().unwrap().is_none());
}
