use rusqlite::{Connection, Result, OptionalExtension, trace::{TraceEvent, TraceEventCodes}};
use std::{fs::File, sync::Mutex, io::Write};

use serde::{Deserialize, Serialize};

const LAST_MSG_ID_KEY: i64 = 0;

static TRACE_FILE: Mutex<Option<File>> = Mutex::new(None);

pub struct MessageIdTable {
    pub db: Mutex<Connection>,
}

impl MessageIdTable {
    pub fn new(path: &str) -> Result<Self> {
        let db = Connection::open(path)?;

        db.execute(
            "CREATE TABLE IF NOT EXISTS state (
                key    INTEGER PRIMARY KEY,
                value  BLOB
            )",
            ()
        )?;

        Ok(Self {db: Mutex::new(db)})
    }

    pub async fn get_last_msg_id(&self) -> Result<Option<[u8; 32]>> {
        let db = self.db.lock().unwrap();
        let bytes: Option<Vec<u8>> = db
            .query_row(
                "SELECT value FROM state WHERE key = ?1",
                [LAST_MSG_ID_KEY],
                |row| row.get(0),
            )
            .optional()?; // ← converts "no row" into None

        let bytes = match bytes {
            Some(b) if b.len() == 32 => b,
            _ => return Ok(None),
        };

        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);

        Ok(Some(arr))
    }

    pub async fn set_last_msg_id(&self, msg_id: &[u8; 32]) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)",
            (LAST_MSG_ID_KEY, &msg_id),

        )?;

        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Dish {
    id: i64,
    name: String,
    data: String,
}

pub struct Menu {
    pub path: String,
    pub queue_file: Option<String>,
}

impl Menu {
    pub fn new(path: &str) -> Result<Self> {
        let db = Connection::open(path)?;

        db.execute(
            "CREATE TABLE IF NOT EXISTS menu (
                id    INTEGER PRIMARY KEY,
                name  TEXT NOT NULL,
                data  BLOB
            )",
            (), // empty list of parameters.
        )?;

        Ok(Self {path: path.to_string(), queue_file: None})
    }

    /// Set up tracing to write SQL statements to the queue file
    pub fn enable_tracing(&mut self, queue_file: &str) {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(queue_file)
            .expect("Failed to open queue file");

        *TRACE_FILE.lock().unwrap() = Some(file);
        self.queue_file = Some(queue_file.to_string());
    }

    /// Open a connection with tracing enabled if configured
    fn open_traced_connection(&self) -> Result<Connection> {
        let db = Connection::open(self.path.clone())?;
        if self.queue_file.is_some() {
            db.trace_v2(
                TraceEventCodes::SQLITE_TRACE_STMT,
                Some(Self::sqlite_trace_fn),
            );
        }
        Ok(db)
    }

    pub async fn insert(&self, name: &str, data: &str) -> Result<String> {
        let name = name.to_owned();
        let data = data.to_owned();
        let db = self.open_traced_connection()?;

        let name2 = name.clone();

        tokio::task::spawn_blocking(move || {
            db.execute(
                "INSERT INTO menu (name, data) VALUES (?1, ?2)",
                (&name, &data),
            )?;
            Ok::<(), rusqlite::Error>(())
        });

        Ok(name2)
    }

    pub async fn select(&self, query: String) -> Result<Vec<Dish>> {
        let db = self.open_traced_connection()?;
        println!("1 {}", query);

        let dishes = tokio::task::spawn_blocking(move || -> Result<Vec<Dish>, rusqlite::Error> {

            println!("2 {}", query);
            let mut stmt = db.prepare(&query)?;
            let rows = stmt.query_map([], |row| {
                Ok(Dish {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    data: row.get(2)?,
                })
            })?;

            rows.collect()
        })
        .await
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))??;

        Ok(dishes)
    }    

    fn sqlite_trace_fn(event: TraceEvent<'_>) {
        if let TraceEvent::Stmt(stmt, _) = event {
            if let Ok(mut guard) = TRACE_FILE.lock() {
                if let Some(file) = guard.as_mut() {
                    let _ = writeln!(file, "{:?}", stmt.expanded_sql());
                }
            }
        }
    }
}