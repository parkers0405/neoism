use neoism_neoworld::{NeoWorldStore, StoredPet};
use neoism_neoworld_core::{PetState, Vec2};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::OnceLock;

struct RuntimeHandle {
    pet: std::sync::Mutex<StoredPet>,
    snapshots: SyncSender<PetState>,
}

static RUNTIME: OnceLock<Result<RuntimeHandle, String>> = OnceLock::new();

pub(crate) fn initial_pet() -> Option<StoredPet> {
    runtime()
        .and_then(|runtime| {
            runtime
                .pet
                .lock()
                .map(|pet| pet.clone())
                .map_err(|_| "NeoWorld pet cache is poisoned")
        })
        .map_err(|error| {
            tracing::error!(target: "neoism::neoworld", %error, "NeoWorld persistence unavailable");
        })
        .ok()
}

pub(crate) fn persist(state: PetState) {
    if let Ok(runtime) = runtime() {
        if let Ok(mut pet) = runtime.pet.lock() {
            pet.state = state;
        }
        let _ = runtime.snapshots.try_send(state);
    }
}

fn runtime() -> Result<&'static RuntimeHandle, &'static str> {
    RUNTIME
        .get_or_init(start_runtime)
        .as_ref()
        .map_err(String::as_str)
}

fn start_runtime() -> Result<RuntimeHandle, String> {
    let data_root = dirs::data_local_dir()
        .unwrap_or_else(neoism_backend::config::config_dir_path)
        .join("neoism")
        .join("neoworld");
    let database_path = data_root.join("neoworld.db");
    let (snapshots, receiver) = std::sync::mpsc::sync_channel(8);
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("neoworld-store".to_string())
        .spawn(move || store_worker(database_path, receiver, ready_tx))
        .map_err(|error| format!("failed to start NeoWorld store worker: {error}"))?;
    let initial = ready_rx
        .recv()
        .map_err(|_| "NeoWorld store worker stopped during startup".to_string())??;
    Ok(RuntimeHandle {
        pet: std::sync::Mutex::new(initial),
        snapshots,
    })
}

fn store_worker(
    path: std::path::PathBuf,
    receiver: Receiver<PetState>,
    ready: SyncSender<Result<StoredPet, String>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ =
                ready.send(Err(format!("failed to create NeoWorld runtime: {error}")));
            return;
        }
    };
    let store = match runtime.block_on(NeoWorldStore::open(path)) {
        Ok(store) => store,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    let mut pet = match runtime
        .block_on(store.load_or_create_local_pet("", Vec2::new(120.0, 150.0)))
    {
        Ok(pet) => pet,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    if ready.send(Ok(pet.clone())).is_err() {
        return;
    }

    while let Ok(mut state) = receiver.recv() {
        while let Ok(newer) = receiver.try_recv() {
            state = newer;
        }
        pet.state = state;
        if let Err(error) = runtime.block_on(store.save_pet(&pet)) {
            tracing::error!(target: "neoism::neoworld", %error, "failed to persist pet snapshot");
        }
    }
}
