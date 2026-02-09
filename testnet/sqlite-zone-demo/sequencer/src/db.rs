use rusqlite::{Connection, Result, trace::{TraceEvent, TraceEventCodes}};
use std::{fs::File, sync::Mutex, io::Write};

use serde::{Deserialize, Serialize};

static TRACE_FILE: Mutex<Option<File>> = Mutex::new(None);

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

    pub async fn query(&self, sql: String) -> Result<Option<Vec<Dish>>> {                                                                                                                                     
        let db = self.open_traced_connection()?;
                                                                                                                                                                                                              
        tokio::task::spawn_blocking(move || {
            let mut stmt = db.prepare(&sql)?;
            if stmt.column_count() > 0 {
                let rows = stmt.query_map([], |row| {
                    Ok(Dish {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        data: row.get(2)?,
                    })
                })?;
                Ok(Some(rows.collect::<Result<Vec<_>>>()?))
            } else {
                stmt.execute([])?;
                Ok(None)
            }
        })
        .await
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
    }
  
   

    fn sqlite_trace_fn(event: TraceEvent<'_>) {
        if let TraceEvent::Stmt(stmt, _) = event {
            if let Some(sql) = stmt.expanded_sql() {
                if sql.trim_start().to_uppercase().starts_with("SELECT") {
                    return;
                }
                if let Ok(mut guard) = TRACE_FILE.lock() {
                    if let Some(file) = guard.as_mut() {
                        let _ = writeln!(file, "{}", sql);
                    }
                }
            }
        }
    }
}