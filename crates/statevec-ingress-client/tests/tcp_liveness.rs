#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::time::Duration;

use statevec_ingress_client::{BatchConnectIo, BatchTcpWorker};

fn command(program: &str, args: &[&str]) -> String {
    let output = Command::new(program).args(args).output().unwrap();
    assert!(output.status.success(), "{program} {args:?}: {:?}", output);
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Linux user/network namespaces, ip and iptables"]
fn lost_probe_survives_but_sustained_loss_retires_socket() {
    const CHILD: &str = "STATEVEC_KEEPALIVE_NETNS_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new("unshare")
            .args(["--user", "--map-root-user", "--net"])
            .arg(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "lost_probe_survives_but_sustained_loss_retires_socket", "--nocapture"])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success(), "isolated kernel keepalive regression: {status}");
        return;
    }
    command("ip", &["link", "set", "lo", "up"]);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let mut worker = BatchTcpWorker::new().unwrap();
    let mut client = worker.connect(address).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    client.set_nonblocking(false).unwrap();
    client.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    client.write_all(b"ready").unwrap();
    peer.read_exact(&mut [0; 5]).unwrap();

    // Drop only the first idle probe, not the handshake or application bytes.
    let port = address.port().to_string();
    command("iptables", &["-A", "INPUT", "-p", "tcp", "--dport", &port,
        "-m", "statistic", "--mode", "nth", "--every", "1000", "--packet", "0", "-j", "DROP"]);
    client.set_read_timeout(Some(Duration::from_millis(7500))).unwrap();
    let error = client.read(&mut [0; 1]).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock,
        "one lost probe must not close a healthy idle connection: {error:?}");
    let counters = command("iptables", &["-L", "INPUT", "-n", "-v", "-x"]);
    let dropped = counters.lines().find(|line| line.contains("DROP")).unwrap();
    assert_eq!(dropped.split_whitespace().next(), Some("1"), "{counters}");
    peer.write_all(b"!").unwrap();
    let mut byte = [0];
    client.read_exact(&mut byte).unwrap();
    assert_eq!(byte, *b"!");

    command("iptables", &["-R", "INPUT", "1", "-p", "tcp", "--dport", &port, "-j", "DROP"]);
    client.set_read_timeout(Some(Duration::from_secs(12))).unwrap();
    let error = client.read(&mut [0; 1]).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::ETIMEDOUT),
        "sustained packet loss must retire the socket before the fixture timeout: {error:?}");
}
