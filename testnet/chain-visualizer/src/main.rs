use std::collections::{BTreeMap, HashMap};

use clap::Parser;
use lb_chain_service::BranchesInfo;
use lb_common_http_client::CommonHttpClient;
use lb_core::header::HeaderId;
use url::Url;

#[derive(Parser, Debug)]
#[command(about = "Logos Blockchain Visualizer")]
struct Args {
    /// Node HTTP API URL (e.g., <http://localhost:8080>)
    #[arg(short, long, default_value = "http://localhost:8080")]
    url: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let base_url = Url::parse(&args.url)?;
    let client = CommonHttpClient::new(None);

    let branches = client.get_branches(base_url).await?;
    print_branch_graph(&branches);

    Ok(())
}

fn print_branch_graph(info: &BranchesInfo) {
    println!("TIP:  {}", format_header_id(&info.tip));
    println!("LIB:  {}", format_header_id(&info.lib));
    println!();

    if info.branches.is_empty() {
        println!("No blocks in chain (at genesis)");
        return;
    }

    let (block_columns, num_columns, blocks_by_height) = build_graph(info);

    println!("Blocks (height | slots | ids):");
    println!();

    // Print from highest to lowest height
    for (&height, blocks) in blocks_by_height.iter().rev() {
        // Build the graph row with all blocks at this height
        let mut row = vec!["  "; num_columns];
        if !blocks.is_empty() {
            let mut min_col = usize::MAX;
            let mut max_col = 0;
            for block in blocks {
                if let Some(&col) = block_columns.get(block) {
                    min_col = min_col.min(col);
                    max_col = max_col.max(col);
                }
            }

            for block in blocks {
                let column = block_columns.get(block).copied().unwrap_or(0);
                if column == min_col {
                    row[column] = " *";
                } else {
                    row[column] = "-*";
                }
            }

            (min_col..=max_col).for_each(|c| {
                if row[c] == "  " {
                    row[c] = "--";
                }
            });
        }
        let graph_part: String = row.join("");

        // Build slot string
        let slots: Vec<String> = blocks
            .iter()
            .map(|id| {
                info.branches
                    .get(id)
                    .expect("branch must exist")
                    .slot
                    .into_inner()
                    .to_string()
            })
            .collect();
        let slot_str = slots.join(",");

        // Build ID string with markers
        let id_parts: Vec<String> = blocks
            .iter()
            .map(|&id| {
                let mut markers = Vec::new();
                if id == info.lib {
                    markers.push("LIB");
                }
                if id == info.tip {
                    markers.push("TIP");
                }
                if info.branch_tips.contains(&id) && id != info.tip {
                    markers.push("FORK");
                }

                let id = format_header_id(&id);
                if markers.is_empty() {
                    id
                } else {
                    format!("{}[{}]", id, markers.join(","))
                }
            })
            .collect();
        let id_str = id_parts.join(" | ");

        println!("{graph_part} h={height:<4} s={slot_str:<8} {id_str}");
    }

    println!();
    println!("Legend: * = block, -- = fork connection");
}

fn build_graph(
    info: &BranchesInfo,
) -> (
    HashMap<HeaderId, usize>,
    usize,
    BTreeMap<u64, Vec<HeaderId>>,
) {
    // Assign each block to a column (fork lane)
    // Honest chain gets column 0 (leftmost), forks get columns to the right
    let mut block_columns: HashMap<HeaderId, usize> = HashMap::new();

    // First, assign honest chain to column 0 (leftmost)
    let mut current = info.branches.get(&info.tip);
    while let Some(branch) = current {
        block_columns.insert(branch.id, 0);
        if branch.parent == branch.id {
            break; // Reached genesis
        }
        current = info.branches.get(&branch.parent);
    }

    // Then, assign fork branches to columns 1, 2, 3, ... (right of honest chain)
    let mut next_column = 1usize;
    for branch in info.branches.values() {
        if branch.id == info.tip {
            continue; // Skip honest chain, already assigned
        }
        if !block_columns.contains_key(&branch.id) {
            let column = next_column;
            next_column += 1;

            let mut current = Some(branch);
            while let Some(branch) = current {
                if block_columns.contains_key(&branch.id) {
                    break; // Hit the honest chain or another fork
                }
                block_columns.insert(branch.id, column);
                if branch.parent == branch.id {
                    break; // Reached genesis
                }
                current = info.branches.get(&branch.parent);
            }
        }
    }
    let num_columns = next_column.max(1);

    // Group blocks by height
    let mut blocks_by_height: BTreeMap<u64, Vec<HeaderId>> = BTreeMap::new();
    for branch in info.branches.values() {
        blocks_by_height
            .entry(branch.height)
            .or_default()
            .push(branch.id);
    }

    // Sort blocks within each height by column for consistent ordering
    for blocks in blocks_by_height.values_mut() {
        blocks.sort_by_key(|b| block_columns.get(b).copied().unwrap_or(0));
    }
    (block_columns, num_columns, blocks_by_height)
}

fn format_header_id(id: &HeaderId) -> String {
    let hex = id.to_string();
    // Remove 0x prefix and take first 6 chars
    hex.trim_start_matches("0x").chars().take(6).collect()
}
