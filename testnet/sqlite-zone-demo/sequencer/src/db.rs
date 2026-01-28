use rusqlite::{Connection, Result};

const LAST_MSG_INDEX_KEY: i64 = 0;

pub struct Query {
    index: u64,
    id: &[u8; 32],
    parent: &[u8; 32],
    data: String,
}

pub struct QueriesTables {
    db: Connection,
}

impl QueriesTables {
    pub fn new(path: &str) -> Result<Self> {
        let db = Connection::open(path)?;

        db.execute_batch(
            "CREATE TABLE queries (
                index INTEGER PRIMARY KEY,
                id    BLOB,
                parent BLOB,
                data  BLOB
            );
            CREATE TABLE queue (
                index INTEGER PRIMARY KEY,
                id    BLOB,
                parent BLOB,
                data  BLOB
            );
            CREATE TABLE state (
                key    INTEGER PRIMARY KEY,
                value  INTEGER
            );",
        )?;

        Ok(Self {conn})
    }

    pub async fn new_query(&self, id: &[u8; 32], data: &str) -> Result<(i64)> {
        let index = self.db.get_last_msg_index();

        let last_id = self.db.query_row(
            "SELECT id FROM queries WHERE index=?1",
            [index],
            |row| row.get(0),
        )

        index++;

        self.db.execute(
            "INSERT INTO queries (index, id, parent, data) VALUES (?1, ?2, ?3, ?4)",
            [(&index, &id, &last_id, &data)],
            |row| row.get(0),
        );

        self.db.execute(
            "INSERT INTO queue (index, id, data, parent) VALUES (?1, ?2, ?3, ?4)",
            (&index, &id, &last_id, &data),
        )?;

        self.db.set_last_msg_index(index);

        return index
    }

    pub async fn get_last_msg_index(&self) -> Result<i64> {
        let last_index = self.db.query_row(
            "SELECT value FROM state WHERE key=?1",
            [LAST_MSG_INDEX_KEY],
            |row| row.get(0),
        )

        return last_index
    }

    async fn set_last_msg_index(&self, index: &i64) -> Result<()> {
        self.db.execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)",
            (LAST_MSG_INDEX_KEY, &index),

        )?;

        Ok(())
    }

    pub async fn queue_drain(&self) -> Result<Vec<Query>> {
        let mut stmt = self.db.prepare("SELECT * FROM queue")?;
        let queue_iter = stmt.query_map([], |row| {
            Ok(Query {
                index: row.get(0)?,
                id: row,get(1)?,
                parent: row.get(2)?,
                data: row.get(3)?,
            })
        })?;

        let mut queue_vec = Vec::new();
        for query in queue_iter {
            queue_vec.push(query?);
        }
    }

    pub async fn queue_is_empty(&self) -> Result<bool> {
        let stmt = self.db.prepare("SELECT * FROM queue")?;
        let mut rows = stmt.query([])?;
        Ok(rows.count == 0)
    }
}