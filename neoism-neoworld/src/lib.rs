#![forbid(unsafe_code)]

//! Turso persistence for NeoWorld device identity and authoritative pet state.

use anyhow::{Context, Result};
use neoism_neoworld_core::{Emotions, PetId, PetMode, PetState, Vec2};
use std::path::Path;
use turso::Value;
use uuid::Uuid;

const SCHEMA_VERSION: i64 = 2;

#[derive(Clone)]
pub struct NeoWorldStore {
    database: turso::Database,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredPet {
    pub device_id: Uuid,
    pub name: String,
    pub state: PetState,
}

impl NeoWorldStore {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create NeoWorld data directory {}",
                    parent.display()
                )
            })?;
        }
        let path = path
            .to_str()
            .context("NeoWorld Turso path is not valid UTF-8")?;
        let database = turso::Builder::new_local(path)
            .build()
            .await
            .with_context(|| format!("failed to open NeoWorld Turso database {path}"))?;
        let store = Self { database };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        let conn = self.database.connect()?;
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS local_device (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                device_id TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS pets (
                pet_id TEXT PRIMARY KEY,
                device_id TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                mode INTEGER NOT NULL,
                position_x REAL NOT NULL,
                position_y REAL NOT NULL,
                velocity_x REAL NOT NULL,
                velocity_y REAL NOT NULL,
                happiness INTEGER NOT NULL,
                irritation INTEGER NOT NULL,
                excitement INTEGER NOT NULL,
                affection INTEGER NOT NULL,
                tiredness INTEGER NOT NULL,
                loneliness INTEGER NOT NULL,
                facing_right INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .await?;
        conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (?, ?)",
            vec![
                Value::Integer(SCHEMA_VERSION),
                Value::Integer(unix_seconds()),
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn create_pet_for_device(
        &self,
        device_id: Uuid,
        pet_id: Uuid,
        name: &str,
        initial_position: Vec2,
    ) -> Result<StoredPet> {
        let state = PetState::new(PetId(*pet_id.as_bytes()), initial_position);
        let conn = self.database.connect()?;
        conn.execute(
            r#"
            INSERT OR IGNORE INTO pets (
                pet_id, device_id, name, mode, position_x, position_y,
                velocity_x, velocity_y, happiness, irritation, excitement,
                affection, tiredness, loneliness, facing_right, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            pet_values(device_id, name, &state),
        )
        .await?;
        self.load_pet_for_device(device_id)
            .await?
            .context("NeoWorld failed to create the device pet")
    }

    pub async fn load_or_create_local_pet(
        &self,
        name: &str,
        initial_position: Vec2,
    ) -> Result<StoredPet> {
        let conn = self.database.connect()?;
        let mut rows = conn
            .query("SELECT device_id FROM local_device WHERE singleton = 1", ())
            .await?;
        let device_id = match rows.next().await? {
            Some(row) => Uuid::parse_str(&row.get::<String>(0)?)?,
            None => {
                let generated = Uuid::now_v7();
                conn.execute(
                    "INSERT INTO local_device (singleton, device_id, created_at) VALUES (1, ?, ?)",
                    vec![
                        Value::Text(generated.to_string()),
                        Value::Integer(unix_seconds()),
                    ],
                )
                .await?;
                generated
            }
        };
        if let Some(pet) = self.load_pet_for_device(device_id).await? {
            return Ok(pet);
        }
        let name = if name.trim().is_empty() {
            default_pet_name(device_id).to_owned()
        } else {
            name.to_owned()
        };
        self.create_pet_for_device(device_id, Uuid::now_v7(), &name, initial_position)
            .await
    }

    pub async fn load_pet_for_device(
        &self,
        device_id: Uuid,
    ) -> Result<Option<StoredPet>> {
        let conn = self.database.connect()?;
        let mut rows = conn
            .query(
                r#"
                SELECT pet_id, device_id, name, mode, position_x, position_y,
                       velocity_x, velocity_y, happiness, irritation, excitement,
                       affection, tiredness, loneliness, facing_right
                FROM pets WHERE device_id = ?
                "#,
                vec![Value::Text(device_id.to_string())],
            )
            .await?;
        let Some(row) = rows.next().await? else {
            return Ok(None);
        };
        let pet_id = Uuid::parse_str(&row.get::<String>(0)?)?;
        let stored_device_id = Uuid::parse_str(&row.get::<String>(1)?)?;
        let name = row.get::<String>(2)?;
        let mode = match row.get::<i64>(3)? {
            0 => PetMode::Critter,
            1 => PetMode::Agent,
            value => anyhow::bail!("unknown NeoWorld pet mode {value}"),
        };
        let state = PetState::restored(
            PetId(*pet_id.as_bytes()),
            mode,
            Vec2::new(row.get::<f64>(4)? as f32, row.get::<f64>(5)? as f32),
            Vec2::new(row.get::<f64>(6)? as f32, row.get::<f64>(7)? as f32),
            Emotions {
                happiness: integer_u16(&row, 8)?,
                irritation: integer_u16(&row, 9)?,
                excitement: integer_u16(&row, 10)?,
                affection: integer_u16(&row, 11)?,
                tiredness: integer_u16(&row, 12)?,
                loneliness: integer_u16(&row, 13)?,
            },
            row.get::<i64>(14)? != 0,
        );
        Ok(Some(StoredPet {
            device_id: stored_device_id,
            name,
            state,
        }))
    }

    pub async fn save_pet(&self, pet: &StoredPet) -> Result<()> {
        let conn = self.database.connect()?;
        let pet_id = Uuid::from_bytes(pet.state.id.0);
        let values = vec![
            mode_value(pet.state.mode),
            real(pet.state.position.x),
            real(pet.state.position.y),
            real(pet.state.velocity.x),
            real(pet.state.velocity.y),
            int(pet.state.emotions.happiness),
            int(pet.state.emotions.irritation),
            int(pet.state.emotions.excitement),
            int(pet.state.emotions.affection),
            int(pet.state.emotions.tiredness),
            int(pet.state.emotions.loneliness),
            Value::Integer(i64::from(pet.state.facing_right)),
            Value::Integer(unix_seconds()),
            Value::Text(pet_id.to_string()),
            Value::Text(pet.device_id.to_string()),
        ];
        conn.execute(
            r#"
            UPDATE pets SET mode = ?, position_x = ?, position_y = ?,
                velocity_x = ?, velocity_y = ?, happiness = ?, irritation = ?,
                excitement = ?, affection = ?, tiredness = ?, loneliness = ?,
                facing_right = ?, updated_at = ?
            WHERE pet_id = ? AND device_id = ?
            "#,
            values,
        )
        .await?;
        Ok(())
    }
}

fn pet_values(device_id: Uuid, name: &str, state: &PetState) -> Vec<Value> {
    vec![
        Value::Text(Uuid::from_bytes(state.id.0).to_string()),
        Value::Text(device_id.to_string()),
        Value::Text(name.to_owned()),
        mode_value(state.mode),
        real(state.position.x),
        real(state.position.y),
        real(state.velocity.x),
        real(state.velocity.y),
        int(state.emotions.happiness),
        int(state.emotions.irritation),
        int(state.emotions.excitement),
        int(state.emotions.affection),
        int(state.emotions.tiredness),
        int(state.emotions.loneliness),
        Value::Integer(i64::from(state.facing_right)),
        Value::Integer(unix_seconds()),
    ]
}

fn mode_value(mode: PetMode) -> Value {
    Value::Integer(match mode {
        PetMode::Critter => 0,
        PetMode::Agent => 1,
    })
}

fn real(value: f32) -> Value {
    Value::Real(value as f64)
}

fn int(value: u16) -> Value {
    Value::Integer(i64::from(value))
}

fn integer_u16(row: &turso::Row, index: usize) -> Result<u16> {
    Ok(u16::try_from(row.get::<i64>(index)?)?)
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn default_pet_name(device_id: Uuid) -> &'static str {
    const NAMES: [&str; 16] = [
        "Bix", "Clover", "Dot", "Fig", "Kip", "Luma", "Miso", "Mochi", "Nim", "Pip",
        "Pogo", "Tavi", "Tink", "Tofu", "Wisp", "Zig",
    ];
    NAMES[usize::from(device_id.as_bytes()[15] & 15)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_device_keeps_one_pet_and_round_trips_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = NeoWorldStore::open(dir.path().join("neoworld.db"))
            .await
            .unwrap();
        let device_id = Uuid::now_v7();
        let original_id = Uuid::now_v7();
        let mut pet = store
            .create_pet_for_device(device_id, original_id, "Pip", Vec2::new(80.0, 100.0))
            .await
            .unwrap();

        let duplicate = store
            .create_pet_for_device(
                device_id,
                Uuid::now_v7(),
                "Duplicate",
                Vec2::new(0.0, 0.0),
            )
            .await
            .unwrap();
        assert_eq!(duplicate.state.id, PetId(*original_id.as_bytes()));
        assert_eq!(duplicate.name, "Pip");

        pet.state.emotions.irritation = 777;
        pet.state.position = Vec2::new(22.0, 44.0);
        store.save_pet(&pet).await.unwrap();
        let loaded = store.load_pet_for_device(device_id).await.unwrap().unwrap();
        assert_eq!(loaded.state.emotions.irritation, 777);
        assert_eq!(loaded.state.position, Vec2::new(22.0, 44.0));
    }

    #[tokio::test]
    async fn local_device_identity_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("neoworld.db");
        let first = NeoWorldStore::open(&path)
            .await
            .unwrap()
            .load_or_create_local_pet("Pip", Vec2::new(80.0, 100.0))
            .await
            .unwrap();
        let second = NeoWorldStore::open(&path)
            .await
            .unwrap()
            .load_or_create_local_pet("Other", Vec2::new(0.0, 0.0))
            .await
            .unwrap();
        assert_eq!(first.device_id, second.device_id);
        assert_eq!(first.state.id, second.state.id);
        assert_eq!(second.name, "Pip");
    }
}
