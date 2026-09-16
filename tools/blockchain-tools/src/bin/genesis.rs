use core::fmt::Debug;
use std::{
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use lb_core::{
    block::genesis::{GenesisBlock, GenesisBlockBuilder},
    crypto::ZkHasher,
    mantle::{
        Note,
        ops::{channel::inscribe::InscriptionOp, sdp::SDPDeclareOp},
    },
};
use lb_node::config::deployment::DeploymentSettings;
use lb_utils::yaml::{OnUnknownKeys, deserialize_value_from_reader};
use logos_blockchain_tools::{
    genesis::{
        distribution::{self, Faucet, ProviderInfo, StakeHolderInfo},
        inscription::{self, InscribeParams},
    },
    overwrite_yaml, value_from_dotted_kv,
};
use serde_norway::Value;

// ── CLI definition
// ────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Generate deployment configs and genesis blocks for Logos Blockchain nodes"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Orchestrate the full genesis ceremony: inscribe, distribute, and build
    /// the final deployment configuration.
    Ceremony(CeremonyArgs),

    /// Generate a deployment config YAML from a well-known deployment or file,
    /// with optional field overrides.
    Config(ConfigArgs),

    /// Build a genesis block from component files and optionally embed it into
    /// a deployment config under `cryptarchia.genesis_block`.
    Block(BlockArgs),

    /// Calculate the distribution of notes and SDP declarations from
    /// stakeholder and provider definitions.
    Distribute(DistributeArgs),

    /// Generate a genesis `InscriptionOp` using entropy sources.
    Inscribe(InscribeArgs),
}

// ── ceremony subcommand
// ──────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
pub struct CeremonyArgs {
    /// Genesis parameters for the `InscriptionOp`.
    #[arg(long, value_name = "FILE")]
    pub inscription_params: PathBuf,

    /// Stakeholder definitions for note distribution.
    #[arg(long, value_name = "FILE")]
    pub stake_holders: PathBuf,

    /// Provider definitions for SDP declarations.
    #[arg(long, value_name = "FILE")]
    pub providers: PathBuf,

    /// Faucet definition for stake distribution.
    #[arg(long, value_name = "FILE")]
    pub faucet: PathBuf,

    /// The path to a custom deployment config.
    #[arg(long = "deployment", value_name = "FILE")]
    pub custom_deployment_path: Option<PathBuf>,

    /// Optional overrides for the deployment config.
    #[arg(long = "override", value_name = "KEY=VALUE|FILE", num_args = 1)]
    pub overrides: Vec<String>,

    /// Write the final deployment config to FILE instead of stdout.
    #[arg(long, short, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

// ── config subcommand
// ─────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
struct ConfigArgs {
    /// The path to a custom deployment config.
    #[arg(long = "deployment", value_name = "FILE")]
    pub custom_deployment_path: Option<PathBuf>,

    /// Override to apply on top of the base config. Each occurrence is either
    /// a dot-notation key=value pair (e.g. `cryptarchia.security_param=60`)
    /// or a path to a YAML file that is deep-merged into the config.
    /// Repeated flags are applied left-to-right.
    #[arg(long = "override", value_name = "KEY=VALUE|FILE", num_args = 1)]
    overrides: Vec<String>,

    /// Write output to FILE instead of stdout.
    #[arg(long, short, value_name = "FILE")]
    output: Option<PathBuf>,
}

// ── block subcommand
// ──────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
struct BlockArgs {
    /// YAML file containing the list of genesis notes.
    /// Each entry must have `value` (u64) and `pk` (hex-encoded `ZkPublicKey`).
    /// At least one note is required.
    ///
    /// Example:
    ///   - value: 100000 pk: eb3158fd...
    #[arg(long, value_name = "FILE")]
    notes: PathBuf,

    /// YAML file containing the genesis `InscriptionOp`.
    /// Must have `channel_id`, `inscription`, `parent`, and `signer` fields.
    ///
    /// Example:
    /// ```yaml
    ///   channel_id: '0000...0000'
    ///   inscription: [103, 101, 110, 101, 115, 105, 115]
    ///   parent: '0000...0000'
    ///   signer: '0000...0000'
    /// ```
    #[arg(long, value_name = "FILE")]
    inscription: PathBuf,

    /// YAML file containing the list of `SDPDeclareOps`.
    /// Each entry must have `service_type`, `locators`, `provider_id`,
    /// `zk_id`, and `locked_note_id` fields.
    /// At least one declaration is required.
    #[arg(long, value_name = "FILE")]
    declarations: PathBuf,

    /// Existing deployment config YAML to embed the genesis block into.
    /// When provided, the block is written into `cryptarchia.genesis_block`
    /// and the merged config is written to --output. Without this flag,
    /// only the serialized genesis block is written.
    #[arg(long, value_name = "FILE")]
    embed_in: Option<PathBuf>,

    /// Write output to FILE instead of stdout.
    #[arg(long, short, value_name = "FILE")]
    output: Option<PathBuf>,
}

// ── distribute subcommand
// ──────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
struct DistributeArgs {
    /// YAML file containing stakeholder info.
    #[arg(long, value_name = "FILE")]
    stake_holders: PathBuf,

    /// YAML file containing provider info.
    #[arg(long, value_name = "FILE")]
    providers: PathBuf,

    /// YAML file containing faucet info.
    #[arg(long, value_name = "FILE")]
    faucet: PathBuf,

    /// Write notes output to FILE instead of stdout.
    #[arg(long, short, value_name = "FILE")]
    notes_output: Option<PathBuf>,

    /// Write declarations output to FILE instead of stdout.
    #[arg(long, short, value_name = "FILE")]
    declarations_output: Option<PathBuf>,
}

// ── inscribe subcommand
// ──────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
struct InscribeArgs {
    /// YAML file containing genesis parameters (`chain_id`, `genesis_time`, and
    /// `entropy_sources`). `entropy_sources` should be a list of hex-encoded
    /// 32-byte strings.

    #[arg(long, value_name = "FILE")]
    params: PathBuf,

    /// Write the serialized `InscriptionOp` to FILE instead of stdout.
    #[arg(long, short, value_name = "FILE")]
    output: Option<PathBuf>,
}

// ── entry point
// ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Ceremony(args) => run_ceremony(&args),
        Commands::Config(args) => run_config(&args),
        Commands::Block(args) => run_block(&args),
        Commands::Distribute(args) => run_distribute(&args),
        Commands::Inscribe(args) => run_inscribe(&args),
    }
}

// ── ceremony implementation
// ─────────────────────────────────────────────────────

fn run_ceremony(args: &CeremonyArgs) -> Result<()> {
    let inscribe_params: InscribeParams = load_yaml_file(&args.inscription_params)?;
    let inscription_op = inscription::inscribe::<ZkHasher>(
        inscribe_params.chain_id,
        inscribe_params.genesis_time,
        inscribe_params.entropy_sources,
    );

    let stakeholders: Vec<StakeHolderInfo> = load_yaml_file(&args.stake_holders)?;
    let providers: Vec<ProviderInfo> = load_yaml_file(&args.providers)?;
    let faucet: Faucet = load_yaml_file(&args.faucet)?;
    let (transfer_op, declarations) = distribution::distribute(stakeholders, providers, &faucet)
        .map_err(|e| anyhow::anyhow!(e))
        .context("Failed to calculate distribution during ceremony")?;
    let notes: Vec<Note> = transfer_op.notes().collect();

    let mut config_value = load_base_config(args.custom_deployment_path.as_ref())?;
    for raw in &args.overrides {
        let patch = resolve_override(raw)?;
        config_value = overwrite_yaml(config_value, patch);
    }

    if notes.is_empty() {
        bail!("Ceremony failed: distribution resulted in zero notes");
    }
    if declarations.is_empty() {
        bail!("Ceremony failed: distribution resulted in zero declarations");
    }
    let genesis_block = build_genesis_block(notes, inscription_op, declarations)?;

    let block_value = struct_to_yaml_value(&genesis_block)?;
    let block_patch = wrap_as_cryptarchia_genesis_block(block_value);
    config_value = overwrite_yaml(config_value, block_patch);

    let faucet_pk_value = struct_to_yaml_value(&faucet.zk_id)?;
    let faucet_patch = wrap_as_cryptarchia_faucet_pk(faucet_pk_value);
    let final_config = overwrite_yaml(config_value, faucet_patch);

    ensure_valid_deployment_settings(&final_config)?;

    write_yaml(&final_config, args.output.as_deref())
}

// ── config implementation
// ─────────────────────────────────────────────────────

fn run_config(args: &ConfigArgs) -> Result<()> {
    let mut config = load_base_config(args.custom_deployment_path.as_ref())?;

    for raw in &args.overrides {
        let patch = resolve_override(raw)?;
        config = overwrite_yaml(config, patch);
    }

    ensure_valid_deployment_settings(&config)?;

    write_yaml(&config, args.output.as_deref())
}

/// Load a deployment config as a raw YAML value.
///
/// If `path` is `None`, returns the default config as a YAML value. Otherwise,
/// loads the YAML file at `path` and returns it as a `Value`.
fn load_base_config(path: Option<&PathBuf>) -> Result<Value> {
    let Some(path) = path else {
        let default_config = DeploymentSettings::default();
        return struct_to_yaml_value(&default_config);
    };

    let content = fs::read_to_string(path)
        .with_context(|| format!("cannot read config file '{}'", path.display()))?;
    serde_norway::from_str(&content)
        .with_context(|| format!("cannot parse YAML from '{}'", path.display()))
}

/// Resolve a single `--override` argument.
///
/// If `s` contains `=`, it is parsed as a dotted `key=value` pair.
/// Otherwise it is treated as a path to a YAML file.
fn resolve_override(s: &str) -> Result<Value> {
    if s.contains('=') {
        return value_from_dotted_kv(s).map_err(|e| anyhow::anyhow!(e));
    }

    let path = Path::new(s);
    let content = fs::read_to_string(path)
        .with_context(|| format!("cannot read override file '{}'", path.display()))?;
    serde_norway::from_str(&content)
        .with_context(|| format!("cannot parse YAML from override file '{}'", path.display()))
}

// ── block implementation
// ──────────────────────────────────────────────────────

fn run_block(args: &BlockArgs) -> Result<()> {
    let notes: Vec<Note> = load_yaml_file(&args.notes)?;
    let inscription: InscriptionOp = load_yaml_file(&args.inscription)?;
    let declarations: Vec<SDPDeclareOp> = load_yaml_file(&args.declarations)?;

    if notes.is_empty() {
        bail!("notes file must contain at least one Note");
    }
    if declarations.is_empty() {
        bail!("declarations file must contain at least one SDPDeclareOp");
    }

    let genesis_block = build_genesis_block(notes, inscription, declarations)?;

    let result = match args.embed_in {
        Some(ref embed_path) => {
            let block_value = struct_to_yaml_value(&genesis_block)?;
            let patch = wrap_as_cryptarchia_genesis_block(block_value);
            let base: Value = load_yaml_file(embed_path)?;
            overwrite_yaml(base, patch)
        }
        None => struct_to_yaml_value(&genesis_block)?,
    };

    write_yaml(&result, args.output.as_deref())
}

/// Drive the [`GenesisBlockBuilder`] typestate machine with the supplied
/// components and return the finished [`GenesisBlock`].
fn build_genesis_block(
    notes: Vec<Note>,
    inscription: InscriptionOp,
    declarations: Vec<SDPDeclareOp>,
) -> Result<GenesisBlock> {
    let mut notes_iter = notes.into_iter();
    let mut decls_iter = declarations.into_iter();

    // Non-emptiness is checked by the caller, so these unwraps are safe.
    let first_note = notes_iter.next().unwrap();
    let first_decl = decls_iter.next().unwrap();

    // Accumulate additional notes into WithNotes state.
    let mut builder = GenesisBlockBuilder::new().add_note(first_note);
    for note in notes_iter {
        builder = builder
            .try_add_note(note)
            .context("failed to append note to genesis transfer")?;
    }

    // Transition: WithNotes → WithNotesAndInscription → WithAll.
    let mut builder = builder
        .set_inscription(inscription)
        .add_declaration(first_decl);
    for decl in decls_iter {
        builder = builder.add_declaration(decl)?;
    }

    builder.build().context("failed to build genesis block")
}

/// Wrap a serialised `GenesisBlock` value in the mapping that corresponds to
/// `cryptarchia.genesis_block` in a deployment config.
fn wrap_as_cryptarchia_genesis_block(block_value: Value) -> Value {
    let mut inner = serde_norway::Mapping::new();
    inner.insert(Value::String("genesis_block".to_owned()), block_value);

    let mut outer = serde_norway::Mapping::new();
    outer.insert(
        Value::String("cryptarchia".to_owned()),
        Value::Mapping(inner),
    );

    Value::Mapping(outer)
}

/// Wrap a serialised faucet public key in the mapping that corresponds to
/// `cryptarchia.faucet_pk` in a deployment config.
fn wrap_as_cryptarchia_faucet_pk(faucet_pk: Value) -> Value {
    let mut inner = serde_norway::Mapping::new();
    inner.insert(Value::String("faucet_pk".to_owned()), faucet_pk);

    let mut outer = serde_norway::Mapping::new();
    outer.insert(
        Value::String("cryptarchia".to_owned()),
        Value::Mapping(inner),
    );

    Value::Mapping(outer)
}

// ── distribute implementation
// ─────────────────────────────────────────────────────

fn run_distribute(args: &DistributeArgs) -> Result<()> {
    let stakeholders: Vec<StakeHolderInfo> = load_yaml_file(&args.stake_holders)?;
    let providers: Vec<ProviderInfo> = load_yaml_file(&args.providers)?;
    let faucet: Faucet = load_yaml_file(&args.faucet)?;

    let (transfer_op, declarations) = distribution::distribute(stakeholders, providers, &faucet)
        .map_err(|e| anyhow::anyhow!(e))
        .context("Failed to calculate distribution")?;
    let notes: Vec<Note> = transfer_op.notes().collect();

    let notes_value = struct_to_yaml_value(&notes)?;
    let declarations_value = struct_to_yaml_value(&declarations)?;

    write_yaml(&notes_value, args.notes_output.as_deref())?;
    write_yaml(&declarations_value, args.declarations_output.as_deref())?;

    Ok(())
}

// ── inscribe implementation
// ─────────────────────────────────────────────────────

fn run_inscribe(args: &InscribeArgs) -> Result<()> {
    let params: InscribeParams = load_yaml_file(&args.params)?;

    let op = inscription::inscribe::<ZkHasher>(
        params.chain_id,
        params.genesis_time,
        params.entropy_sources,
    );

    let op_value = struct_to_yaml_value(&op)?;
    write_yaml(&op_value, args.output.as_deref())
}

// ── shared helpers
// ────────────────────────────────────────────────────────────

/// Serialize a value to a human-readable YAML [`Value`].
///
/// Keep the text serialization boundary used by the ceremony tools: types
/// guarded by `is_human_readable()` (such as `HeaderId`) retain their readable
/// representation. Unsupported raw byte serialization remains an error rather
/// than silently changing the generated configuration to a sequence of numbers.
fn struct_to_yaml_value<T: serde::Serialize>(value: &T) -> Result<Value> {
    let yaml_string = serde_norway::to_string(value)?;
    serde_norway::from_str(&yaml_string).map_err(Into::into)
}

fn ensure_valid_deployment_settings(value: &Value) -> Result<()> {
    let yaml = serde_norway::to_string(value)?;
    drop(
        deserialize_value_from_reader::<DeploymentSettings, _>(
            yaml.as_bytes(),
            OnUnknownKeys::Fail,
        )
        .context("generated config is not a valid DeploymentSettings value")?,
    );
    Ok(())
}

fn load_yaml_file<T>(path: &Path) -> Result<T>
where
    T: serde::de::DeserializeOwned + Send + Sync + Debug + 'static,
{
    let content =
        fs::read_to_string(path).with_context(|| format!("cannot read '{}'", path.display()))?;
    deserialize_value_from_reader(content.as_bytes(), OnUnknownKeys::Fail)
        .with_context(|| format!("cannot parse YAML from '{}'", path.display()))
}

fn write_yaml(value: &Value, output: Option<&Path>) -> Result<()> {
    let yaml = serde_norway::to_string(value)?;
    output.map_or_else(
        || {
            io::stdout()
                .write_all(yaml.as_bytes())
                .context("cannot write to stdout")
        },
        |path| {
            fs::write(path, yaml.as_bytes())
                .with_context(|| format!("cannot write to '{}'", path.display()))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct HumanReadableProbe;

    impl serde::Serialize for HumanReadableProbe {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            if serializer.is_human_readable() {
                serializer.serialize_str("human-readable")
            } else {
                serializer.serialize_u64(42)
            }
        }
    }

    struct BytesProbe;

    impl serde::Serialize for BytesProbe {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_bytes(&[0, 1, 255])
        }
    }

    #[test]
    fn yaml_conversion_preserves_human_readable_and_byte_error_contracts() {
        assert_eq!(
            struct_to_yaml_value(&HumanReadableProbe).unwrap(),
            Value::String("human-readable".to_owned())
        );
        assert!(struct_to_yaml_value(&BytesProbe).is_err());
    }

    #[test]
    fn yaml_conversion_preserves_header_hex_and_enum_tags() {
        #[derive(serde::Serialize)]
        enum Tagged {
            Header(lb_core::header::HeaderId),
        }

        let value = struct_to_yaml_value(&Tagged::Header([0x12; 32].into())).unwrap();
        let expected: Value =
            serde_norway::from_str(&format!("!Header '{}'\n", "12".repeat(32))).unwrap();
        assert_eq!(value, expected);
        assert_eq!(
            serde_norway::from_str::<Value>(&serde_norway::to_string(&value).unwrap()).unwrap(),
            expected
        );
    }

    #[test]
    fn ceremony_templates_round_trip_without_value_changes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deployment/ceremony/genesis");
        for environment in ["standalone", "testnet", "devnet"] {
            for name in [
                "providers",
                "stakeholders",
                "inscribe",
                "deployment-template",
                "faucet",
            ] {
                let path = root.join(environment).join(format!("{name}.yaml"));
                let input = fs::read_to_string(&path).unwrap();
                let value: Value = serde_norway::from_str(&input).unwrap();
                let output = serde_norway::to_string(&value).unwrap();
                // Exercise the node's existing YAML reader as well as the
                // tools' serializer, without rewriting ceremony inputs.
                let decoded: Value =
                    deserialize_value_from_reader(output.as_bytes(), OnUnknownKeys::Fail).unwrap();
                assert_eq!(decoded, value, "template: {}", path.display());
            }
        }
    }
}
