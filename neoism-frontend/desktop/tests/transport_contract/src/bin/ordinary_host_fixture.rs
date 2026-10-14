//! Owned, bounded protocol child for the real persistent-host startup driver.
//! It is NOT an authentication server or a production workspace/Agent process.
use neoism_workspace_daemon::standalone::{
    StartupAcknowledgement, FAST_READINESS_PROTOCOL, HOSTED_READINESS_NONCE_ENV,
    HOSTED_READINESS_PROTOCOL,
};
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = &args[1];
    let port: u16 = args[2].parse().unwrap();
    std::fs::write(&args[3], std::process::id().to_string()).unwrap();
    if mode == "exit" {
        eprintln!("fixture association 409 credential=contract-ordinary-secret");
        std::process::exit(9);
    }
    let acknowledgement = StartupAcknowledgement {
        protocol: if mode == "fast" {
            FAST_READINESS_PROTOCOL
        } else {
            HOSTED_READINESS_PROTOCOL
        }
        .into(),
        nonce: if mode == "nonce" {
            "wrong-owned-nonce".into()
        } else {
            std::env::var(HOSTED_READINESS_NONCE_ENV).unwrap()
        },
        address: std::net::SocketAddr::from((
            [0, 0, 0, 0],
            if mode == "port" { port + 1 } else { port },
        )),
    };
    println!("{}", serde_json::to_string(&acknowledgement).unwrap());
    std::io::stdout().flush().unwrap();
    // Success deliberately detaches; self-expiry bounds even a broken driver.
    // Failure cases are killed/reaped immediately by the actual source driver.
    std::thread::sleep(std::time::Duration::from_secs(1));
}
