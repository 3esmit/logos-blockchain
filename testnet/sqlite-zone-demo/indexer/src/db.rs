use rusqlite::{Connection, Result};

use demo_sqlite_sequencer::db::Dish;

pub struct MenuReadOnly {
    pub path: String,
}

impl MenuReadOnly {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS menu (
                id    INTEGER PRIMARY KEY,
                name  TEXT NOT NULL,
                data  BLOB
            )",
            (), // empty list of parameters.
        )?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        Ok(Self { path: path.to_string() })
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let db = Connection::open(self.path.clone())?;
        db.execute_batch(sql)
    }

    pub async fn query(&self, sql: String) -> Result<Vec<Dish>> {
        let db = Connection::open(self.path.clone())?;

        tokio::task::spawn_blocking(move || {
            let mut stmt = db.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok(Dish {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    data: row.get(2)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>>>()?)
        })
        .await
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
    }
}
