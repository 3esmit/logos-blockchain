use clap::Parser as _;

use crate::config::CliArgs;

#[test]
fn parse_config_path() {
    let parsed_args = CliArgs::parse_from(["", "test_cfg.yaml"]);
    assert_eq!(parsed_args.config_path().to_str().unwrap(), "test_cfg.yaml");
}

#[test]
fn parse_dry_run_flag() {
    let parsed_args = CliArgs::parse_from(["", "test_cfg.yaml", "--check-config"]);
    assert!(parsed_args.dry_run());
}
