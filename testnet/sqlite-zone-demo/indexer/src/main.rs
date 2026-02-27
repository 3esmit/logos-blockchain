use clap::Parser as _;
use demo_sqlite_indexer::{IndexerArgs, run};

#[tokio::main]
async fn main() {
    let args = IndexerArgs::parse();
    drop(run(args).await);
}
