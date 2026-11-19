use clap::Parser;
use neochat::{board::{random_token, now}, protocol::Discovery, Board};
use std::{fs::{self, OpenOptions}, io::{self, Write}, net::SocketAddr, path::{Path, PathBuf}, time::Duration};

#[derive(Parser)]
#[command(about="Durable board-only Neoboard service (never executes board content)")]
struct Args {
    #[arg(long, env="NEOCHAT_BIND", default_value="127.0.0.1:0")]
    bind: SocketAddr,
    #[arg(long, env="NEOCHAT_DATA_DIR", default_value_os_t=neochat::default_data_dir())]
    data_dir: PathBuf,
}

fn private_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options=OpenOptions::new(); options.write(true).create_new(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    options.open(path)
}
fn token(path: &Path) -> io::Result<String> {
    match private_file(path) {
        Ok(mut f) => { let token=random_token(); f.write_all(token.as_bytes())?; f.sync_all()?; Ok(token) }
        Err(e) if e.kind()==io::ErrorKind::AlreadyExists => Ok(fs::read_to_string(path)?.trim().to_owned()),
        Err(e)=>Err(e),
    }
}

#[tokio::main]
async fn main() -> Result<(),Box<dyn std::error::Error>> {
    let args=Args::parse();
    if !args.bind.ip().is_loopback() {
        eprintln!("warning: non-loopback bind uses plaintext HTTP; use a trusted private network or TLS tunnel, never a public port");
    }
    fs::create_dir_all(&args.data_dir)?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&args.data_dir,fs::Permissions::from_mode(0o700))?; }
    // OS file locking prevents a second service from replacing a live discovery endpoint.
    let lock=OpenOptions::new().create(true).truncate(false).write(true).open(args.data_dir.join("service.lock"))?;
    lock.try_lock()?;
    let board_token=token(&args.data_dir.join("board-token"))?;
    let admin_token=token(&args.data_dir.join("admin-token"))?;
    let board=Board::open(args.data_dir.join("board.sqlite3"),&board_token,&admin_token)?;
    let listener=tokio::net::TcpListener::bind(args.bind).await?;
    let bound=listener.local_addr()?;
    let host=if bound.ip().is_unspecified() { SocketAddr::new(if bound.is_ipv4() { "127.0.0.1".parse()? } else { "::1".parse()? },bound.port()) } else { bound };
    let discovery=Discovery { url:format!("http://{host}"),token:board_token };
    let discovery_path=args.data_dir.join("endpoint.json");
    let pending=args.data_dir.join(format!(".endpoint-{}-{}",std::process::id(),now()));
    let mut file=private_file(&pending)?; file.write_all(&serde_json::to_vec_pretty(&discovery)?)?; file.sync_all()?;
    fs::rename(&pending,&discovery_path)?;
    println!("{}",discovery_path.display());
    let tick_board=board.clone();
    let (stop_tx,mut stop_rx)=tokio::sync::watch::channel(false);
    let worker=tokio::spawn(async move {
        loop {
            tokio::select! {
                _=stop_rx.changed()=>break,
                _=tokio::time::sleep(Duration::from_millis(250))=>{
                    let b=tick_board.clone();
                    match tokio::task::spawn_blocking(move||b.tick()).await { Ok(Ok(()))=>{}, other=>eprintln!("maintenance error: {other:?}") }
                }
            }
        }
    });
    let result=axum::serve(listener,neochat::router(board)).with_graceful_shutdown(shutdown()).await;
    let _=stop_tx.send(true); let _=worker.await;
    let _=fs::remove_file(discovery_path);
    drop(lock);
    result?; Ok(())
}
async fn shutdown() {
    #[cfg(unix)] {
        let mut term=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=term.recv()=>{} }
    }
    #[cfg(not(unix))] { let _=tokio::signal::ctrl_c().await; }
}