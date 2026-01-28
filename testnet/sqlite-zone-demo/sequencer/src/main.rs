use rusqlite::{Connection, Result};


#[derive(Debug)]
pub struct Dish {
    id: i32,
    name: String,
    data: String,
}

pub struct Menu {
    db: Connection,
}

impl Menu {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;

        conn.execute(
            "CREATE TABLE menu (
                id    INTEGER PRIMARY KEY,
                name  TEXT NOT NULL,
                data  BLOB
            )",
            (), // empty list of parameters.
        )?;

        Ok(Self {conn})
    }

    pub async fn insert(&self, name: &str, data: &str) -> Result<()> {
        conn.execute(
            "INSERT INTO menu (name, data) VALUES (?1, ?2)",
            (&name, &data),
        )?;
    }

    pub async fn select(&self, query: &str) -> Result<Vec<Dish>> {
        let mut stmt = conn.prepare(query)?;
        let dish_iter = stmt.query_map([], |row| {
            Ok(Dish {
                id: row.get(0)?,
                name: row.get(1)?,
                data: row.get(2)?,
            })
        })?;

        let mut dishes = Vec::new();
        for dish in dish_iter {
            dishes.push(dish?);
        }
        return dishes;
    }
}


fn main() -> Result<()> {
    
    
    Ok(())
}