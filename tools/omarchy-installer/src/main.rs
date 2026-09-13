#[path = "../../../neoism-frontend/desktop/src/bootstrap_omarchy.rs"]
mod installer;

fn main() {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["--install-host"] {
        eprintln!("No changes made. Explicit opt-in: cargo run --manifest-path tools/omarchy-installer/Cargo.toml -- --install-host");
        std::process::exit(2);
    }
    match installer::run_host() {
        Ok(message) => println!("{message}"),
        Err(error) => {
            eprintln!("Omarchy installer: {error}");
            std::process::exit(1);
        }
    }
}
