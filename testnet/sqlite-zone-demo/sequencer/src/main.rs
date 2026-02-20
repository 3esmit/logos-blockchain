use clap::Parser as _;
use demo_sqlite_sequencer::{SequencerArgs, run};

#[tokio::main]
async fn main() {
    let args = SequencerArgs::parse();
    run(args).await;
}
